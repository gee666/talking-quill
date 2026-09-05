//! Prioritized framed output, critical batches, and bounded writer joins.

use crate::{
    CriticalDelivery, RunError,
    framing::{FrameError, write_frame},
    gateway::{TerminalReason, TerminalSignal},
    protocol::{Outbound, OutboundEncodingError, encode_outbound},
};
use crossbeam_channel::Receiver;
#[cfg(test)]
use crossbeam_channel::bounded;
use std::{
    io::{self, Write},
    sync::Arc,
    thread::JoinHandle,
    time::Duration,
};

pub(super) fn wait_for_writer(
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

pub(super) fn writer_loop(
    final_response: Receiver<Outbound>,
    critical: Receiver<CriticalDelivery>,
    outbound: Receiver<Outbound>,
    terminal: Arc<TerminalSignal>,
) -> Result<(), FrameError> {
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    write_prioritized_messages(final_response, critical, outbound, terminal, &mut stdout)
}

#[cfg(test)]
pub(super) fn write_messages<W: Write>(
    outbound: Receiver<Outbound>,
    terminal: Arc<TerminalSignal>,
    writer: &mut W,
) -> Result<(), FrameError> {
    let (final_tx, final_rx) = bounded(1);
    let (critical_tx, critical_rx) = bounded(1);
    drop(final_tx);
    drop(critical_tx);
    write_prioritized_messages(final_rx, critical_rx, outbound, terminal, writer)
}

pub(super) fn write_prioritized_messages<W: Write>(
    final_response: Receiver<Outbound>,
    critical: Receiver<CriticalDelivery>,
    outbound: Receiver<Outbound>,
    terminal: Arc<TerminalSignal>,
    writer: &mut W,
) -> Result<(), FrameError> {
    let never_final = crossbeam_channel::never();
    let never_critical = crossbeam_channel::never();
    let never_outbound = crossbeam_channel::never();
    let mut final_open = true;
    let mut critical_open = true;
    let mut outbound_open = true;
    while final_open || critical_open || outbound_open {
        // Terminal failures stop ordinary/final output, but accepted critical
        // paste deliveries remain an obligation until their sender closes.
        if terminal.is_triggered() {
            if terminal.reason() == Some(TerminalReason::StdoutDisconnected) {
                return Ok(());
            }
            final_open = false;
            outbound_open = false;
        }

        if final_open {
            match final_response.try_recv() {
                Ok(response) => {
                    return drain_before_final_response(
                        response, &critical, &outbound, &terminal, writer,
                    );
                }
                Err(crossbeam_channel::TryRecvError::Disconnected) => final_open = false,
                Err(crossbeam_channel::TryRecvError::Empty) => {}
            }
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

        if !final_open && !critical_open && !outbound_open {
            break;
        }

        let selected_final = if final_open {
            &final_response
        } else {
            &never_final
        };
        let selected_critical = if critical_open {
            &critical
        } else {
            &never_critical
        };
        let selected_outbound = if outbound_open {
            &outbound
        } else {
            &never_outbound
        };
        let item = crossbeam_channel::select_biased! {
            recv(selected_final) -> response => match response {
                Ok(response) => Some(EitherOutbound::Final(response)),
                Err(_) => {
                    final_open = false;
                    None
                }
            },
            recv(selected_critical) -> delivery => match delivery {
                Ok(delivery) => Some(EitherOutbound::Critical(delivery)),
                Err(_) => {
                    critical_open = false;
                    None
                }
            },
            recv(selected_outbound) -> message => match message {
                Ok(message) => Some(EitherOutbound::Ordinary(message)),
                Err(_) => {
                    outbound_open = false;
                    None
                }
            },
        };
        match item {
            Some(EitherOutbound::Final(response)) => {
                return drain_before_final_response(
                    response, &critical, &outbound, &terminal, writer,
                );
            }
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
    Final(Outbound),
    Critical(CriticalDelivery),
    Ordinary(Outbound),
}

fn drain_before_final_response<W: Write>(
    final_response: Outbound,
    critical: &Receiver<CriticalDelivery>,
    outbound: &Receiver<Outbound>,
    terminal: &TerminalSignal,
    writer: &mut W,
) -> Result<(), FrameError> {
    // Shutdown closes callback admission and quiesces all accepted producers
    // before publishing this reserved delivery. Everything already accepted by
    // either normal path is therefore a finite prefix which must precede the
    // one final response.
    while let Ok(delivery) = critical.try_recv() {
        write_critical_delivery(delivery, terminal, writer)?;
    }
    while let Ok(message) = outbound.try_recv() {
        write_message(message, terminal, writer)?;
    }
    write_message(final_response, terminal, writer)
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
