use std::fmt;

use serde::{Deserialize, Serialize};
use thiserror::Error;

const MAX_LENGTH: usize = 128;

/// A pacman package name. It never starts with `-`, so pacman cannot read it
/// as an option.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct PackageName(String);

#[derive(Debug, Error)]
#[error("{0:?} is not a package name")]
pub struct PackageNameError(String);

impl PackageName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for PackageName {
    type Error = PackageNameError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let allowed =
            |c: char| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '_' | '+' | '-');
        let well_formed = text.len() <= MAX_LENGTH
            && text.starts_with(|first: char| first.is_ascii_alphanumeric())
            && text.chars().all(allowed);
        if well_formed {
            Ok(Self(text))
        } else {
            Err(PackageNameError(text))
        }
    }
}

impl TryFrom<&str> for PackageName {
    type Error = PackageNameError;

    fn try_from(text: &str) -> Result<Self, Self::Error> {
        Self::try_from(text.to_owned())
    }
}

impl From<PackageName> for String {
    fn from(name: PackageName) -> Self {
        name.0
    }
}

impl AsRef<str> for PackageName {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PackageName {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_pacman_names() {
        for good in ["crownbar", "lib32-glibc", "gtk+3", "python3.12", "a@b_c"] {
            assert!(PackageName::try_from(good).is_ok(), "{good:?} refused");
        }
    }

    #[test]
    fn refuses_options_and_paths() {
        for bad in ["", "--overwrite=*", "-Syu", ".hidden", "a/b", "a b"] {
            assert!(PackageName::try_from(bad).is_err(), "{bad:?} accepted");
        }
    }
}
