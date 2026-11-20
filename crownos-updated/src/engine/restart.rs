use anyhow::Context;
use tokio::process::Command;
use update_manifest::RestartKind;
use update_proto::FailedStep;

use super::{AtStep, Engine, StepFailure};
use crate::process::capture;

impl Engine {
    /// A reboot or soft reboot ends this process; the next start sees the new
    /// boot and drops the marker. A session restart is the client's job, so
    /// the daemon only forgets the pending restart.
    pub(super) async fn restart(&self) -> Result<(), StepFailure> {
        let pending = self
            .published
            .with(|snapshot| snapshot.pending_restart.clone())
            .context("no restart is pending")
            .at(FailedStep::Restart)?;
        let systemctl_verb = match pending.kind {
            RestartKind::Reboot => "reboot",
            RestartKind::SoftReboot => "soft-reboot",
            RestartKind::Session => {
                self.store
                    .clear_applied()
                    .await
                    .context("cannot remove the applied marker")
                    .at(FailedStep::Restart)?;
                self.transition(|snapshot| snapshot.pending_restart = None)
                    .await;
                return Ok(());
            }
        };
        capture(Command::new(&self.paths.tools.systemctl).arg(systemctl_verb))
            .await
            .at(FailedStep::Restart)?;
        Ok(())
    }
}
