use std::{path::Path, process::Command};

use crate::{ReleaseVersion, utils::os_release::read_version_id};

const RELEASE_PACKAGE: &str = "crownos-release";

/// The release this system runs: `VERSION_ID` from os-release, else the
/// version of the `crownos-release` package, which the manifest keeps equal to
/// it. Blocking: it can run pacman.
pub fn installed_version(os_release: &Path, pacman: &Path) -> Option<ReleaseVersion> {
    read_version_id(os_release)
        .and_then(|text| text.parse().ok())
        .or_else(|| packaged_version(pacman))
}

fn packaged_version(pacman: &Path) -> Option<ReleaseVersion> {
    let output = Command::new(pacman)
        .args(["-Q", RELEASE_PACKAGE])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    version_from_query(&String::from_utf8_lossy(&output.stdout))
}

fn version_from_query(output: &str) -> Option<ReleaseVersion> {
    let (version, _package_release) = output.split_whitespace().nth(1)?.rsplit_once('-')?;
    version.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_the_package_release() {
        let version = version_from_query("crownos-release 0.1.12-1\n");
        assert_eq!(
            version.map(|found| found.to_string()).as_deref(),
            Some("0.1.12")
        );
    }

    #[test]
    fn unrelated_output_is_not_a_version() {
        assert!(version_from_query("").is_none());
        assert!(version_from_query("crownos-release rolling-1").is_none());
        assert!(version_from_query("crownos-release 0.1.12").is_none());
    }

    #[test]
    fn os_release_wins_over_the_package() -> std::io::Result<()> {
        let directory = tempfile::tempdir()?;
        let os_release = directory.path().join("os-release");
        std::fs::write(&os_release, "VERSION_ID=\"0.4.0\"\n")?;
        let version = installed_version(&os_release, Path::new("/nonexistent/pacman"));
        assert_eq!(
            version.map(|found| found.to_string()).as_deref(),
            Some("0.4.0")
        );
        Ok(())
    }
}
