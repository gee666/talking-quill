//! Production runtime startup and bounded shutdown.

use super::{
    CLEAN_WRITER_FLUSH_TIMEOUT, CRITICAL_OUTBOUND_QUEUE_CAPACITY, FINAL_OUTBOUND_QUEUE_CAPACITY,
    INPUT_QUEUE_CAPACITY, OUTBOUND_QUEUE_CAPACITY,
    coordinator::{CoordinatorOutcome, coordinate, outcome_after_shutdown, stdin_loop},
    diagnostics::{TerminalObservabilityOutcome, publish_terminal_observability},
    writer::{wait_for_writer, writer_loop},
};
use crate::{
    RunError,
    framing::FrameError,
    gateway::{
        ActivationCaptureGate, CallbackGate, GatewayBackend, TerminalReason, TerminalSignal,
    },
    protocol::Server,
};
use crossbeam_channel::bounded;
use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    thread,
};

/// Runs the helper until stdin closes, framing fails, stdout fails, an accepted
/// `shutdown` arrives, or a native callback reports a terminal failure.
///
/// Stdin is read on a detached thread so a blocking OS read cannot prevent the
/// coordinator from immediately shutting down hooks and returning. Stdout is
/// owned exclusively by its writer thread and never receives unframed bytes.
/// Clean exit waits a bounded interval for queued frames to flush, then detaches
/// a writer blocked by a non-reading parent rather than hanging the helper.
pub fn run() -> Result<(), RunError> {
    run_with_activation_capture_gate(ActivationCaptureGate::for_process())
}

fn run_with_activation_capture_gate(
    activation_capture_gate: ActivationCaptureGate,
) -> Result<(), RunError> {
    let gate = Arc::new(CallbackGate::new());
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, outbound_rx) = bounded(OUTBOUND_QUEUE_CAPACITY);
    let (critical_tx, critical_rx) = bounded(CRITICAL_OUTBOUND_QUEUE_CAPACITY);
    let (final_tx, final_rx) = bounded(FINAL_OUTBOUND_QUEUE_CAPACITY);

    let writer_terminal = Arc::clone(&terminal);
    let (writer_done_tx, writer_done_rx) = bounded(1);
    let writer = thread::Builder::new()
        .name("talking-quill-helper-stdout".into())
        .spawn(move || {
            let panic_terminal = Arc::clone(&writer_terminal);
            let result = catch_unwind(AssertUnwindSafe(|| {
                writer_loop(final_rx, critical_rx, outbound_rx, writer_terminal)
            }))
            .unwrap_or_else(|_| {
                panic_terminal.trigger(TerminalReason::StdoutDisconnected);
                Err(FrameError::Io(io::Error::other(
                    "stdout writer thread panicked",
                )))
            });
            let _ = writer_done_tx.try_send(());
            result
        })
        .map_err(|_| RunError::WriterThread)?;

    let platform = match crate::owner::platform_client::OwnerGatewayBackend::start(
        outbound_tx.clone(),
        Arc::clone(&gate),
        Arc::clone(&terminal),
        activation_capture_gate,
    ) {
        Ok(platform) => platform,
        Err(error) => {
            gate.close();
            drop(final_tx);
            drop(critical_tx);
            drop(outbound_tx);
            let _ = wait_for_writer(writer, &writer_done_rx, CLEAN_WRITER_FLUSH_TIMEOUT);
            return Err(RunError::Gateway(error));
        }
    };
    let mut server = Server::new_with_activation_capture_gate(
        platform,
        outbound_tx.clone(),
        critical_tx.clone(),
        final_tx.clone(),
        Arc::clone(&gate),
        Arc::clone(&terminal),
        activation_capture_gate,
    );
    drop(final_tx);
    drop(critical_tx);
    drop(outbound_tx);

    let (input_tx, input_rx) = bounded(INPUT_QUEUE_CAPACITY);
    let stdin_reader = match thread::Builder::new()
        .name("talking-quill-helper-stdin".into())
        .spawn(move || stdin_loop(io::stdin(), input_tx))
    {
        Ok(reader) => reader,
        Err(_) => {
            server.shutdown();
            let snapshot = server.take_terminal_observability();
            drop(server);
            let _ = wait_for_writer(writer, &writer_done_rx, CLEAN_WRITER_FLUSH_TIMEOUT);
            publish_terminal_observability(snapshot, TerminalObservabilityOutcome::Failure);
            return Err(RunError::ReaderThread);
        }
    };
    // Deliberately detach: joining could hang forever in a blocking stdin read.
    drop(stdin_reader);

    let outcome = coordinate(&mut server, &input_rx, &terminal_rx, &terminal);
    server.shutdown();
    let outcome = outcome_after_shutdown(outcome, &terminal);
    let snapshot = server.take_terminal_observability();
    drop(server);

    // Accepted critical batches receive the same bounded flush window on
    // terminal exits; dropping them immediately can hide a committed paste.
    let writer_result = wait_for_writer(writer, &writer_done_rx, CLEAN_WRITER_FLUSH_TIMEOUT);
    let terminal_outcome = if matches!(outcome, CoordinatorOutcome::Clean)
        && matches!(writer_result, Ok(Some(Ok(()))))
    {
        TerminalObservabilityOutcome::Shutdown
    } else {
        TerminalObservabilityOutcome::Failure
    };
    publish_terminal_observability(snapshot, terminal_outcome);
    let writer_result = writer_result?;

    match writer_result {
        Some(Err(error)) => return Err(RunError::Writer(error)),
        None => return Err(RunError::WriterFlushTimeout),
        Some(Ok(())) => {}
    }

    match outcome {
        CoordinatorOutcome::Clean => Ok(()),
        CoordinatorOutcome::Framing(error) => Err(RunError::Framing(error)),
        CoordinatorOutcome::ReaderStopped => Err(RunError::ReaderThread),
        CoordinatorOutcome::Terminal(reason) => Err(RunError::Terminal(reason)),
    }
}
