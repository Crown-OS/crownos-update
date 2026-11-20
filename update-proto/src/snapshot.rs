use std::{
    fmt,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use update_manifest::{Channel, PackageName, ReleaseVersion, RestartKind};

/// Everything the daemon knows, as one value. `state()` returns it and
/// `StateChanged` carries it after each transition.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    pub installed: Option<ReleaseVersion>,
    pub channel: Option<Channel>,
    pub phase: Phase,
    pub next: Option<NextRelease>,
    pub pending_restart: Option<PendingRestart>,
    pub in_progress: Option<InProgress>,
    pub failure: Option<Failure>,
    pub last_check: Option<UnixSeconds>,
    pub log_tail: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    Downloading,
    Applying(ApplyStep),
}

/// The steps of an update, in the order they run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ApplyStep {
    Snapshot,
    KeepKernel,
    Install,
    SwitchChannel,
    PostUpdate,
    RecordApplied,
}

/// An update that changed the system and is not finished yet. `apply`
/// continues it after the step that `completed` names.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InProgress {
    pub from: ReleaseVersion,
    pub to: ReleaseVersion,
    pub completed: ApplyStep,
}

/// The release after the installed one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NextRelease {
    pub version: ReleaseVersion,
    pub restart: RestartKind,
    pub notes: Option<String>,
    pub new_packages: Vec<PackageName>,
    pub download: DownloadStatus,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DownloadStatus {
    #[default]
    Pending,
    Complete,
}

/// An update is installed and waits for the restart its release asks for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRestart {
    pub version: ReleaseVersion,
    pub from: ReleaseVersion,
    pub kind: RestartKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Failure {
    pub step: FailedStep,
    pub message: String,
    pub rollback: Option<Rollback>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailedStep {
    DetectRelease,
    Check,
    Download,
    Apply(ApplyStep),
    Restart,
}

/// The snapshot to boot from the hidden rEFInd menu to get `from` back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rollback {
    pub snapshot: SnapperNumber,
    pub from: ReleaseVersion,
}

/// The number snapper gave a snapshot of the root configuration.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SnapperNumber(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct UnixSeconds(pub u64);

impl ApplyStep {
    /// The step that runs after this one; `None` after the last step.
    #[must_use]
    pub fn following(self) -> Option<Self> {
        match self {
            Self::Snapshot => Some(Self::KeepKernel),
            Self::KeepKernel => Some(Self::Install),
            Self::Install => Some(Self::SwitchChannel),
            Self::SwitchChannel => Some(Self::PostUpdate),
            Self::PostUpdate => Some(Self::RecordApplied),
            Self::RecordApplied => None,
        }
    }
}

impl Snapshot {
    #[must_use]
    pub fn is_idle(&self) -> bool {
        self.phase == Phase::Idle
    }
}

impl UnixSeconds {
    pub fn now() -> Self {
        Self(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs()),
        )
    }
}

impl fmt::Display for SnapperNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed_snapshot() -> Result<Snapshot, Box<dyn std::error::Error>> {
        Ok(Snapshot {
            installed: Some("0.4.0".parse()?),
            channel: Some("v0.4.0".parse()?),
            phase: Phase::Applying(ApplyStep::Install),
            next: Some(NextRelease {
                version: "0.4.1".parse()?,
                restart: RestartKind::SoftReboot,
                notes: Some("## Highlights".into()),
                new_packages: vec![PackageName::try_from("crowndock")?],
                download: DownloadStatus::Complete,
            }),
            pending_restart: None,
            in_progress: Some(InProgress {
                from: "0.4.0".parse()?,
                to: "0.4.1".parse()?,
                completed: ApplyStep::Install,
            }),
            failure: Some(Failure {
                step: FailedStep::Apply(ApplyStep::PostUpdate),
                message: "post-update exited with status 1".into(),
                rollback: Some(Rollback {
                    snapshot: SnapperNumber(7),
                    from: "0.4.0".parse()?,
                }),
            }),
            last_check: Some(UnixSeconds(1_763_600_000)),
            log_tail: vec!["(1/2) upgrading crownbar".into()],
        })
    }

    #[test]
    fn crosses_the_wire_unchanged() -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = failed_snapshot()?;
        let bytes = crownos_ipc::postcard::to_stdvec(&snapshot)?;
        assert_eq!(
            crownos_ipc::postcard::from_bytes::<Snapshot>(&bytes)?,
            snapshot
        );
        Ok(())
    }

    #[test]
    fn steps_follow_in_run_order() {
        let mut step = ApplyStep::Snapshot;
        while let Some(following) = step.following() {
            assert!(following > step);
            step = following;
        }
        assert_eq!(step, ApplyStep::RecordApplied);
    }

    #[test]
    fn failures_persist_as_toml() -> Result<(), Box<dyn std::error::Error>> {
        let failure = failed_snapshot()?.failure.ok_or("no failure")?;
        let text = toml::to_string(&failure)?;
        assert_eq!(toml::from_str::<Failure>(&text)?, failure);
        Ok(())
    }
}
