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

use crossbeam_channel::{Receiver, Sender, bounded, unbounded};
use thiserror::Error;

use crate::{
    framing::{FrameError, read_frame, write_frame},
    platform::{
        CallbackGate, NativePlatform, Platform, PlatformError, TerminalReason, TerminalSignal,
    },
    protocol::{HandleOutcome, Outbound, OutboundEncodingError, Server, encode_outbound},
};

const OUTBOUND_QUEUE_CAPACITY: usize = 256;
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

    let writer_terminal = Arc::clone(&terminal);
    let (writer_done_tx, writer_done_rx) = bounded(1);
    let writer = thread::Builder::new()
        .name("talking-quill-helper-stdout".into())
        .spawn(move || {
            let panic_terminal = Arc::clone(&writer_terminal);
            let result = catch_unwind(AssertUnwindSafe(|| {
                writer_loop(outbound_rx, writer_terminal)
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
            drop(outbound_tx);
            let _ = wait_for_writer(writer, &writer_done_rx, CLEAN_WRITER_FLUSH_TIMEOUT);
            return Err(RunError::Platform(error));
        }
    };
    let mut server = Server::new(
        platform,
        outbound_tx.clone(),
        Arc::clone(&gate),
        Arc::clone(&terminal),
    );
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
    drop(server);

    let flush_timeout = if matches!(outcome, CoordinatorOutcome::Terminal(_)) {
        Duration::ZERO
    } else {
        CLEAN_WRITER_FLUSH_TIMEOUT
    };
    let writer_result = wait_for_writer(writer, &writer_done_rx, flush_timeout)?;

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
/// It shares the exact stdin decoder, coordinator, server, outbound encoder, and frame writer.
#[doc(hidden)]
pub fn run_framed_stream<P: Platform, R: Read, W: Write>(
    platform: P,
    input: R,
    mut output: W,
) -> Result<(), RunError> {
    let gate = Arc::new(CallbackGate::new());
    let (terminal_tx, terminal_rx) = bounded(1);
    let terminal = Arc::new(TerminalSignal::new(Arc::clone(&gate), terminal_tx));
    let (outbound_tx, outbound_rx) = bounded(OUTBOUND_QUEUE_CAPACITY);
    let mut server = Server::new(
        platform,
        outbound_tx.clone(),
        Arc::clone(&gate),
        Arc::clone(&terminal),
    );
    drop(outbound_tx);
    let (input_tx, input_rx) = unbounded();
    stdin_loop(input, input_tx);
    let outcome = coordinate(&mut server, &input_rx, &terminal_rx, &terminal);
    server.shutdown();
    drop(server);
    write_messages(outbound_rx, terminal, &mut output).map_err(RunError::Writer)?;
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
    outbound: Receiver<Outbound>,
    terminal: Arc<TerminalSignal>,
) -> Result<(), FrameError> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    write_messages(outbound, terminal, &mut stdout)
}

fn write_messages<W: Write>(
    outbound: Receiver<Outbound>,
    terminal: Arc<TerminalSignal>,
    writer: &mut W,
) -> Result<(), FrameError> {
    while let Ok(message) = outbound.recv() {
        if terminal.is_triggered() {
            return Ok(());
        }
        let payload = match encode_outbound(&message) {
            Ok(payload) => payload,
            Err(OutboundEncodingError::FrameTooLarge(_)) => {
                // The complete JSON value was rejected before any stdout write.
                // Drop unexpected oversized notifications fail-open; request
                // results are replaced with a bounded error before enqueue.
                continue;
            }
            Err(OutboundEncodingError::Serialization(error)) => {
                terminal.trigger(TerminalReason::StdoutDisconnected);
                return Err(FrameError::Io(io::Error::other(error)));
            }
        };
        if let Err(error) = write_frame(writer, &payload) {
            terminal.trigger(TerminalReason::StdoutDisconnected);
            return Err(error);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keyboard::{ActivationKey, EventPhase, HelperEvent};

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
                key: ActivationKey::Z,
                phase: EventPhase::Down,
                shift: false,
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
    fn clean_writer_wait_joins_after_queued_frames_flush() {
        let gate = Arc::new(CallbackGate::new());
        let (terminal_tx, _terminal_rx) = bounded(1);
        let terminal = Arc::new(TerminalSignal::new(gate, terminal_tx));
        let (outbound_tx, outbound_rx) = bounded(1);
        outbound_tx
            .send(Outbound::Event(HelperEvent::Activation {
                key: ActivationKey::Z,
                phase: EventPhase::Down,
                shift: false,
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
                key: ActivationKey::Z,
                phase: EventPhase::Down,
                shift: false,
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
