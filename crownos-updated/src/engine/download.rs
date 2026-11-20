use anyhow::Context;
use tokio::process::Command;
use update_manifest::{Channel, PackageName, chain::channel_of};
use update_proto::{DownloadStatus, FailedStep, Phase};

use super::{AtStep, Engine, StepFailure};
use crate::{pacman_conf::with_channel, process::stream, utils::atomic_write::write_atomically};

/// The pacman operations that move the system to the next release.
#[derive(Clone, Copy, Debug)]
pub(super) enum Transaction {
    DownloadOnly,
    Upgrade,
}

impl Transaction {
    fn operation(self) -> &'static str {
        match self {
            Self::DownloadOnly => "-Syuw",
            Self::Upgrade => "-Syu",
        }
    }
}

impl Engine {
    pub(super) async fn download(&self) -> Result<(), StepFailure> {
        let next = self
            .published
            .with(|snapshot| snapshot.next.clone())
            .context("no update is available")
            .at(FailedStep::Download)?;
        self.begin(Phase::Downloading).await;
        self.write_override_conf(channel_of(&next.version))
            .await
            .at(FailedStep::Download)?;
        self.run_pacman(
            Transaction::DownloadOnly,
            &next.new_packages,
            Phase::Downloading,
        )
        .await
        .at(FailedStep::Download)?;
        self.transition(|snapshot| {
            if let Some(next) = &mut snapshot.next {
                next.download = DownloadStatus::Complete;
            }
            snapshot.phase = Phase::Idle;
        })
        .await;
        Ok(())
    }

    /// A copy of `/etc/pacman.conf` with `[crownos]` on `channel`, so the
    /// installed configuration changes only after the update succeeded.
    pub(super) async fn write_override_conf(&self, channel: Channel) -> anyhow::Result<()> {
        let paths = &self.paths;
        let conf = tokio::fs::read_to_string(&paths.pacman_conf)
            .await
            .with_context(|| format!("cannot read {}", paths.pacman_conf.display()))?;
        tokio::fs::create_dir_all(&paths.runtime_directory)
            .await
            .with_context(|| format!("cannot create {}", paths.runtime_directory.display()))?;
        write_atomically(
            &paths.override_pacman_conf,
            with_channel(&conf, channel)?.as_bytes(),
        )
        .await
        .with_context(|| format!("cannot write {}", paths.override_pacman_conf.display()))
    }

    /// One pacman transaction against the override configuration. New
    /// packages join the system upgrade after `--`, so no name reads as an option.
    pub(super) async fn run_pacman(
        &self,
        transaction: Transaction,
        new_packages: &[PackageName],
        phase: Phase,
    ) -> anyhow::Result<()> {
        let mut command = Command::new(&self.paths.tools.pacman);
        command
            .args([transaction.operation(), "--noconfirm", "--config"])
            .arg(&self.paths.override_pacman_conf);
        if !new_packages.is_empty() {
            command
                .args(["--needed", "--"])
                .args(new_packages.iter().map(PackageName::as_str));
        }
        stream(&mut command, |line| self.published.line(phase, line)).await
    }
}
