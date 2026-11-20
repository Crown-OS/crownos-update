//! A new process restores what the last one left: the persisted state, a
//! pending restart on this boot, and an update that stopped halfway.

use std::sync::Arc;

use tokio::sync::mpsc;
use update_proto::{
    ApplyStep, DownloadStatus, FailedStep, Failure, NextRelease, Snapshot, updated,
};

use super::{
    Engine,
    persisted::{AppliedMarker, InProgressUpdate, Store},
    state::Published,
};
use crate::{paths::Paths, utils::boot_marker::BootMarker};

const STOPPED_DURING_UPDATE: &str = "The update service stopped during the update.";

impl Engine {
    pub async fn start(paths: Arc<Paths>, events: mpsc::UnboundedSender<updated::Event>) -> Self {
        let store = Store::new(&paths);
        let persisted = store.load_state().await;
        let notes = match persisted.next {
            Some(_) => store.load_notes().await,
            None => None,
        };
        let applied = store.load_applied().await;
        let mut unfinished = store.load_in_progress().await;
        if unfinished
            .as_ref()
            .is_some_and(|unfinished| is_already_recorded(unfinished, applied.as_ref()))
        {
            if let Err(error) = store.clear_in_progress().await {
                tracing::warn!("cannot remove the finished in-progress record: {error}");
            }
            unfinished = None;
        }
        let mut snapshot = Snapshot {
            last_check: persisted.last_check,
            next: persisted.next.map(|next| next.with_notes(notes)),
            in_progress: unfinished.as_ref().map(InProgressUpdate::summary),
            failure: persisted.failure,
            ..Snapshot::default()
        };
        if let Some(marker) = applied {
            if BootMarker::current(&paths)
                .await
                .restarted_since(&marker.boot)
            {
                if let Err(error) = store.clear_applied().await {
                    tracing::warn!("cannot remove the applied marker: {error}");
                }
            } else {
                snapshot.pending_restart = Some(marker.pending_restart());
            }
        }
        let engine = Self {
            store,
            published: Published::new(snapshot, events),
            paths,
        };
        engine.refresh_installed().await;
        if let Some(unfinished) = unfinished {
            let notes = engine.store.load_notes().await;
            engine
                .transition(|snapshot| resume_unfinished(snapshot, &unfinished, notes))
                .await;
        }
        engine
    }
}

/// The daemon stopped after it saved the applied marker and before it
/// removed the record: the update is complete.
fn is_already_recorded(unfinished: &InProgressUpdate, applied: Option<&AppliedMarker>) -> bool {
    unfinished.completed == ApplyStep::PostUpdate
        && applied.is_some_and(|marker| marker.version == unfinished.to)
}

/// The next release is the target of the unfinished update, already
/// downloaded. Without a recorded failure, the failure names the step to
/// continue with: the one after the last completed step when the new release
/// is installed, the upgrade when the old one still is.
fn resume_unfinished(
    snapshot: &mut Snapshot,
    unfinished: &InProgressUpdate,
    notes: Option<String>,
) {
    match &mut snapshot.next {
        Some(next) if next.version == unfinished.to => next.download = DownloadStatus::Complete,
        _ => {
            snapshot.next = Some(NextRelease {
                version: unfinished.to.clone(),
                restart: unfinished.restart,
                notes,
                new_packages: unfinished.new_packages.clone(),
                download: DownloadStatus::Complete,
            });
        }
    }
    if snapshot.failure.is_some() {
        return;
    }
    let installed = snapshot.installed.as_ref();
    let continue_with = if installed == Some(&unfinished.to) {
        unfinished
            .completed
            .following()
            .unwrap_or(ApplyStep::RecordApplied)
    } else if installed == Some(&unfinished.from) {
        ApplyStep::Install
    } else {
        return;
    };
    snapshot.failure = Some(Failure {
        step: FailedStep::Apply(continue_with),
        message: STOPPED_DURING_UPDATE.to_owned(),
        rollback: Some(unfinished.rollback.clone()),
    });
}

#[cfg(test)]
mod tests {
    use update_manifest::RestartKind;
    use update_proto::{Rollback, SnapperNumber};

    use super::*;

    fn unfinished(completed: ApplyStep) -> Result<InProgressUpdate, Box<dyn std::error::Error>> {
        Ok(InProgressUpdate {
            from: "0.4.0".parse()?,
            to: "0.4.1".parse()?,
            restart: RestartKind::Reboot,
            new_packages: Vec::new(),
            rollback: Rollback {
                snapshot: SnapperNumber(7),
                from: "0.4.0".parse()?,
            },
            completed,
        })
    }

    fn installed(version: &str) -> Result<Snapshot, Box<dyn std::error::Error>> {
        Ok(Snapshot {
            installed: Some(version.parse()?),
            ..Snapshot::default()
        })
    }

    #[test]
    fn the_new_release_continues_after_the_completed_step() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut snapshot = installed("0.4.1")?;
        resume_unfinished(&mut snapshot, &unfinished(ApplyStep::Install)?, None);
        let failure = snapshot.failure.ok_or("no failure")?;
        assert_eq!(failure.step, FailedStep::Apply(ApplyStep::SwitchChannel));
        assert_eq!(
            failure.rollback.map(|rollback| rollback.snapshot),
            Some(SnapperNumber(7))
        );
        let next = snapshot.next.ok_or("no next release")?;
        assert_eq!(next.download, DownloadStatus::Complete);
        assert_eq!(next.version.to_string(), "0.4.1");
        Ok(())
    }

    #[test]
    fn the_old_release_continues_with_the_upgrade() -> Result<(), Box<dyn std::error::Error>> {
        let mut snapshot = installed("0.4.0")?;
        resume_unfinished(&mut snapshot, &unfinished(ApplyStep::KeepKernel)?, None);
        let step = snapshot.failure.map(|failure| failure.step);
        assert_eq!(step, Some(FailedStep::Apply(ApplyStep::Install)));
        Ok(())
    }

    #[test]
    fn a_saved_applied_marker_finishes_the_record() -> Result<(), Box<dyn std::error::Error>> {
        let marker = AppliedMarker {
            version: "0.4.1".parse()?,
            from: "0.4.0".parse()?,
            restart: RestartKind::Reboot,
            boot: BootMarker::default(),
        };
        assert!(is_already_recorded(
            &unfinished(ApplyStep::PostUpdate)?,
            Some(&marker)
        ));
        assert!(!is_already_recorded(
            &unfinished(ApplyStep::PostUpdate)?,
            None
        ));
        assert!(!is_already_recorded(
            &unfinished(ApplyStep::Install)?,
            Some(&marker)
        ));
        let older = AppliedMarker {
            version: "0.4.0".parse()?,
            ..marker
        };
        assert!(!is_already_recorded(
            &unfinished(ApplyStep::PostUpdate)?,
            Some(&older)
        ));
        Ok(())
    }

    #[test]
    fn a_recorded_failure_is_kept() -> Result<(), Box<dyn std::error::Error>> {
        let mut snapshot = installed("0.4.0")?;
        snapshot.failure = Some(Failure {
            step: FailedStep::Apply(ApplyStep::Install),
            message: "error: failed to commit transaction".into(),
            rollback: None,
        });
        resume_unfinished(&mut snapshot, &unfinished(ApplyStep::KeepKernel)?, None);
        let message = snapshot.failure.map(|failure| failure.message);
        assert_eq!(
            message.as_deref(),
            Some("error: failed to commit transaction")
        );
        Ok(())
    }
}
