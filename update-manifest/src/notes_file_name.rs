use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The file name of the release notes beside a manifest. It is one path
/// component, so it cannot point outside the channel directory.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(try_from = "String", into = "String")]
pub struct NotesFileName(String);

#[derive(Debug, Error)]
#[error("{0:?} is not a file name beside the manifest")]
pub struct NotesFileNameError(String);

impl NotesFileName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for NotesFileName {
    type Error = NotesFileNameError;

    fn try_from(text: String) -> Result<Self, Self::Error> {
        let well_formed = !text.is_empty()
            && !text.starts_with('.')
            && text
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
        if well_formed {
            Ok(Self(text))
        } else {
            Err(NotesFileNameError(text))
        }
    }
}

impl From<NotesFileName> for String {
    fn from(name: NotesFileName) -> Self {
        name.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_file_names_pass() {
        assert!(NotesFileName::try_from("notes.md".to_owned()).is_ok());
        for bad in ["", "../notes.md", "a/notes.md", ".notes", "notes .md"] {
            assert!(NotesFileName::try_from(bad.to_owned()).is_err(), "{bad:?}");
        }
    }
}
