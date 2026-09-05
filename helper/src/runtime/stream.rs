//! In-memory adapters for the production coordinator and writer.

use super::{
    CRITICAL_OUTBOUND_QUEUE_CAPACITY, FINAL_OUTBOUND_QUEUE_CAPACITY, INPUT_QUEUE_CAPACITY,
    OUTBOUND_QUEUE_CAPACITY,
    coordinator::{CoordinatorOutcome, coordinate, outcome_after_shutdown, stdin_loop},
    writer::write_prioritized_messages,
};
use crate::{
    RunError,
    gateway::{ActivationCaptureGate, CallbackGate, GatewayBackend, PlatformError, TerminalSignal},
    protocol::{Outbound, Server},
};
use crossbeam_channel::{Sender, bounded};
use std::{
    io::{Read, Write},
    sync::Arc,
    thread,
};

/// In-memory production coordinator used by framing/property integration tests.
/// It shares the exact bounded, detached stdin reader, coordinator, server,
/// outbound encoder, and frame writer. `output` must be an in-memory/nonblocking
/// writer; unlike `run`, this test adapter joins its scoped writer directly so
/// it cannot safely detach a borrow.
#[doc(hidden)]
pub fn run_framed_stream<P: GatewayBackend, R: Read + Send + 'static, W: Write + Send>(
    platform: P,
    input: R,
    output: W,
) -> Result<(), RunError> {
    run_framed_stream_with_factory(input, output, |_, _, _, _| Ok(platform))
}

/// In-memory coordinator variant which constructs the platform through the
/// production `GatewayBackend::start` boundary. This exists only for integration
/// tests that need real callback senders and the exact production writer.
#[doc(hidden)]
pub fn run_framed_stream_started<P: GatewayBackend, R: Read + Send + 'static, W: Write + Send>(
    input: R,
    output: W,
) -> Result<(), RunError> {
    run_framed_stream_with_factory(input, output, P::start)
}

fn run_framed_stream_with_factory<
    P: GatewayBackend,
    R: Read + Send + 'static,
    W: Write + Send,
    F: FnOnce(
        Sender<Outbound>,
        Arc<CallbackGate>,
        Arc<TerminalSignal>,
        ActivationCaptureGate,
    ) -> Result<P, PlatformError>,
>(
    input: R,
    output: W,
    platform_factory: F,
) -> Result<(), RunError> {
    run_framed_stream_with_factory_and_gate_inner(
        input,
        output,
        ActivationCaptureGate::default(),
        platform_factory,
    )
}

/// Cross-process integration seam for exercising enable reconciliation. It is
/// absent from optimized/package builds and accepts no endpoint credentials.
#[cfg(debug_assertions)]
#[doc(hidden)]
pub fn run_framed_stream_with_factory_and_gate<
    P: GatewayBackend,
    R: Read + Send + 'static,
    W: Write + Send,
    F: FnOnce(
        Sender<Outbound>,
        Arc<CallbackGate>,
        Arc<TerminalSignal>,
        ActivationCaptureGate,
    ) -> Result<P, PlatformError>,
>(
    input: R,
    output: W,
    activation_capture_gate: ActivationCaptureGate,
    platform_factory: F,
) -> Result<(), RunError> {
    run_framed_stream_with_factory_and_gate_inner(
        input,
        output,
        activation_capture_gate,
        platform_factory,
    )
}

fn run_framed_stream_with_factory_and_gate_inner<
    P: GatewayBackend,
    R: Read + Send + 'static,
    W: Write + Send,
    F: FnOnce(
        Sender<Outbound>,
        Arc<CallbackGate>,
        Arc<TerminalSignal>,
        ActivationCaptureGate,
    ) -> Result<P, PlatformError>,
>(
    input: R,
    mut output: W,
    activation_capture_gate: ActivationCaptureGate,
    platform_factory: F,
) -> Result<(), RunError> {
    let gate = Arc::new(CallbackGate::new());
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, outbound_rx) = bounded(OUTBOUND_QUEUE_CAPACITY);
    let (critical_tx, critical_rx) = bounded(CRITICAL_OUTBOUND_QUEUE_CAPACITY);
    let (final_tx, final_rx) = bounded(FINAL_OUTBOUND_QUEUE_CAPACITY);
    let platform = platform_factory(
        outbound_tx.clone(),
        Arc::clone(&gate),
        Arc::clone(&terminal),
        activation_capture_gate,
    )?;
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
        .name("talking-quill-helper-test-stdin".into())
        .spawn(move || stdin_loop(input, input_tx))
    {
        Ok(reader) => reader,
        Err(_) => {
            server.shutdown();
            drop(server);
            return Err(RunError::ReaderThread);
        }
    };
    // Match production: a blocked reader must not delay terminal shutdown.
    drop(stdin_reader);
    let writer_terminal = Arc::clone(&terminal);
    let (outcome, writer_result) = thread::scope(|scope| {
        let writer = scope.spawn(move || {
            write_prioritized_messages(
                final_rx,
                critical_rx,
                outbound_rx,
                writer_terminal,
                &mut output,
            )
        });
        let outcome = coordinate(&mut server, &input_rx, &terminal_rx, &terminal);
        server.shutdown();
        let outcome = outcome_after_shutdown(outcome, &terminal);
        let _terminal_snapshot = server.take_terminal_observability();
        drop(server);
        let writer_result = writer
            .join()
            .map_err(|_| RunError::WriterThread)
            .and_then(|result| result.map_err(RunError::Writer));
        (outcome, writer_result)
    });
    writer_result?;
    match outcome {
        CoordinatorOutcome::Clean => Ok(()),
        CoordinatorOutcome::Framing(error) => Err(RunError::Framing(error)),
        CoordinatorOutcome::ReaderStopped => Err(RunError::ReaderThread),
        CoordinatorOutcome::Terminal(reason) => Err(RunError::Terminal(reason)),
    }
}
