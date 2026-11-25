use tokio::process::Command;
use update_manifest::PackageName;
use update_proto::{ApplyStep, Phase};

use super::Engine;
use crate::{process::stream, utils::installed_packages::installed_packages};

impl Engine {
    /// Removes the packages that the release retired, with the dependencies
    /// that only they needed. A retired package that is not installed is
    /// skipped, so a second run does nothing.
    pub(super) async fn remove_retired_packages(
        &self,
        retired: &[PackageName],
    ) -> anyhow::Result<()> {
        if retired.is_empty() {
            return Ok(());
        }
        let installed = installed_packages(&self.paths.tools.pacman).await?;
        let installed_retired: Vec<&str> = retired
            .iter()
            .map(PackageName::as_str)
            .filter(|package| installed.contains(*package))
            .collect();
        if installed_retired.is_empty() {
            return Ok(());
        }
        let mut command = Command::new(&self.paths.tools.pacman);
        command
            .args(["-Rs", "--noconfirm", "--"])
            .args(&installed_retired);
        stream(&mut command, |line| {
            self.published
                .line(Phase::Applying(ApplyStep::PostUpdate), line)
        })
        .await
    }
}
