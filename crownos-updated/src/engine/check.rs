use anyhow::{Context, anyhow};
use tokio::task::spawn_blocking;
use update_manifest::{
    Channel, PackageName, ReleaseManifest, ReleaseVersion, VerifiedManifest,
    chain::{channel_of, index_channel, installed_channel, next_release},
    fetch_notes, fetch_verified, installed_version,
};
use update_proto::{DownloadStatus, FailedStep, NextRelease, Phase, UnixSeconds};

use super::{AtStep, Engine, StepFailure};
use crate::{paths::Paths, utils::installed_packages::installed_packages};

/// Notes travel inside every snapshot, which must fit one IPC frame.
const MAX_NOTES_BYTES: usize = 32 * 1024;

pub struct DetectedRelease {
    pub version: Option<ReleaseVersion>,
    pub channel: Option<Channel>,
}

struct FoundRelease {
    verified: VerifiedManifest,
    notes: Option<String>,
}

impl DetectedRelease {
    pub fn require(&self) -> anyhow::Result<(&ReleaseVersion, Channel)> {
        let version = self
            .version
            .as_ref()
            .context("cannot tell which CrownOS release is installed")?;
        let channel = self
            .channel
            .context("/etc/pacman.conf does not pin [crownos] to a release channel")?;
        Ok((version, channel))
    }
}

pub async fn detect_installed(paths: &Paths) -> DetectedRelease {
    let (os_release, pacman) = (paths.os_release.clone(), paths.tools.pacman.clone());
    let version = spawn_blocking(move || installed_version(&os_release, &pacman))
        .await
        .ok()
        .flatten();
    let channel = tokio::fs::read_to_string(&paths.pacman_conf)
        .await
        .ok()
        .and_then(|conf| installed_channel(&conf));
    DetectedRelease { version, channel }
}

impl Engine {
    pub(super) async fn check(&self) -> Result<(), StepFailure> {
        self.begin(Phase::Checking).await;
        let detected = self.refresh_installed().await;
        let (installed, channel) = detected.require().at(FailedStep::DetectRelease)?;
        let found = find_next_release(&self.paths, installed, channel)
            .await
            .at(FailedStep::Check)?;
        let next = match &found {
            Some(found) => Some(self.next_release(found).await.at(FailedStep::Check)?),
            None => None,
        };
        let saved = match &found {
            Some(found) => {
                self.store
                    .save_next(&found.verified.bytes, found.notes.as_deref())
                    .await
            }
            None => self.store.clear_next().await,
        };
        saved
            .context("cannot save the next release")
            .at(FailedStep::Check)?;
        self.transition(|snapshot| {
            let already_downloaded = matches!(
                (&snapshot.next, &next),
                (Some(known), Some(found))
                    if known.version == found.version && known.download == DownloadStatus::Complete
            );
            snapshot.next = next.map(|found| NextRelease {
                download: if already_downloaded {
                    DownloadStatus::Complete
                } else {
                    found.download
                },
                ..found
            });
            snapshot.last_check = Some(UnixSeconds::now());
            snapshot.phase = Phase::Idle;
        })
        .await;
        Ok(())
    }

    async fn next_release(&self, found: &FoundRelease) -> anyhow::Result<NextRelease> {
        let manifest = &found.verified.manifest;
        Ok(NextRelease {
            version: manifest.version.clone(),
            restart: manifest.restart,
            notes: found.notes.clone(),
            new_packages: self.new_packages(manifest).await?,
            download: DownloadStatus::Pending,
        })
    }

    /// The release's packages that this system does not have yet.
    async fn new_packages(&self, manifest: &ReleaseManifest) -> anyhow::Result<Vec<PackageName>> {
        let installed = installed_packages(&self.paths.tools.pacman).await?;
        Ok(manifest
            .packages
            .iter()
            .filter(|package| !installed.contains(package.as_str()))
            .cloned()
            .collect())
    }
}

/// Reads the release list of the moving channel, then the signed manifest
/// and notes of the first release after `installed`.
async fn find_next_release(
    paths: &Paths,
    installed: &ReleaseVersion,
    channel: Channel,
) -> anyhow::Result<Option<FoundRelease>> {
    let paths = paths.clone();
    let installed = installed.clone();
    spawn_blocking(move || -> anyhow::Result<Option<FoundRelease>> {
        let index = fetch_verified(
            &paths.repository_url,
            index_channel(channel),
            &paths.keyring,
        )?;
        index.manifest.validate_chain()?;
        let Some(next) = next_release(&installed, channel, &index.manifest) else {
            return Ok(None);
        };
        let next_channel = channel_of(next);
        let verified = fetch_verified(&paths.repository_url, next_channel, &paths.keyring)?;
        verified.manifest.expect_version(next)?;
        let notes = match &verified.manifest.notes {
            Some(file) => fetch_notes(&paths.repository_url, next_channel, file, &paths.keyring)?,
            None => None,
        };
        Ok(Some(FoundRelease {
            verified,
            notes: notes.map(bounded_notes),
        }))
    })
    .await
    .map_err(|error| anyhow!("the release check stopped: {error}"))?
}

fn bounded_notes(mut notes: String) -> String {
    if notes.len() > MAX_NOTES_BYTES {
        let boundary = notes.floor_char_boundary(MAX_NOTES_BYTES);
        let end = notes[..boundary].rfind('\n').unwrap_or(0);
        notes.truncate(end);
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_notes_end_at_a_whole_line() {
        let line = "a".repeat(99) + "\n";
        let notes = bounded_notes(line.repeat(1000));
        assert!(notes.len() <= MAX_NOTES_BYTES);
        assert!(notes.ends_with('a'));
        assert_eq!(bounded_notes("short".into()), "short");
    }
}
