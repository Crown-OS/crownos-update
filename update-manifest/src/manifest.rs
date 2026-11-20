use serde::Deserialize;
use thiserror::Error;

use crate::{Channel, NotesFileName, PackageName, ReleaseVersion, RestartKind};

/// The published `manifest.toml`; the `[[package]]` version table is not needed here.
#[derive(Clone, Debug, Deserialize)]
pub struct ReleaseManifest {
    pub version: ReleaseVersion,
    pub build_id: String,
    pub channel: Channel,
    #[serde(default)]
    pub restart: RestartKind,
    pub notes: Option<NotesFileName>,
    pub releases: Vec<ReleaseVersion>,
    pub packages: Vec<PackageName>,
}

#[derive(Debug, Error)]
pub enum ManifestError {
    #[error("the manifest is not UTF-8")]
    NotUtf8(#[from] std::str::Utf8Error),
    #[error("the manifest is malformed")]
    Syntax(#[from] toml::de::Error),
    #[error("the manifest lists no releases")]
    EmptyChain,
    #[error("the release list is not in ascending order at {0}")]
    ChainOutOfOrder(ReleaseVersion),
    #[error("the release list ends at {last}, not at the manifest version {version}")]
    ChainEnd {
        last: ReleaseVersion,
        version: ReleaseVersion,
    },
    #[error("the manifest describes {found}, not {expected}")]
    WrongVersion {
        expected: ReleaseVersion,
        found: ReleaseVersion,
    },
}

impl ReleaseManifest {
    pub fn parse(bytes: &[u8]) -> Result<Self, ManifestError> {
        Ok(toml::from_str(std::str::from_utf8(bytes)?)?)
    }

    pub fn validate_chain(&self) -> Result<(), ManifestError> {
        if let Some(pair) = self.releases.windows(2).find(|pair| pair[0] >= pair[1]) {
            return Err(ManifestError::ChainOutOfOrder(pair[1].clone()));
        }
        match self.releases.last() {
            None => Err(ManifestError::EmptyChain),
            Some(last) if *last != self.version => Err(ManifestError::ChainEnd {
                last: last.clone(),
                version: self.version.clone(),
            }),
            Some(_) => Ok(()),
        }
    }

    pub fn expect_version(&self, expected: &ReleaseVersion) -> Result<(), ManifestError> {
        if self.version == *expected {
            Ok(())
        } else {
            Err(ManifestError::WrongVersion {
                expected: expected.clone(),
                found: self.version.clone(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLISHED: &str = r#"
version = "0.4.2"
build_id = "0a1b2c"
channel = "stable"
restart = "soft-reboot"
notes = "notes.md"
releases = ["0.4.0", "0.4.1", "0.4.2"]
packages = ["crownbar", "crownos-base"]

[[package]]
name = "crownbar"
version = "0.3.0-1"
"#;

    #[test]
    fn parses_the_published_layout() -> Result<(), ManifestError> {
        let manifest = ReleaseManifest::parse(PUBLISHED.as_bytes())?;
        assert_eq!(manifest.version.to_string(), "0.4.2");
        assert_eq!(manifest.channel, Channel::Stable);
        assert_eq!(manifest.restart, RestartKind::SoftReboot);
        assert_eq!(
            manifest.notes.as_ref().map(NotesFileName::as_str),
            Some("notes.md")
        );
        assert_eq!(manifest.packages.len(), 2);
        manifest.validate_chain()
    }

    #[test]
    fn restart_defaults_to_reboot_and_notes_are_optional() -> Result<(), ManifestError> {
        let text = PUBLISHED
            .replace("restart = \"soft-reboot\"\n", "")
            .replace("notes = \"notes.md\"\n", "");
        let manifest = ReleaseManifest::parse(text.as_bytes())?;
        assert_eq!(manifest.restart, RestartKind::Reboot);
        assert!(manifest.notes.is_none());
        Ok(())
    }

    #[test]
    fn hostile_names_are_rejected() {
        for (from, to) in [
            ("\"crownbar\", ", "\"--overwrite=*\", "),
            ("\"notes.md\"", "\"../notes.md\""),
        ] {
            let text = PUBLISHED.replace(from, to);
            assert!(ReleaseManifest::parse(text.as_bytes()).is_err(), "{to}");
        }
    }

    #[test]
    fn a_broken_chain_is_refused() -> Result<(), ManifestError> {
        for (chain, broken) in [
            ("[]", "empty"),
            ("[\"0.4.1\", \"0.4.0\", \"0.4.2\"]", "order"),
            ("[\"0.4.0\", \"0.4.0\", \"0.4.2\"]", "duplicate"),
            ("[\"0.4.0\", \"0.4.1\"]", "end"),
        ] {
            let text = PUBLISHED.replace("[\"0.4.0\", \"0.4.1\", \"0.4.2\"]", chain);
            let manifest = ReleaseManifest::parse(text.as_bytes())?;
            assert!(manifest.validate_chain().is_err(), "{broken}");
        }
        Ok(())
    }

    #[test]
    fn expect_version_compares_the_whole_version() -> Result<(), Box<dyn std::error::Error>> {
        let manifest = ReleaseManifest::parse(PUBLISHED.as_bytes())?;
        manifest.expect_version(&"0.4.2".parse()?)?;
        assert!(matches!(
            manifest.expect_version(&"0.4.1".parse()?),
            Err(ManifestError::WrongVersion { .. })
        ));
        Ok(())
    }
}
