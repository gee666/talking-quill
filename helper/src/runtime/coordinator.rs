//! Input framing and terminal-first runtime coordination.

use crate::{
    framing::{FrameError, read_frame},
    gateway::{GatewayBackend, TerminalReason, TerminalSignal},
    protocol::{HandleOutcome, Server},
};
use crossbeam_channel::{Receiver, Sender};
use std::io::Read;

pub(super) enum InputMessage {
    Frame(Vec<u8>),
    Eof,
    Error(FrameError),
}

pub(super) enum CoordinatorOutcome {
    Clean,
    Framing(FrameError),
    ReaderStopped,
    Terminal(TerminalReason),
}

pub(super) fn coordinate<P: GatewayBackend>(
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

pub(super) fn outcome_after_shutdown(
    outcome: CoordinatorOutcome,
    terminal: &TerminalSignal,
) -> CoordinatorOutcome {
    match (outcome, terminal.reason()) {
        (CoordinatorOutcome::Clean, Some(reason)) => CoordinatorOutcome::Terminal(reason),
        (outcome, _) => outcome,
    }
}

pub(super) fn stdin_loop<R: Read>(mut input: R, sender: Sender<InputMessage>) {
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
