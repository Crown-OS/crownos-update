//! One task owns the update state and runs one step at a time. The IPC
//! handler only reads the published state and queues verbs.

mod apply;
mod check;
mod download;
mod persisted;
mod restart;
mod startup;
mod state;

use std::{sync::Arc, time::Duration};

use tokio::{
    sync::{mpsc, watch},
    time::{Instant, MissedTickBehavior, interval_at},
};
use update_proto::{FailedStep, Failure, Phase, Rollback, Snapshot, Verb};

use crate::paths::Paths;
use persisted::Store;
use state::Published;

const FIRST_CHECK_AFTER_START: Duration = Duration::from_secs(15 * 60);
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_FAILURE_MESSAGE_BYTES: usize = 2 * 1024;

pub struct Engine {
    paths: Arc<Paths>,
    store: Store,
    published: Published,
}

/// Who asked for a verb. A failed check that nobody asked for is only logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Trigger {
    Caller,
    Schedule,
}

/// A step that failed, with the way back when the system already changed.
struct StepFailure {
    step: FailedStep,
    error: anyhow::Error,
    rollback: Option<Rollback>,
}

trait AtStep<T> {
    fn at(self, step: FailedStep) -> Result<T, StepFailure>;
    fn recoverable_by(self, step: FailedStep, rollback: &Rollback) -> Result<T, StepFailure>;
}

impl<T> AtStep<T> for anyhow::Result<T> {
    fn at(self, step: FailedStep) -> Result<T, StepFailure> {
        self.map_err(|error| StepFailure {
            step,
            error,
            rollback: None,
        })
    }

    fn recoverable_by(self, step: FailedStep, rollback: &Rollback) -> Result<T, StepFailure> {
        self.map_err(|error| StepFailure {
            step,
            error,
            rollback: Some(rollback.clone()),
        })
    }
}

impl Engine {
    pub fn subscribe(&self) -> watch::Receiver<Snapshot> {
        self.published.subscribe()
    }

    /// Runs verbs until every sender is gone. The verb that runs when the
    /// senders go away finishes first; verbs still queued then are dropped.
    pub async fn run(self, mut verbs: mpsc::UnboundedReceiver<Verb>) {
        let mut schedule = interval_at(Instant::now() + FIRST_CHECK_AFTER_START, CHECK_INTERVAL);
        schedule.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            let (verb, trigger) = tokio::select! {
                verb = verbs.recv() => match verb {
                    Some(verb) => (verb, Trigger::Caller),
                    None => return,
                },
                _ = schedule.tick() => (Verb::Check, Trigger::Schedule),
            };
            if verbs.is_closed() {
                return;
            }
            self.handle(verb, trigger, &verbs).await;
            if verbs.is_closed() {
                return;
            }
        }
    }

    async fn handle(&self, verb: Verb, trigger: Trigger, verbs: &mpsc::UnboundedReceiver<Verb>) {
        if let Some(reason) = self.published.with(|snapshot| snapshot.refusal(verb)) {
            tracing::debug!("skipping {verb:?}: {reason}");
            return;
        }
        let shown_failure = self.published.with(|snapshot| snapshot.failure.clone());
        let outcome = match verb {
            Verb::Check => self.check_and_download(verbs).await,
            Verb::Download => self.download().await,
            Verb::Apply => self.apply().await,
            Verb::Restart => self.restart().await,
        };
        let Err(failure) = outcome else {
            return;
        };
        let message = bounded_message(format!("{:#}", failure.error));
        tracing::warn!("{:?} failed: {message}", failure.step);
        if is_only_logged(trigger, failure.step) {
            self.published.line(
                Phase::Checking,
                &format!("The scheduled check failed: {message}"),
            );
            self.transition(|snapshot| {
                snapshot.phase = Phase::Idle;
                snapshot.failure = shown_failure;
            })
            .await;
            return;
        }
        self.transition(|snapshot| {
            snapshot.phase = Phase::Idle;
            snapshot.failure = Some(Failure {
                step: failure.step,
                message,
                rollback: failure.rollback,
            });
        })
        .await;
    }

    async fn check_and_download(
        &self,
        verbs: &mpsc::UnboundedReceiver<Verb>,
    ) -> Result<(), StepFailure> {
        self.check().await?;
        let download_fits = self
            .published
            .with(|snapshot| snapshot.refusal(Verb::Download).is_none());
        if download_fits && !verbs.is_closed() {
            self.download().await?;
        }
        Ok(())
    }

    /// Starts a step: clears the last failure and the log.
    async fn begin(&self, phase: Phase) {
        self.transition(|snapshot| {
            snapshot.phase = phase;
            snapshot.failure = None;
            snapshot.log_tail.clear();
        })
        .await;
    }

    async fn transition(&self, change: impl FnOnce(&mut Snapshot)) {
        let snapshot = self.published.change(change);
        if let Err(error) = self.store.save_state(&snapshot).await {
            tracing::warn!("cannot save the update state: {error}");
        }
    }

    /// Records the installed release; a system without one is a failure.
    /// The next release stays while an unfinished update targets it.
    async fn refresh_installed(&self) -> check::DetectedRelease {
        let detected = check::detect_installed(&self.paths).await;
        self.transition(|snapshot| {
            snapshot.installed = detected.version.clone();
            snapshot.channel = detected.channel;
            let detection_failed = matches!(
                &snapshot.failure,
                Some(Failure {
                    step: FailedStep::DetectRelease,
                    ..
                })
            );
            match detected.require() {
                Err(error) => {
                    snapshot.failure = Some(Failure {
                        step: FailedStep::DetectRelease,
                        message: format!("{error:#}"),
                        rollback: None,
                    });
                }
                Ok(_) if detection_failed => snapshot.failure = None,
                Ok(_) => {}
            }
            let next_is_installed = matches!(
                (&snapshot.next, &snapshot.installed),
                (Some(next), Some(installed)) if next.version <= *installed
            );
            if next_is_installed && snapshot.in_progress.is_none() {
                snapshot.next = None;
            }
        })
        .await;
        if self.published.with(|snapshot| snapshot.next.is_none())
            && let Err(error) = self.store.clear_next().await
        {
            tracing::warn!("cannot remove the stale next release: {error}");
        }
        detected
    }
}

/// Being offline at a scheduled check is not a failure to show; the
/// failure shown before the check stays.
fn is_only_logged(trigger: Trigger, step: FailedStep) -> bool {
    trigger == Trigger::Schedule && step == FailedStep::Check
}

fn bounded_message(mut message: String) -> String {
    message.truncate(message.floor_char_boundary(MAX_FAILURE_MESSAGE_BYTES));
    message
}

#[cfg(test)]
mod tests {
    use update_proto::ApplyStep;

    use super::*;

    /// An installed 0.4.0 whose repository does not exist, so every check fails.
    async fn offline_engine(root: &std::path::Path) -> Result<Engine, Box<dyn std::error::Error>> {
        std::fs::create_dir_all(root.join("etc"))?;
        std::fs::write(root.join("etc/os-release"), "VERSION_ID=0.4.0\n")?;
        std::fs::write(
            root.join("etc/pacman.conf"),
            "[crownos]\nServer = https://repo.crownos.org/v0.4.0/$arch\n",
        )?;
        let paths = Paths::new(
            root,
            &root.join("tools"),
            format!("file://{}", root.join("repo").display()),
        );
        let (events, _outbox) = mpsc::unbounded_channel();
        Ok(Engine::start(Arc::new(paths), events).await)
    }

    #[tokio::test]
    async fn a_failed_scheduled_check_keeps_the_shown_failure()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let engine = offline_engine(directory.path()).await?;
        let shown = Failure {
            step: FailedStep::Apply(ApplyStep::KeepKernel),
            message: "no kernel on the ESP".into(),
            rollback: None,
        };
        engine
            .transition(|snapshot| snapshot.failure = Some(shown.clone()))
            .await;
        let (_sender, verbs) = mpsc::unbounded_channel();

        engine.handle(Verb::Check, Trigger::Schedule, &verbs).await;
        let snapshot = engine.published.current();
        assert_eq!(snapshot.failure, Some(shown));
        assert!(snapshot.is_idle());
        assert_eq!(snapshot.last_check, None);

        engine.handle(Verb::Check, Trigger::Caller, &verbs).await;
        let step = engine
            .published
            .with(|snapshot| snapshot.failure.as_ref().map(|failure| failure.step));
        assert_eq!(step, Some(FailedStep::Check));
        Ok(())
    }

    #[tokio::test]
    async fn verbs_queued_after_the_senders_are_gone_do_not_run()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let engine = offline_engine(directory.path()).await?;
        let published = engine.subscribe();
        let (sender, verbs) = mpsc::unbounded_channel();
        sender.send(Verb::Check)?;
        drop(sender);
        engine.run(verbs).await;
        assert_eq!(published.borrow().failure, None);
        Ok(())
    }

    #[test]
    fn only_a_scheduled_check_fails_quietly() {
        assert!(is_only_logged(Trigger::Schedule, FailedStep::Check));
        assert!(!is_only_logged(Trigger::Caller, FailedStep::Check));
        assert!(!is_only_logged(Trigger::Schedule, FailedStep::Download));
        assert!(!is_only_logged(
            Trigger::Schedule,
            FailedStep::DetectRelease
        ));
    }

    #[test]
    fn failure_messages_end_at_a_character() {
        let message = bounded_message("é".repeat(MAX_FAILURE_MESSAGE_BYTES));
        assert!(message.len() <= MAX_FAILURE_MESSAGE_BYTES);
        assert!(message.chars().all(|character| character == 'é'));
        assert_eq!(bounded_message("short".into()), "short");
    }
}
