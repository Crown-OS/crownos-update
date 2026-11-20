use std::{fmt, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::ReleaseNumber;

const STABLE: &str = "stable";
const TESTING: &str = "testing";
const RELEASE_PREFIX: char = 'v';

/// A directory of the CrownOS repository: `stable` and `testing` move,
/// `v<X.Y.Z>` holds one release forever.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub enum Channel {
    Stable,
    Testing,
    Release(ReleaseNumber),
}

#[derive(Debug, Error)]
#[error("{0:?} is not a CrownOS channel such as stable, testing or v0.4.1")]
pub struct ChannelError(String);

impl FromStr for Channel {
    type Err = ChannelError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text {
            STABLE => Ok(Self::Stable),
            TESTING => Ok(Self::Testing),
            _ => text
                .strip_prefix(RELEASE_PREFIX)
                .and_then(|number| number.parse().ok())
                .map(Self::Release)
                .ok_or_else(|| ChannelError(text.to_owned())),
        }
    }
}

impl TryFrom<String> for Channel {
    type Error = ChannelError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        text.parse()
    }
}

impl From<Channel> for String {
    fn from(channel: Channel) -> Self {
        channel.to_string()
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stable => formatter.write_str(STABLE),
            Self::Testing => formatter.write_str(TESTING),
            Self::Release(number) => write!(formatter, "{RELEASE_PREFIX}{number}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_every_form() -> Result<(), ChannelError> {
        for text in ["stable", "testing", "v0.4.1"] {
            assert_eq!(text.parse::<Channel>()?.to_string(), text);
        }
        Ok(())
    }

    #[test]
    fn rejects_other_directories() {
        for bad in [
            "",
            "Stable",
            "0.4.1",
            "v0.4",
            "v0.4.1.r2.gabcdef0",
            "x86_64",
        ] {
            assert!(bad.parse::<Channel>().is_err(), "{bad:?} accepted");
        }
    }
}
