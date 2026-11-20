//! The wire contract of `crownos-updated`. Every local user may read the
//! state and subscribe; the four verbs need an administrator.

mod snapshot;
mod verb;

pub use crownos_ipc::RemoteError;
pub use snapshot::{
    ApplyStep, DownloadStatus, FailedStep, Failure, InProgress, NextRelease, PendingRestart, Phase,
    Rollback, SnapperNumber, Snapshot, UnixSeconds,
};
pub use update_manifest::{Channel, PackageName, ReleaseVersion, RestartKind};
pub use verb::Verb;

pub const SOCKET_DIR: &str = "/run/crownos";

/// The caller is not root and not a member of `wheel`.
pub const UNAUTHORIZED: u32 = RemoteError::UNAUTHORIZED;
/// The verb does not fit the current phase, for example `apply` with no update.
pub const WRONG_PHASE: u32 = 100;

crownos_ipc::protocol! {
    pub mod updated: "crownos-updated" {
        fn state() -> Snapshot;
        fn check();
        fn download();
        fn apply();
        fn restart();
        event StateChanged { snapshot: Snapshot };
        event Progress { phase: Phase, fraction: Option<f32>, line: String };
    }
}
