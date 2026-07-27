pub mod framing;
pub mod keyboard;
pub mod owned_tree;
pub mod platform;
pub mod protocol;

use std::{
    io::{self, Read, Write},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
    thread::{self, JoinHandle},
    time::Duration,
};

use crossbeam_channel::{Receiver, Sender, bounded};
use thiserror::Error;

use crate::{
    framing::{FrameError, read_frame, write_frame},
    platform::{
        CallbackGate, NativePlatform, Platform, PlatformError, TerminalReason, TerminalSignal,
    },
    protocol::{HandleOutcome, Outbound, OutboundEncodingError, Server, encode_outbound},
};

const OUTBOUND_QUEUE_CAPACITY: usize = 256;
const CRITICAL_OUTBOUND_QUEUE_CAPACITY: usize = 1;
const INPUT_QUEUE_CAPACITY: usize = 8;
const CLEAN_WRITER_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum RunError {
    #[error("protocol framing failed: {0}")]
    Framing(#[from] FrameError),
    #[error("native platform startup failed: {0}")]
    Platform(#[from] PlatformError),
    #[error("stdin reader thread failed")]
    ReaderThread,
    #[error("stdout writer thread failed")]
    WriterThread,
    #[error("stdout protocol writer failed: {0}")]
    Writer(FrameError),
    #[error("terminal helper failure: {0:?}")]
    Terminal(TerminalReason),
}

enum InputMessage {
    Frame(Vec<u8>),
    Eof,
    Error(FrameError),
}

enum CoordinatorOutcome {
    Clean,
    Framing(FrameError),
    ReaderStopped,
    Terminal(TerminalReason),
}

/// A slot reserved before an irreversible paste operation. The writer owns the
/// slot before native dispatch begins, then receives commit and response as one
/// ordered batch.
#[doc(hidden)]
pub struct CriticalDelivery {
    acquired: Sender<()>,
    completion: Receiver<Vec<Outbound>>,
}

impl CriticalDelivery {
    pub(crate) const fn new(acquired: Sender<()>, completion: Receiver<Vec<Outbound>>) -> Self {
        Self {
            acquired,
            completion,
        }
    }

    /// Acquires this writer reservation and returns its complete ordered batch.
    #[doc(hidden)]
    pub fn accept(self) -> Option<Vec<Outbound>> {
        self.acquired.send(()).ok()?;
        self.completion.recv().ok()
    }
}

/// Runs the helper until stdin closes, framing fails, stdout fails, an accepted
/// `shutdown` arrives, or a native callback reports a terminal failure.
///
/// Stdin is read on a detached thread so a blocking OS read cannot prevent the
/// coordinator from immediately shutting down hooks and returning. Stdout is
/// owned exclusively by its writer thread and never receives unframed bytes.
/// Clean exit waits a bounded interval for queued frames to flush, then detaches
/// a writer blocked by a non-reading parent rather than hanging the helper.
pub fn run() -> Result<(), RunError> {
    let gate = Arc::new(CallbackGate::new());
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, outbound_rx) = bounded(OUTBOUND_QUEUE_CAPACITY);
    let (critical_tx, critical_rx) = bounded(CRITICAL_OUTBOUND_QUEUE_CAPACITY);

    let writer_terminal = Arc::clone(&terminal);
    let (writer_done_tx, writer_done_rx) = bounded(1);
    let writer = thread::Builder::new()
        .name("talking-quill-helper-stdout".into())
        .spawn(move || {
            let panic_terminal = Arc::clone(&writer_terminal);
            let result = catch_unwind(AssertUnwindSafe(|| {
                writer_loop(critical_rx, outbound_rx, writer_terminal)
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

    let platform = match NativePlatform::start(
        outbound_tx.clone(),
        Arc::clone(&gate),
        Arc::clone(&terminal),
    ) {
        Ok(platform) => platform,
        Err(error) => {
            gate.close();
            drop(critical_tx);
            drop(outbound_tx);
            let _ = wait_for_writer(writer, &writer_done_rx, CLEAN_WRITER_FLUSH_TIMEOUT);
            return Err(RunError::Platform(error));
        }
    };
    let mut server = Server::new(
        platform,
        outbound_tx.clone(),
        critical_tx.clone(),
        Arc::clone(&gate),
        Arc::clone(&terminal),
    );
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
            drop(server);
            let _ = wait_for_writer(writer, &writer_done_rx, CLEAN_WRITER_FLUSH_TIMEOUT);
            return Err(RunError::ReaderThread);
        }
    };
    // Deliberately detach: joining could hang forever in a blocking stdin read.
    drop(stdin_reader);

    let outcome = coordinate(&mut server, &input_rx, &terminal_rx, &terminal);
    server.shutdown();
    let outcome = outcome_after_shutdown(outcome, &terminal);
    drop(server);

    // Accepted critical batches receive the same bounded flush window on
    // terminal exits; dropping them immediately can hide a committed paste.
    let writer_result = wait_for_writer(writer, &writer_done_rx, CLEAN_WRITER_FLUSH_TIMEOUT)?;

    if let Some(Err(error)) = writer_result {
        return Err(RunError::Writer(error));
    }

    match outcome {
        CoordinatorOutcome::Clean => Ok(()),
        CoordinatorOutcome::Framing(error) => Err(RunError::Framing(error)),
        CoordinatorOutcome::ReaderStopped => Err(RunError::ReaderThread),
        CoordinatorOutcome::Terminal(reason) => Err(RunError::Terminal(reason)),
    }
}

/// In-memory production coordinator used by framing/property integration tests.
/// It shares the exact bounded, detached stdin reader, coordinator, server,
/// outbound encoder, and frame writer. `output` must be an in-memory/nonblocking
/// writer; unlike `run`, this test adapter joins its scoped writer directly so
/// it cannot safely detach a borrow.
#[doc(hidden)]
pub fn run_framed_stream<P: Platform, R: Read + Send + 'static, W: Write + Send>(
    platform: P,
    input: R,
    mut output: W,
) -> Result<(), RunError> {
    let gate = Arc::new(CallbackGate::new());
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, outbound_rx) = bounded(OUTBOUND_QUEUE_CAPACITY);
    let (critical_tx, critical_rx) = bounded(CRITICAL_OUTBOUND_QUEUE_CAPACITY);
    let mut server = Server::new(
        platform,
        outbound_tx.clone(),
        critical_tx.clone(),
        Arc::clone(&gate),
        Arc::clone(&terminal),
    );
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
            write_prioritized_messages(critical_rx, outbound_rx, writer_terminal, &mut output)
        });
        let outcome = coordinate(&mut server, &input_rx, &terminal_rx, &terminal);
        server.shutdown();
        let outcome = outcome_after_shutdown(outcome, &terminal);
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

fn wait_for_writer(
    writer: JoinHandle<Result<(), FrameError>>,
    done: &Receiver<()>,
    timeout: Duration,
) -> Result<Option<Result<(), FrameError>>, RunError> {
    match done.recv_timeout(timeout) {
        Ok(()) => writer.join().map(Some).map_err(|_| RunError::WriterThread),
        Err(crossbeam_channel::RecvTimeoutError::Disconnected) if writer.is_finished() => {
            writer.join().map(Some).map_err(|_| RunError::WriterThread)
        }
        Err(
            crossbeam_channel::RecvTimeoutError::Timeout
            | crossbeam_channel::RecvTimeoutError::Disconnected,
        ) => {
            // A blocked OS pipe write cannot be interrupted portably. Native
            // input is already stopped, so detach and let process exit end it.
            drop(writer);
            Ok(None)
        }
    }
}

fn coordinate<P: Platform>(
    server: &mut Server<P>,
    input: &Receiver<InputMessage>,
    terminal_events: &Receiver<TerminalReason>,
    terminal: &TerminalSignal,
) -> CoordinatorOutcome {
    loop {
        crossbeam_channel::select_biased! {
            recv(terminal_events) -> event => {
                let reason = event.ok().or_else(|| terminal.reason())
                    .unwrap_or(TerminalReason::HookStopped);
                return CoordinatorOutcome::Terminal(reason);
            }
            recv(input) -> message => {
                match message {
                    Ok(InputMessage::Frame(payload)) => {
                        match server.handle_payload_deferred(&payload) {
                            HandleOutcome::Continue => {}
                            HandleOutcome::Shutdown(id) => {
                                if server.complete_shutdown(id) {
                                    return CoordinatorOutcome::Clean;
                                }
                                return terminal.reason()
                                    .map_or(CoordinatorOutcome::Clean, CoordinatorOutcome::Terminal);
                            }
                            HandleOutcome::Stop => {
                                return terminal.reason()
                                    .map_or(CoordinatorOutcome::Clean, CoordinatorOutcome::Terminal);
                            }
                        }
                    }
                    Ok(InputMessage::Eof) => return CoordinatorOutcome::Clean,
                    Ok(InputMessage::Error(error)) => return CoordinatorOutcome::Framing(error),
                    Err(_) => return CoordinatorOutcome::ReaderStopped,
                }
            }
        }
    }
}

fn outcome_after_shutdown(
    outcome: CoordinatorOutcome,
    terminal: &TerminalSignal,
) -> CoordinatorOutcome {
    match (outcome, terminal.reason()) {
        (CoordinatorOutcome::Clean, Some(reason)) => CoordinatorOutcome::Terminal(reason),
        (outcome, _) => outcome,
    }
}

fn stdin_loop<R: Read>(mut input: R, sender: Sender<InputMessage>) {
    loop {
        let message = match read_frame(&mut input) {
            Ok(Some(payload)) => InputMessage::Frame(payload),
            Ok(None) => InputMessage::Eof,
            Err(error) => InputMessage::Error(error),
        };
        let terminal = !matches!(message, InputMessage::Frame(_));
        if sender.send(message).is_err() || terminal {
            return;
        }
    }
}

fn writer_loop(
    critical: Receiver<CriticalDelivery>,
    outbound: Receiver<Outbound>,
    terminal: Arc<TerminalSignal>,
) -> Result<(), FrameError> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    write_prioritized_messages(critical, outbound, terminal, &mut stdout)
}

#[cfg(test)]
fn write_messages<W: Write>(
    outbound: Receiver<Outbound>,
    terminal: Arc<TerminalSignal>,
    writer: &mut W,
) -> Result<(), FrameError> {
    let (critical_tx, critical_rx) = bounded(1);
    drop(critical_tx);
    write_prioritized_messages(critical_rx, outbound, terminal, writer)
}

fn write_prioritized_messages<W: Write>(
    critical: Receiver<CriticalDelivery>,
    outbound: Receiver<Outbound>,
    terminal: Arc<TerminalSignal>,
    writer: &mut W,
) -> Result<(), FrameError> {
    let mut critical_open = true;
    let mut outbound_open = true;
    while critical_open || outbound_open {
        // Terminal failures stop ordinary output, but accepted critical
        // deliveries remain an obligation until their sender closes.
        if terminal.is_triggered() {
            if terminal.reason() == Some(TerminalReason::StdoutDisconnected) {
                return Ok(());
            }
            outbound_open = false;
        }

        if critical_open {
            match critical.try_recv() {
                Ok(delivery) => {
                    write_critical_delivery(delivery, &terminal, writer)?;
                    continue;
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => critical_open = false,
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
        }

        let item = match (critical_open, outbound_open) {
            (true, true) => crossbeam_channel::select_biased! {
                recv(critical) -> delivery => match delivery {
                    Ok(delivery) => Some(EitherOutbound::Critical(delivery)),
                    Err(_) => {
                        critical_open = false;
                        None
                    }
                },
                recv(outbound) -> message => match message {
                    Ok(message) => Some(EitherOutbound::Ordinary(message)),
                    Err(_) => {
                        outbound_open = false;
                        None
                    }
                },
            },
            (true, false) => match critical.recv() {
                Ok(delivery) => Some(EitherOutbound::Critical(delivery)),
                Err(_) => {
                    critical_open = false;
                    None
                }
            },
            (false, true) => match outbound.recv() {
                Ok(message) => Some(EitherOutbound::Ordinary(message)),
                Err(_) => {
                    outbound_open = false;
                    None
                }
            },
            (false, false) => None,
        };
        match item {
            Some(EitherOutbound::Critical(delivery)) => {
                write_critical_delivery(delivery, &terminal, writer)?;
            }
            Some(EitherOutbound::Ordinary(message)) if !terminal.is_triggered() => {
                write_message(message, &terminal, writer)?;
            }
            Some(EitherOutbound::Ordinary(_)) | None => {}
        }
    }
    Ok(())
}

enum EitherOutbound {
    Critical(CriticalDelivery),
    Ordinary(Outbound),
}

fn write_critical_delivery<W: Write>(
    delivery: CriticalDelivery,
    terminal: &TerminalSignal,
    writer: &mut W,
) -> Result<(), FrameError> {
    let Some(batch) = delivery.accept() else {
        return Ok(());
    };
    for message in batch {
        write_message(message, terminal, writer)?;
    }
    Ok(())
}

fn write_message<W: Write>(
    message: Outbound,
    terminal: &TerminalSignal,
    writer: &mut W,
) -> Result<(), FrameError> {
    let payload = match encode_outbound(&message) {
        Ok(payload) => payload,
        Err(OutboundEncodingError::FrameTooLarge(size)) => {
            terminal.trigger(TerminalReason::OutboundEncodingUnavailable);
            return Err(FrameError::OutboundTooLarge(size));
        }
        Err(OutboundEncodingError::Serialization(error)) => {
            terminal.trigger(TerminalReason::OutboundEncodingUnavailable);
            return Err(FrameError::Io(io::Error::other(error)));
        }
    };
    if let Err(error) = write_frame(writer, &payload) {
        terminal.trigger(TerminalReason::StdoutDisconnected);
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        keyboard::{
            ActivationBinding, ActivationKey, EventPhase, HelperEvent, ProfileId, Shortcut,
        },
        protocol::{RequestId, RpcResponse},
    };

    struct BrokenWriter;

    struct BlockingWriter {
        entered: Sender<()>,
        release: Receiver<()>,
        blocked: bool,
    }

    impl Write for BrokenWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(io::ErrorKind::BrokenPipe, "closed pipe"))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl Write for BlockingWriter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if !self.blocked {
                self.blocked = true;
                let _ = self.entered.try_send(());
                let _ = self.release.recv();
            }
            Ok(buffer.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn writer_disconnect_is_terminal_and_closes_callback_gate() {
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
        let (outbound_tx, outbound_rx) = bounded(1);
        outbound_tx
            .send(Outbound::Event(HelperEvent::Activation {
                binding: ActivationBinding::new(
                    ProfileId::GENERAL,
                    Shortcut::legacy_alt_letter(ActivationKey::Z, false),
                ),
                phase: EventPhase::Down,
            }))
            .unwrap();
        drop(outbound_tx);

        let error = write_messages(outbound_rx, Arc::clone(&terminal), &mut BrokenWriter)
            .expect_err("broken stdout must fail");
        assert!(matches!(error, FrameError::Io(_)));
        assert!(!gate.is_open());
        assert_eq!(terminal.reason(), Some(TerminalReason::StdoutDisconnected));
        assert_eq!(
            terminal_rx.try_recv(),
            Ok(TerminalReason::StdoutDisconnected)
        );
    }

    #[test]
    fn unexpected_outbound_encoding_overflow_is_terminal() {
        let gate = Arc::new(CallbackGate::new());
        gate.open();
        let (terminal_tx, _terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
        let (outbound_tx, outbound_rx) = bounded(1);
        outbound_tx
            .send(Outbound::Response(
                RpcResponse::success(
                    RequestId::for_test(1),
                    "x".repeat(crate::framing::MAX_FRAME_BYTES),
                )
                .unwrap(),
            ))
            .unwrap();
        drop(outbound_tx);

        assert!(matches!(
            write_messages(outbound_rx, Arc::clone(&terminal), &mut Vec::new()),
            Err(FrameError::OutboundTooLarge(_))
        ));
        assert!(!gate.is_open());
        assert_eq!(
            terminal.reason(),
            Some(TerminalReason::OutboundEncodingUnavailable)
        );
    }

    #[test]
    fn terminal_failure_drains_an_accepted_critical_batch_before_ordinary_output() {
        let gate = Arc::new(CallbackGate::new());
        let (terminal_tx, _terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(gate, terminal_tx));
        let (critical_tx, critical_rx) = bounded(1);
        let (acquired_tx, _acquired_rx) = bounded(1);
        let (completion_tx, completion_rx) = bounded(1);
        let (outbound_tx, outbound_rx) = bounded(1);
        outbound_tx
            .send(Outbound::Event(HelperEvent::Activation {
                binding: ActivationBinding::new(
                    ProfileId::GENERAL,
                    Shortcut::legacy_alt_letter(ActivationKey::A, false),
                ),
                phase: EventPhase::Down,
            }))
            .unwrap();
        critical_tx
            .send(CriticalDelivery::new(acquired_tx, completion_rx))
            .unwrap();
        completion_tx
            .send(vec![
                Outbound::PasteCommitted(crate::protocol::RequestId::for_test(7)),
                Outbound::PasteCommitted(crate::protocol::RequestId::for_test(8)),
            ])
            .unwrap();
        terminal.trigger(TerminalReason::OutboundQueueUnavailable);
        drop(completion_tx);
        drop(critical_tx);
        drop(outbound_tx);

        let mut output = Vec::new();
        write_prioritized_messages(critical_rx, outbound_rx, terminal, &mut output).unwrap();
        let mut framed = io::Cursor::new(output);
        let first = read_frame(&mut framed).unwrap().unwrap();
        let first: serde_json::Value = serde_json::from_slice(&first).unwrap();
        assert_eq!(first["method"], "paste.committed");
        let second = read_frame(&mut framed).unwrap().unwrap();
        let second: serde_json::Value = serde_json::from_slice(&second).unwrap();
        assert_eq!(second["method"], "paste.committed");
        assert_eq!(second["params"]["requestId"], 8);
        assert!(read_frame(&mut framed).unwrap().is_none());
    }

    #[test]
    fn clean_writer_wait_joins_after_queued_frames_flush() {
        let gate = Arc::new(CallbackGate::new());
        let (terminal_tx, _terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(gate, terminal_tx));
        let (outbound_tx, outbound_rx) = bounded(1);
        outbound_tx
            .send(Outbound::Event(HelperEvent::Activation {
                binding: ActivationBinding::new(
                    ProfileId::GENERAL,
                    Shortcut::legacy_alt_letter(ActivationKey::Z, false),
                ),
                phase: EventPhase::Down,
            }))
            .unwrap();
        drop(outbound_tx);

        let (done_tx, done_rx) = bounded(1);
        let writer = thread::spawn(move || {
            let result = write_messages(outbound_rx, terminal, &mut Vec::new());
            let _ = done_tx.try_send(());
            result
        });

        assert!(matches!(
            wait_for_writer(writer, &done_rx, Duration::from_secs(1)),
            Ok(Some(Ok(())))
        ));
    }

    #[test]
    fn clean_writer_wait_times_out_without_joining_a_blocked_write() {
        let gate = Arc::new(CallbackGate::new());
        let (terminal_tx, _terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(gate, terminal_tx));
        let (outbound_tx, outbound_rx) = bounded(1);
        outbound_tx
            .send(Outbound::Event(HelperEvent::Activation {
                binding: ActivationBinding::new(
                    ProfileId::GENERAL,
                    Shortcut::legacy_alt_letter(ActivationKey::Z, false),
                ),
                phase: EventPhase::Down,
            }))
            .unwrap();
        drop(outbound_tx);

        let (entered_tx, entered_rx) = bounded(1);
        let (release_tx, release_rx) = bounded(1);
        let (done_tx, done_rx) = bounded(1);
        let writer = thread::spawn(move || {
            let mut output = BlockingWriter {
                entered: entered_tx,
                release: release_rx,
                blocked: false,
            };
            let result = write_messages(outbound_rx, terminal, &mut output);
            let _ = done_tx.try_send(());
            result
        });
        entered_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("writer entered blocking write");

        let started = std::time::Instant::now();
        let result = wait_for_writer(writer, &done_rx, Duration::from_millis(20)).unwrap();
        assert!(result.is_none());
        assert!(started.elapsed() < Duration::from_secs(1));

        release_tx.send(()).unwrap();
        done_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("detached writer completed after release");
    }
}
