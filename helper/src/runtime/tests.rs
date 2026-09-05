use super::writer::{wait_for_writer, write_messages, write_prioritized_messages};
use crate::{
    CriticalDelivery,
    framing::{FrameError, read_frame},
    gateway::{ActivationCaptureGate, CallbackGate, TerminalReason, TerminalSignal},
    protocol::Outbound,
};
use crate::{
    gateway::GATEWAY_POLICY_MARKER,
    protocol::{RequestId, RpcResponse},
};
use crossbeam_channel::{Receiver, Sender, bounded};
use std::{
    io::{self, Write},
    sync::Arc,
    thread,
    time::Duration,
};
use talking_quill_keyboard_core::{
    ActivationBinding, ActivationContext, ActivationGeneration, ActivationKey, EventPhase,
    KeyboardEvent, ProfileId, Shortcut,
};

#[test]
fn production_gateway_carries_the_permanent_no_suppression_marker() {
    assert_eq!(
        std::hint::black_box(GATEWAY_POLICY_MARKER),
        "TALKING_QUILL_KEYBOARD_GATEWAY=PROTOCOL_V1_GATEWAY_CANNOT_SUPPRESS"
    );
    let forwarding_gate = ActivationCaptureGate::for_process();
    assert!(forwarding_gate.is_open());
    assert!(!forwarding_gate.development_disabled());
}

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
        .send(Outbound::Event(KeyboardEvent::Activation {
            binding: ActivationBinding::new(
                ProfileId::GENERAL,
                Shortcut::legacy_alt_letter(ActivationKey::Z, false),
            ),
            context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
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
        .send(Outbound::Event(KeyboardEvent::Activation {
            binding: ActivationBinding::new(
                ProfileId::GENERAL,
                Shortcut::legacy_alt_letter(ActivationKey::A, false),
            ),
            context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
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
    drop(completion_tx);
    terminal.trigger(TerminalReason::OutboundQueueUnavailable);
    let (final_tx, final_rx) = bounded(1);
    drop(final_tx);
    drop(critical_tx);
    drop(outbound_tx);

    let mut output = Vec::new();
    write_prioritized_messages(final_rx, critical_rx, outbound_rx, terminal, &mut output).unwrap();
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
        .send(Outbound::Event(KeyboardEvent::Activation {
            binding: ActivationBinding::new(
                ProfileId::GENERAL,
                Shortcut::legacy_alt_letter(ActivationKey::Z, false),
            ),
            context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
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
        .send(Outbound::Event(KeyboardEvent::Activation {
            binding: ActivationBinding::new(
                ProfileId::GENERAL,
                Shortcut::legacy_alt_letter(ActivationKey::Z, false),
            ),
            context: ActivationContext::target_unavailable(ActivationGeneration::FIRST),
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
