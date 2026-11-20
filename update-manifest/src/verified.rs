use thiserror::Error;

use crate::{
    Channel, Keyring, ManifestError, NotesFileName, ReleaseManifest, VerifyError,
    endpoint::{MANIFEST_FILE, channel_file_url, signature_url},
    fetch::{FetchError, fetch},
    verify::verify_detached,
};

/// A manifest whose signature checked out, with the exact bytes that were signed.
#[derive(Clone, Debug)]
pub struct VerifiedManifest {
    pub bytes: Vec<u8>,
    pub manifest: ReleaseManifest,
}

#[derive(Debug, Error)]
pub enum PublishedFileError {
    #[error("cannot download {url}")]
    Fetch {
        url: String,
        #[source]
        source: FetchError,
    },
    #[error("{url} is not signed by CrownOS")]
    Verify {
        url: String,
        #[source]
        source: VerifyError,
    },
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    #[error("the release notes are not UTF-8")]
    NotesNotUtf8(#[from] std::string::FromUtf8Error),
}

/// Blocking: downloads `<channel>/manifest.toml` and checks its signature.
pub fn fetch_verified(
    repository_url: &str,
    channel: Channel,
    keyring: &Keyring,
) -> Result<VerifiedManifest, PublishedFileError> {
    let url = channel_file_url(repository_url, channel, MANIFEST_FILE);
    let bytes = fetch_signed(&url, keyring)?;
    let manifest = ReleaseManifest::parse(&bytes)?;
    Ok(VerifiedManifest { bytes, manifest })
}

/// Blocking: the release notes beside a manifest. A channel without notes
/// gives `None`; notes with a bad signature are an error.
pub fn fetch_notes(
    repository_url: &str,
    channel: Channel,
    file: &NotesFileName,
    keyring: &Keyring,
) -> Result<Option<String>, PublishedFileError> {
    let url = channel_file_url(repository_url, channel, file.as_str());
    match fetch_signed(&url, keyring) {
        Ok(bytes) => Ok(Some(String::from_utf8(bytes)?)),
        Err(PublishedFileError::Fetch {
            url: missing,
            source: FetchError::NotFound,
        }) if missing == url => Ok(None),
        Err(error) => Err(error),
    }
}

fn fetch_signed(url: &str, keyring: &Keyring) -> Result<Vec<u8>, PublishedFileError> {
    let bytes = download(url)?;
    let signature = download(&signature_url(url))?;
    verify_detached(keyring, &bytes, &signature).map_err(|source| PublishedFileError::Verify {
        url: url.to_owned(),
        source,
    })?;
    Ok(bytes)
}

fn download(url: &str) -> Result<Vec<u8>, PublishedFileError> {
    fetch(url).map_err(|source| PublishedFileError::Fetch {
        url: url.to_owned(),
        source,
    })
}
