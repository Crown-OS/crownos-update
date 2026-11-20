use tokio::sync::{mpsc, watch};
use update_proto::{Phase, Snapshot, updated};

use crate::utils::progress::progress_of;

/// Every snapshot travels in one frame, so it keeps only a short log.
const LOG_TAIL_LINES: usize = 24;
const MAX_LINE_BYTES: usize = 256;

/// The one copy of the state: readers borrow it through the watch channel,
/// subscribers get each change as an event.
pub struct Published {
    snapshot: watch::Sender<Snapshot>,
    events: mpsc::UnboundedSender<updated::Event>,
}

impl Published {
    pub fn new(initial: Snapshot, events: mpsc::UnboundedSender<updated::Event>) -> Self {
        Self {
            snapshot: watch::Sender::new(initial),
            events,
        }
    }

    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.snapshot.subscribe()
    }

    pub fn current(&self) -> Snapshot {
        self.snapshot.borrow().clone()
    }

    pub fn with<T>(&self, read: impl FnOnce(&Snapshot) -> T) -> T {
        read(&self.snapshot.borrow())
    }

    /// Applies `change`, announces the result and returns it.
    pub fn change(&self, change: impl FnOnce(&mut Snapshot)) -> Snapshot {
        self.snapshot.send_modify(change);
        let snapshot = self.current();
        self.emit(updated::Event::StateChanged(updated::StateChanged {
            snapshot: snapshot.clone(),
        }));
        snapshot
    }

    /// One output line of a running program: kept in the log tail and sent as
    /// `Progress`, without a full `StateChanged`.
    pub fn line(&self, phase: Phase, line: &str) {
        let line = bounded(line);
        self.snapshot.send_modify(|snapshot| {
            if snapshot.log_tail.len() >= LOG_TAIL_LINES {
                snapshot.log_tail.remove(0);
            }
            snapshot.log_tail.push(line.to_owned());
        });
        self.emit(updated::Event::Progress(updated::Progress {
            phase,
            fraction: progress_of(line),
            line: line.to_owned(),
        }));
    }

    fn emit(&self, event: updated::Event) {
        if self.events.send(event).is_err() {
            tracing::debug!("the IPC server is gone; event dropped");
        }
    }
}

fn bounded(line: &str) -> &str {
    &line[..line.floor_char_boundary(MAX_LINE_BYTES)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_log_tail_keeps_the_latest_bounded_lines() {
        let (events, mut received) = mpsc::unbounded_channel();
        let published = Published::new(Snapshot::default(), events);
        for index in 0..LOG_TAIL_LINES + 3 {
            published.line(Phase::Downloading, &format!("line {index} 50%"));
        }
        published.line(Phase::Downloading, &"é".repeat(MAX_LINE_BYTES));
        let tail = published.current().log_tail;
        assert_eq!(tail.len(), LOG_TAIL_LINES);
        assert_eq!(tail[0], "line 4 50%");
        assert!(tail[LOG_TAIL_LINES - 1].len() <= MAX_LINE_BYTES);
        assert!(matches!(
            received.try_recv(),
            Ok(updated::Event::Progress(updated::Progress {
                fraction: Some(0.5),
                ..
            }))
        ));
    }

    #[test]
    fn a_change_is_announced_with_the_new_state() {
        let (events, mut received) = mpsc::unbounded_channel();
        let published = Published::new(Snapshot::default(), events);
        let changed = published.change(|snapshot| snapshot.phase = Phase::Checking);
        assert_eq!(changed.phase, Phase::Checking);
        assert!(matches!(
            received.try_recv(),
            Ok(updated::Event::StateChanged(updated::StateChanged { snapshot })) if snapshot == changed
        ));
    }
}
