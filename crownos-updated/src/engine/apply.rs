use std::os::unix::fs::PermissionsExt;

use anyhow::Context;
use tokio::process::Command;
use update_manifest::{ReleaseVersion, chain::channel_of};
use update_proto::{ApplyStep, FailedStep, Failure, NextRelease, Phase, Rollback};

use super::{
    AtStep, Engine, StepFailure,
    download::Transaction,
    persisted::{AppliedMarker, InProgressUpdate},
};
use crate::{
    pacman_conf::with_channel,
    process::stream,
    rollback::{
        create_rollback_snapshot, delete_snapshot, keep_kernel, prune_rollback_snapshots,
        remove_rollback_entry, rollback_snapshot_exists, write_rollback_entry,
    },
    utils::{atomic_write::write_atomically, boot_marker::BootMarker},
};

const EXECUTABLE_BITS: u32 = 0o111;

fn failed_at(step: ApplyStep) -> FailedStep {
    FailedStep::Apply(step)
}

fn first_step_of(update: &InProgressUpdate) -> ApplyStep {
    update
        .completed
        .following()
        .unwrap_or(ApplyStep::RecordApplied)
}

impl Engine {
    /// A fresh update makes the snapshot, keeps the kernel and writes the
    /// rollback entry; nothing before the pacman upgrade touches the installed
    /// system. An update that already has its rollback continues where it
    /// stopped and never makes a second snapshot.
    pub(super) async fn apply(&self) -> Result<(), StepFailure> {
        let (next, installed, failure) = self.published.with(|snapshot| {
            (
                snapshot.next.clone(),
                snapshot.installed.clone(),
                snapshot.failure.clone(),
            )
        });
        let next = next
            .context("no update is available")
            .at(failed_at(ApplyStep::Snapshot))?;
        let update = match self
            .resumable_update(&next, installed.as_ref(), failure.as_ref())
            .await
        {
            Some(update) => {
                self.begin(Phase::Applying(first_step_of(&update))).await;
                update
            }
            None => {
                let from = installed
                    .context("cannot tell which CrownOS release is installed")
                    .at(failed_at(ApplyStep::Snapshot))?;
                self.begin(Phase::Applying(ApplyStep::Snapshot)).await;
                let rollback = self.prepare_rollback(&from, &next.version).await?;
                InProgressUpdate {
                    from,
                    to: next.version.clone(),
                    restart: next.restart,
                    new_packages: next.new_packages.clone(),
                    rollback,
                    completed: ApplyStep::KeepKernel,
                }
            }
        };
        self.continue_update(update).await
    }

    /// The recorded unfinished update to `next`, else the rollback of a
    /// failed earlier attempt from the installed release whose snapshot still
    /// exists. The boot entry is written again for that snapshot, so the
    /// failure hint always names an entry that exists.
    async fn resumable_update(
        &self,
        next: &NextRelease,
        installed: Option<&ReleaseVersion>,
        failure: Option<&Failure>,
    ) -> Option<InProgressUpdate> {
        if let Some(update) = self.store.load_in_progress().await
            && update.to == next.version
        {
            return Some(update);
        }
        let rollback = failure?.rollback.clone()?;
        if Some(&rollback.from) != installed {
            return None;
        }
        match rollback_snapshot_exists(&self.paths, rollback.snapshot).await {
            Ok(true) => {}
            Ok(false) => return None,
            Err(error) => {
                tracing::warn!("cannot list the rollback snapshots: {error:#}");
                return None;
            }
        }
        if let Err(error) =
            write_rollback_entry(&self.paths, &rollback.from, rollback.snapshot).await
        {
            tracing::warn!("cannot write the rollback entry again: {error:#}");
            return None;
        }
        Some(InProgressUpdate {
            from: rollback.from.clone(),
            to: next.version.clone(),
            restart: next.restart,
            new_packages: next.new_packages.clone(),
            rollback,
            completed: ApplyStep::KeepKernel,
        })
    }

    /// Snapshot, kernel pair and rEFInd entry. A failure after the snapshot
    /// deletes it again, so no rollback without a boot entry is left behind.
    async fn prepare_rollback(
        &self,
        from: &ReleaseVersion,
        to: &ReleaseVersion,
    ) -> Result<Rollback, StepFailure> {
        let number = create_rollback_snapshot(&self.paths, from, to)
            .await
            .at(failed_at(ApplyStep::Snapshot))?;
        self.enter(ApplyStep::KeepKernel).await;
        let prepared = match keep_kernel(&self.paths).await {
            Ok(()) => write_rollback_entry(&self.paths, from, number).await,
            Err(error) => Err(error),
        };
        if let Err(error) = prepared {
            if let Err(cleanup) = remove_rollback_entry(&self.paths).await {
                tracing::warn!("cannot remove the rollback entry: {cleanup:#}");
            }
            if let Err(cleanup) = delete_snapshot(&self.paths, number).await {
                tracing::warn!("cannot delete snapshot {number}: {cleanup:#}");
            }
            return Err(error.context(
                "the rollback entry in the boot menu is unavailable until the next update",
            ))
            .at(failed_at(ApplyStep::KeepKernel));
        }
        if let Err(error) = prune_rollback_snapshots(&self.paths, number).await {
            tracing::warn!("cannot delete older rollback snapshots: {error:#}");
        }
        Ok(Rollback {
            snapshot: number,
            from: from.clone(),
        })
    }

    /// Runs every step after `update.completed` and records each one, so a
    /// restart of the daemon continues at the right step.
    async fn continue_update(&self, mut update: InProgressUpdate) -> Result<(), StepFailure> {
        let start = first_step_of(&update);
        let rollback = update.rollback.clone();
        self.record_progress(&update)
            .await
            .recoverable_by(failed_at(start), &rollback)?;
        if start <= ApplyStep::Install {
            self.enter(ApplyStep::Install).await;
            self.install(&update)
                .await
                .recoverable_by(failed_at(ApplyStep::Install), &rollback)?;
            update.completed = ApplyStep::Install;
            self.record_progress(&update)
                .await
                .recoverable_by(failed_at(ApplyStep::SwitchChannel), &rollback)?;
        }
        if start <= ApplyStep::SwitchChannel {
            self.enter(ApplyStep::SwitchChannel).await;
            self.switch_channel(&update.to)
                .await
                .recoverable_by(failed_at(ApplyStep::SwitchChannel), &rollback)?;
            update.completed = ApplyStep::SwitchChannel;
            self.record_progress(&update)
                .await
                .recoverable_by(failed_at(ApplyStep::PostUpdate), &rollback)?;
        }
        if start <= ApplyStep::PostUpdate {
            self.enter(ApplyStep::PostUpdate).await;
            self.run_post_update(&update.from, &update.to)
                .await
                .recoverable_by(failed_at(ApplyStep::PostUpdate), &rollback)?;
            update.completed = ApplyStep::PostUpdate;
            self.record_progress(&update)
                .await
                .recoverable_by(failed_at(ApplyStep::RecordApplied), &rollback)?;
        }
        self.enter(ApplyStep::RecordApplied).await;
        self.finish(update)
            .await
            .recoverable_by(failed_at(ApplyStep::RecordApplied), &rollback)
    }

    async fn enter(&self, step: ApplyStep) {
        self.transition(|snapshot| snapshot.phase = Phase::Applying(step))
            .await;
    }

    async fn record_progress(&self, update: &InProgressUpdate) -> anyhow::Result<()> {
        self.store
            .save_in_progress(update)
            .await
            .context("cannot record the progress of the update")?;
        self.transition(|snapshot| snapshot.in_progress = Some(update.summary()))
            .await;
        Ok(())
    }

    async fn install(&self, update: &InProgressUpdate) -> anyhow::Result<()> {
        self.write_override_conf(channel_of(&update.to)).await?;
        self.run_pacman(
            Transaction::Upgrade,
            &update.new_packages,
            Phase::Applying(ApplyStep::Install),
        )
        .await
    }

    async fn switch_channel(&self, to: &ReleaseVersion) -> anyhow::Result<()> {
        let path = &self.paths.pacman_conf;
        let conf = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("cannot read {}", path.display()))?;
        write_atomically(path, with_channel(&conf, channel_of(to))?.as_bytes())
            .await
            .with_context(|| format!("cannot write {}", path.display()))
    }

    async fn run_post_update(
        &self,
        from: &ReleaseVersion,
        to: &ReleaseVersion,
    ) -> anyhow::Result<()> {
        let script = &self.paths.post_update;
        let executable = tokio::fs::metadata(script).await.is_ok_and(|metadata| {
            metadata.is_file() && metadata.permissions().mode() & EXECUTABLE_BITS != 0
        });
        if !executable {
            return Ok(());
        }
        let mut command = Command::new(script);
        command
            .env("CROWNOS_FROM_VERSION", from.to_string())
            .env("CROWNOS_TO_VERSION", to.to_string());
        stream(&mut command, |line| {
            self.published
                .line(Phase::Applying(ApplyStep::PostUpdate), line)
        })
        .await
    }

    async fn finish(&self, update: InProgressUpdate) -> anyhow::Result<()> {
        let marker = AppliedMarker {
            version: update.to,
            from: update.from,
            restart: update.restart,
            boot: BootMarker::current(&self.paths).await,
        };
        self.store
            .save_applied(&marker)
            .await
            .context("cannot record the applied update")?;
        if let Err(error) = self.store.clear_in_progress().await {
            tracing::warn!("cannot remove the in-progress record: {error}");
        }
        if let Err(error) = self.store.clear_next().await {
            tracing::warn!("cannot remove the applied release files: {error}");
        }
        let pending = marker.pending_restart();
        self.transition(|snapshot| {
            snapshot.channel = Some(channel_of(&pending.version));
            snapshot.installed = Some(pending.version.clone());
            snapshot.next = None;
            snapshot.in_progress = None;
            snapshot.pending_restart = Some(pending);
            snapshot.phase = Phase::Idle;
        })
        .await;
        Ok(())
    }
}
