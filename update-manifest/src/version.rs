use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The `X.Y.Z` of a `vX.Y.Z` crownos-iso tag.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ReleaseNumber {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// A testing build: `distance` commits after the tag, at `commit`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TestingBuild {
    pub distance: u32,
    pub commit: CommitId,
}

/// An abbreviated git commit hash, lowercase hexadecimal.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CommitId(String);

/// A CrownOS release as `lib.sh:iso_version` spells it: `X.Y.Z` for a tag,
/// `X.Y.Z.rN.g<commit>` for a testing build. A testing build sorts after its
/// tag and before the next release, as `vercmp` sorts them.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct ReleaseVersion {
    pub number: ReleaseNumber,
    pub testing: Option<TestingBuild>,
}

#[derive(Debug, Error)]
#[error("{0:?} is not a release version such as 0.4.1 or 0.4.1.r3.g1a2b3c4")]
pub struct VersionError(String);

const COMMIT_LENGTHS: std::ops::RangeInclusive<usize> = 4..=40;

fn decimal(part: &str) -> Option<u32> {
    if part.is_empty() || !part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    part.parse().ok()
}

impl CommitId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    fn parse(text: &str) -> Option<Self> {
        let well_formed = COMMIT_LENGTHS.contains(&text.len())
            && text
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        well_formed.then(|| Self(text.to_owned()))
    }
}

impl ReleaseVersion {
    pub fn is_testing(&self) -> bool {
        self.testing.is_some()
    }
}

impl From<ReleaseNumber> for ReleaseVersion {
    fn from(number: ReleaseNumber) -> Self {
        Self {
            number,
            testing: None,
        }
    }
}

impl FromStr for ReleaseNumber {
    type Err = VersionError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.parse::<ReleaseVersion>()? {
            ReleaseVersion {
                number,
                testing: None,
            } => Ok(number),
            ReleaseVersion {
                testing: Some(_), ..
            } => Err(VersionError(text.to_owned())),
        }
    }
}

impl FromStr for ReleaseVersion {
    type Err = VersionError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let invalid = || VersionError(text.to_owned());
        let parts: Vec<&str> = text.split('.').collect();
        let (release, testing) = match parts.as_slice() {
            [major, minor, patch] => ([major, minor, patch], None),
            [major, minor, patch, distance, commit] => {
                ([major, minor, patch], Some((*distance, *commit)))
            }
            _ => return Err(invalid()),
        };
        let [major, minor, patch] = release.map(|part| decimal(part));
        let number = ReleaseNumber {
            major: major.ok_or_else(invalid)?,
            minor: minor.ok_or_else(invalid)?,
            patch: patch.ok_or_else(invalid)?,
        };
        let testing = match testing {
            None => None,
            Some((distance, commit)) => Some(TestingBuild {
                distance: distance
                    .strip_prefix('r')
                    .and_then(decimal)
                    .ok_or_else(invalid)?,
                commit: commit
                    .strip_prefix('g')
                    .and_then(CommitId::parse)
                    .ok_or_else(invalid)?,
            }),
        };
        Ok(Self { number, testing })
    }
}

impl TryFrom<String> for ReleaseVersion {
    type Error = VersionError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<ReleaseVersion> for String {
    fn from(version: ReleaseVersion) -> Self {
        version.to_string()
    }
}

impl fmt::Display for ReleaseNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl fmt::Display for ReleaseVersion {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.number)?;
        if let Some(TestingBuild { distance, commit }) = &self.testing {
            write!(formatter, ".r{distance}.g{}", commit.as_str())?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn version(text: &str) -> Result<ReleaseVersion, VersionError> {
        text.parse()
    }

    #[test]
    fn compares_numerically() -> Result<(), VersionError> {
        assert!(version("0.1.12")? > version("0.1.9")?);
        assert!(version("1.0.0")? > version("0.99.99")?);
        assert_eq!(version("0.1.12")?, version("0.1.12")?);
        Ok(())
    }

    #[test]
    fn testing_builds_sort_like_vercmp() -> Result<(), VersionError> {
        assert!(version("0.1.11.r7.gf66daf5")? > version("0.1.11")?);
        assert!(version("0.1.11.r7.gf66daf5")? < version("0.1.12")?);
        assert!(version("0.1.11.r10.g0000000")? > version("0.1.11.r7.gf66daf5")?);
        Ok(())
    }

    #[test]
    fn round_trips_both_forms() -> Result<(), VersionError> {
        for text in ["0.4.1", "0.1.11.r7.gf66daf5"] {
            assert_eq!(version(text)?.to_string(), text);
        }
        assert!(version("0.1.11.r7.gf66daf5")?.is_testing());
        Ok(())
    }

    #[test]
    fn rejects_tags_and_malformed_builds() {
        for bad in [
            "v0.1.2",
            "1.2",
            "1.2.3.4",
            "1..3",
            "+1.2.3",
            "1.2.-3",
            "",
            "0.1.11.7.gf66daf5",
            "0.1.11.r7.f66daf5",
            "0.1.11.r7.gF66DAF5",
            "0.1.11.r7.gxyz1234",
            "0.1.11.r.gf66daf5",
        ] {
            assert!(version(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn a_release_number_is_never_a_testing_build() {
        assert!("0.4.1".parse::<ReleaseNumber>().is_ok());
        assert!("0.4.1.r1.gabcdef0".parse::<ReleaseNumber>().is_err());
    }

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Holder {
        version: ReleaseVersion,
    }

    #[test]
    fn round_trips_through_toml() -> Result<(), Box<dyn std::error::Error>> {
        let holder = Holder {
            version: version("0.1.12")?,
        };
        let text = toml::to_string(&holder)?;
        assert_eq!(text.trim(), "version = \"0.1.12\"");
        assert_eq!(toml::from_str::<Holder>(&text)?, holder);
        assert!(toml::from_str::<Holder>("version = \"v1.2.3\"").is_err());
        Ok(())
    }
}
