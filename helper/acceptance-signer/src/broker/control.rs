use super::*;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum ControlAction {
    Continue,
    Terminate,
    Deadline,
}

pub(super) struct Control {
    pub(super) receiver: std::sync::mpsc::Receiver<Result<ControlAction, &'static str>>,
}

impl Control {
    pub(super) fn new(correlation: String) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            loop {
                let result = read_action(&correlation).map(|action| {
                    if action == "continue" {
                        ControlAction::Continue
                    } else {
                        ControlAction::Terminate
                    }
                });
                let terminal = !matches!(result, Ok(ControlAction::Continue));
                if sender.send(result).is_err() || terminal {
                    break;
                }
            }
        });
        Self { receiver }
    }

    pub(super) fn poll(&self) -> Result<Option<ControlAction>, &'static str> {
        match self.receiver.try_recv() {
            Ok(action) => action.map(Some),
            Err(std::sync::mpsc::TryRecvError::Empty) => Ok(None),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Ok(Some(ControlAction::Terminate)),
        }
    }

    pub(super) fn wait(&self, deadline: u64) -> Result<ControlAction, &'static str> {
        let duration = Duration::from_millis(u64::from(remaining_ms(deadline)?));
        match self.receiver.recv_timeout(duration) {
            Ok(action) => action,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err("probe action deadline"),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Ok(ControlAction::Terminate),
        }
    }
}

fn read_action(correlation: &str) -> Result<String, &'static str> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Action {
        version: u8,
        correlation: String,
        action: String,
    }
    let mut frame = String::new();
    std::io::stdin()
        .read_line(&mut frame)
        .map_err(|_| "probe action")?;
    if frame.len() > 1024 || !frame.ends_with('\n') {
        return Err("probe action");
    }
    let action: Action = serde_json::from_str(&frame).map_err(|_| "probe action")?;
    if action.version != 1
        || action.correlation != correlation
        || !matches!(action.action.as_str(), "continue" | "terminate")
    {
        return Err("probe action");
    }
    Ok(action.action)
}

pub(super) fn valid_pipe_name(value: &str) -> bool {
    value.starts_with(r"\\.\pipe\TalkingQuill.")
        && value.len() <= 240
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '\\' | '.' | '-'))
}
pub(super) fn unix_ms() -> Result<u64, &'static str> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|_| "clock")?
        .as_millis()
        .try_into()
        .map_err(|_| "clock")
}
pub(super) fn remaining_ms(deadline: u64) -> Result<u32, &'static str> {
    let remaining = deadline.checked_sub(unix_ms()?).ok_or("deadline")?;
    Ok(remaining.clamp(1, u64::from(u32::MAX)) as u32)
}
