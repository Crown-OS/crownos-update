//! The CrownOS release as the repository publishes it. The `fetch` feature
//! adds the signed downloads; without it the crate holds only the types, so
//! clients of the update daemon do not build an HTTP stack.

pub mod chain;
mod channel;
pub mod endpoint;
mod installed;
mod manifest;
mod notes_file_name;
mod package_name;
mod restart;
pub mod utils;
mod version;

#[cfg(feature = "fetch")]
pub mod fetch;
#[cfg(feature = "fetch")]
mod verified;
#[cfg(feature = "fetch")]
pub mod verify;

pub use channel::{Channel, ChannelError};
pub use installed::installed_version;
pub use manifest::{ManifestError, ReleaseManifest};
pub use notes_file_name::{NotesFileName, NotesFileNameError};
pub use package_name::{PackageName, PackageNameError};
pub use restart::RestartKind;
pub use version::{CommitId, ReleaseNumber, ReleaseVersion, TestingBuild, VersionError};

#[cfg(feature = "fetch")]
pub use verified::{PublishedFileError, VerifiedManifest, fetch_notes, fetch_verified};
#[cfg(feature = "fetch")]
pub use verify::{Keyring, VerifyError};
