use std::{
    env,
    path::{Path, PathBuf},
};

use update_client::proto::{ReleaseVersion, Snapshot};

const STATE_DIRECTORY: &str = "crownos";
const STATE_FILE: &str = "update-notify";

/// The last release announced, kept in `$XDG_STATE_HOME/crownos/update-notify`
/// (else `~/.local/state`). Without a home directory nothing is remembered.
pub struct AnnouncedRelease {
    path: Option<PathBuf>,
}

impl AnnouncedRelease {
    pub fn at_default_path() -> Self {
        let base = env::var_os("XDG_STATE_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")));
        Self {
            path: base.map(|base| base.join(STATE_DIRECTORY).join(STATE_FILE)),
        }
    }

    /// Anything unreadable counts as nothing announced.
    pub async fn read(&self) -> Option<ReleaseVersion> {
        let text = tokio::fs::read_to_string(self.path.as_deref()?)
            .await
            .ok()?;
        text.trim().parse().ok()
    }

    /// Best effort: failing to remember only means announcing again.
    pub async fn remember(&self, version: &ReleaseVersion) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if let Err(error) = write(path, version).await {
            tracing::warn!(
                "cannot remember the announced release in {}: {error}",
                path.display()
            );
        }
    }
}

async fn write(path: &Path, version: &ReleaseVersion) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(path, version.to_string()).await
}

/// The release to announce: newer than this system and than the last one announced.
pub fn release_to_announce<'a>(
    snapshot: &'a Snapshot,
    last_announced: Option<&ReleaseVersion>,
) -> Option<&'a ReleaseVersion> {
    let available = &snapshot.next.as_ref()?.version;
    let newer_than_installed = snapshot
        .installed
        .as_ref()
        .is_none_or(|installed| available > installed);
    let unannounced = last_announced.is_none_or(|announced| available > announced);
    (newer_than_installed && unannounced).then_some(available)
}

#[cfg(test)]
mod tests {
    use update_client::proto::{DownloadStatus, NextRelease, RestartKind};

    use super::*;

    fn snapshot(installed: &str, next: &str) -> Result<Snapshot, Box<dyn std::error::Error>> {
        Ok(Snapshot {
            installed: Some(installed.parse()?),
            next: Some(NextRelease {
                version: next.parse()?,
                restart: RestartKind::Reboot,
                notes: None,
                new_packages: Vec::new(),
                download: DownloadStatus::Pending,
            }),
            ..Snapshot::default()
        })
    }

    #[test]
    fn announces_only_newer_unannounced_releases() -> Result<(), Box<dyn std::error::Error>> {
        let announced: ReleaseVersion = "0.4.1".parse()?;
        let offered = snapshot("0.4.0", "0.4.1")?;
        assert_eq!(release_to_announce(&offered, None), Some(&announced));
        assert_eq!(release_to_announce(&offered, Some(&announced)), None);
        let later = snapshot("0.4.1", "0.4.2")?;
        assert!(release_to_announce(&later, Some(&announced)).is_some());
        assert_eq!(
            release_to_announce(&snapshot("0.4.1", "0.4.1")?, None),
            None
        );
        assert_eq!(release_to_announce(&Snapshot::default(), None), None);
        Ok(())
    }

    #[tokio::test]
    async fn remembers_and_forgets_gracefully() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let announced = AnnouncedRelease {
            path: Some(directory.path().join("crownos/update-notify")),
        };
        assert!(announced.read().await.is_none());
        let version: ReleaseVersion = "0.4.1".parse()?;
        announced.remember(&version).await;
        assert_eq!(announced.read().await, Some(version));
        std::fs::write(directory.path().join("crownos/update-notify"), "garbage")?;
        assert!(announced.read().await.is_none());
        assert!(AnnouncedRelease { path: None }.read().await.is_none());
        Ok(())
    }
}
