use std::{io, sync::LazyLock, time::Duration};

use thiserror::Error;
use ureq::Agent;

const TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;
const NOT_FOUND: u16 = 404;
const FILE_SCHEME: &str = "file://";

static AGENT: LazyLock<Agent> = LazyLock::new(|| {
    Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(Some(TIMEOUT))
        .user_agent(concat!("crownos-update/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
});

#[derive(Debug, Error)]
pub enum FetchError {
    #[error("{0}")]
    Transport(#[from] ureq::Error),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("the server has no such file")]
    NotFound,
    #[error("the server answered {0}")]
    Status(u16),
}

/// Blocking GET of `url`. A `file://` URL reads the file, so a locally built
/// repository can be tested without a server.
pub fn fetch(url: &str) -> Result<Vec<u8>, FetchError> {
    if let Some(path) = url.strip_prefix(FILE_SCHEME) {
        return std::fs::read(path).map_err(|error| match error.kind() {
            io::ErrorKind::NotFound => FetchError::NotFound,
            _ => FetchError::Io(error),
        });
    }
    let mut response = AGENT.get(url).call()?;
    match response.status().as_u16() {
        NOT_FOUND => Err(FetchError::NotFound),
        _ if response.status().is_success() => Ok(response
            .body_mut()
            .with_config()
            .limit(MAX_BODY_BYTES)
            .read_to_vec()?),
        status => Err(FetchError::Status(status)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_read_files_and_report_missing_ones() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("manifest.toml");
        std::fs::write(&path, "x")?;
        let url = format!("{FILE_SCHEME}{}", path.display());
        assert_eq!(fetch(&url).ok(), Some(b"x".to_vec()));
        assert!(matches!(
            fetch(&format!("{url}.sig")),
            Err(FetchError::NotFound)
        ));
        Ok(())
    }
}
