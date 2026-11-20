//! What survives a restart of the daemon, under `/var/lib/crownos/update`.
//! The phase and the log are not kept: a new process always starts idle.

use std::{io, path::PathBuf};

use serde::{Deserialize, Serialize};
use tokio::fs;
use update_manifest::{PackageName, ReleaseVersion, RestartKind};
use update_proto::{
    ApplyStep, DownloadStatus, Failure, InProgress, NextRelease, PendingRestart, Rollback,
    Snapshot, UnixSeconds,
};

use crate::{
    paths::Paths,
    utils::{atomic_write::write_atomically, boot_marker::BootMarker},
};

const STATE_FILE: &str = "state.toml";
const NEXT_MANIFEST_FILE: &str = "next-manifest.toml";
const NEXT_NOTES_FILE: &str = "next-notes.md";
const APPLIED_FILE: &str = "applied.toml";
const IN_PROGRESS_FILE: &str = "in-progress.toml";

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct PersistedState {
    pub last_check: Option<UnixSeconds>,
    pub next: Option<PersistedNext>,
    pub failure: Option<Failure>,
}

/// [`NextRelease`] without the notes, which live in `next-notes.md`.
#[derive(Debug, Deserialize, Serialize)]
pub struct PersistedNext {
    pub version: ReleaseVersion,
    pub restart: RestartKind,
    pub new_packages: Vec<PackageName>,
    pub download: DownloadStatus,
}

/// Written only after the post-update script succeeded: the update is
/// installed and waits for its restart on this boot.
#[derive(Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct AppliedMarker {
    pub version: ReleaseVersion,
    pub from: ReleaseVersion,
    pub restart: RestartKind,
    #[serde(flatten)]
    pub boot: BootMarker,
}

/// An update that changed the system and did not finish. Written before
/// the pacman upgrade and after each later step; removed when the applied
/// marker is saved.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct InProgressUpdate {
    pub from: ReleaseVersion,
    pub to: ReleaseVersion,
    pub restart: RestartKind,
    pub new_packages: Vec<PackageName>,
    pub rollback: Rollback,
    pub completed: ApplyStep,
}

pub struct Store {
    directory: PathBuf,
}

impl PersistedState {
    fn of(snapshot: &Snapshot) -> Self {
        Self {
            last_check: snapshot.last_check,
            next: snapshot.next.as_ref().map(|next| PersistedNext {
                version: next.version.clone(),
                restart: next.restart,
                new_packages: next.new_packages.clone(),
                download: next.download,
            }),
            failure: snapshot.failure.clone(),
        }
    }
}

impl PersistedNext {
    pub fn with_notes(self, notes: Option<String>) -> NextRelease {
        NextRelease {
            version: self.version,
            restart: self.restart,
            notes,
            new_packages: self.new_packages,
            download: self.download,
        }
    }
}

impl AppliedMarker {
    pub fn pending_restart(&self) -> PendingRestart {
        PendingRestart {
            version: self.version.clone(),
            from: self.from.clone(),
            kind: self.restart,
        }
    }
}

impl InProgressUpdate {
    pub fn summary(&self) -> InProgress {
        InProgress {
            from: self.from.clone(),
            to: self.to.clone(),
            completed: self.completed,
        }
    }
}

impl Store {
    pub fn new(paths: &Paths) -> Self {
        Self {
            directory: paths.state_directory.clone(),
        }
    }

    fn file(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }

    pub async fn load_state(&self) -> PersistedState {
        read_toml(self.file(STATE_FILE)).await.unwrap_or_default()
    }

    pub async fn load_notes(&self) -> Option<String> {
        fs::read_to_string(self.file(NEXT_NOTES_FILE)).await.ok()
    }

    pub async fn load_applied(&self) -> Option<AppliedMarker> {
        read_toml(self.file(APPLIED_FILE)).await
    }

    pub async fn save_state(&self, snapshot: &Snapshot) -> io::Result<()> {
        self.write_toml(STATE_FILE, &PersistedState::of(snapshot))
            .await
    }

    pub async fn save_next(&self, manifest: &[u8], notes: Option<&str>) -> io::Result<()> {
        self.write(NEXT_MANIFEST_FILE, manifest).await?;
        match notes {
            Some(notes) => self.write(NEXT_NOTES_FILE, notes.as_bytes()).await,
            None => remove_if_present(self.file(NEXT_NOTES_FILE)).await,
        }
    }

    pub async fn clear_next(&self) -> io::Result<()> {
        remove_if_present(self.file(NEXT_MANIFEST_FILE)).await?;
        remove_if_present(self.file(NEXT_NOTES_FILE)).await
    }

    pub async fn save_applied(&self, marker: &AppliedMarker) -> io::Result<()> {
        self.write_toml(APPLIED_FILE, marker).await
    }

    pub async fn clear_applied(&self) -> io::Result<()> {
        remove_if_present(self.file(APPLIED_FILE)).await
    }

    pub async fn load_in_progress(&self) -> Option<InProgressUpdate> {
        read_toml(self.file(IN_PROGRESS_FILE)).await
    }

    pub async fn save_in_progress(&self, update: &InProgressUpdate) -> io::Result<()> {
        self.write_toml(IN_PROGRESS_FILE, update).await
    }

    pub async fn clear_in_progress(&self) -> io::Result<()> {
        remove_if_present(self.file(IN_PROGRESS_FILE)).await
    }

    async fn write_toml(&self, name: &str, value: &impl Serialize) -> io::Result<()> {
        let text = toml::to_string(value).map_err(io::Error::other)?;
        self.write(name, text.as_bytes()).await
    }

    async fn write(&self, name: &str, bytes: &[u8]) -> io::Result<()> {
        fs::create_dir_all(&self.directory).await?;
        write_atomically(&self.file(name), bytes).await
    }
}

async fn read_toml<T: serde::de::DeserializeOwned>(path: PathBuf) -> Option<T> {
    let text = fs::read_to_string(&path).await.ok()?;
    toml::from_str(&text)
        .inspect_err(|error| tracing::warn!("ignoring {}: {error}", path.display()))
        .ok()
}

async fn remove_if_present(path: PathBuf) -> io::Result<()> {
    match fs::remove_file(path).await {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use update_proto::{FailedStep, SnapperNumber};

    use super::*;

    fn store(directory: &Path) -> Store {
        Store {
            directory: directory.join("var/lib/crownos/update"),
        }
    }

    #[tokio::test]
    async fn the_state_round_trips_without_notes() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = store(directory.path());
        let snapshot = Snapshot {
            next: Some(NextRelease {
                version: "0.4.1".parse()?,
                restart: RestartKind::SoftReboot,
                notes: Some("notes".into()),
                new_packages: vec![PackageName::try_from("crowndock")?],
                download: DownloadStatus::Complete,
            }),
            failure: Some(Failure {
                step: FailedStep::Apply(ApplyStep::Install),
                message: "pacman failed".into(),
                rollback: Some(Rollback {
                    snapshot: SnapperNumber(7),
                    from: "0.4.0".parse()?,
                }),
            }),
            last_check: Some(UnixSeconds(5)),
            ..Snapshot::default()
        };
        store.save_state(&snapshot).await?;
        let loaded = store.load_state().await;
        assert_eq!(loaded.last_check, snapshot.last_check);
        assert_eq!(loaded.failure, snapshot.failure);
        let next = loaded.next.ok_or("no next")?.with_notes(None);
        assert_eq!(next.download, DownloadStatus::Complete);
        assert_eq!(next.new_packages.len(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn the_applied_marker_keeps_flat_boot_fields() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = store(directory.path());
        let marker = AppliedMarker {
            version: "0.4.1".parse()?,
            from: "0.4.0".parse()?,
            restart: RestartKind::Reboot,
            boot: BootMarker {
                boot_id: Some("abc".into()),
                soft_reboots: Some(0),
            },
        };
        store.save_applied(&marker).await?;
        let text = std::fs::read_to_string(store.file(APPLIED_FILE))?;
        assert!(text.contains("boot_id = \"abc\""), "{text}");
        assert_eq!(store.load_applied().await, Some(marker));
        store.clear_applied().await?;
        store.clear_applied().await?;
        assert_eq!(store.load_applied().await, None);
        Ok(())
    }

    #[tokio::test]
    async fn the_in_progress_record_round_trips() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let store = store(directory.path());
        let update = InProgressUpdate {
            from: "0.4.0".parse()?,
            to: "0.4.1".parse()?,
            restart: RestartKind::Session,
            new_packages: vec![PackageName::try_from("crowndock")?],
            rollback: Rollback {
                snapshot: SnapperNumber(7),
                from: "0.4.0".parse()?,
            },
            completed: ApplyStep::Install,
        };
        store.save_in_progress(&update).await?;
        let text = std::fs::read_to_string(store.file(IN_PROGRESS_FILE))?;
        assert!(text.contains("completed = \"install\""), "{text}");
        assert_eq!(store.load_in_progress().await, Some(update));
        store.clear_in_progress().await?;
        assert_eq!(store.load_in_progress().await, None);
        Ok(())
    }
}
