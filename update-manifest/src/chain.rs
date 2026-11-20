//! A stable install steps through every tagged release in order: the next
//! target is the first release after the installed one. A testing install
//! stays on testing and follows the newest testing build.

use crate::{Channel, ReleaseManifest, ReleaseVersion};

const CROWNOS_SECTION: &str = "[crownos]";
const SERVER_KEY: &str = "Server";
const ARCHITECTURE_SUFFIX: &str = "/$arch";

/// The release to install next, from the manifest of
/// [`index_channel`]`(installed_channel)`.
#[must_use]
pub fn next_release<'a>(
    installed: &ReleaseVersion,
    installed_channel: Channel,
    index: &'a ReleaseManifest,
) -> Option<&'a ReleaseVersion> {
    match installed_channel {
        Channel::Testing => (index.version > *installed).then_some(&index.version),
        Channel::Stable | Channel::Release(_) => {
            index.releases.iter().find(|release| *release > installed)
        }
    }
}

#[must_use]
pub fn channel_of(version: &ReleaseVersion) -> Channel {
    if version.is_testing() {
        Channel::Testing
    } else {
        Channel::Release(version.number)
    }
}

/// The moving channel whose manifest lists the releases after `installed`.
#[must_use]
pub fn index_channel(installed: Channel) -> Channel {
    match installed {
        Channel::Release(_) => Channel::Stable,
        moving => moving,
    }
}

/// The channel the `[crownos]` `Server` line of a pacman.conf points at.
pub fn installed_channel(pacman_conf: &str) -> Option<Channel> {
    crownos_server_urls(pacman_conf)
        .find_map(|url| split_server_url(url).and_then(|(_, channel)| channel.parse().ok()))
}

/// The `Server` values of the `[crownos]` section, in order.
fn crownos_server_urls(pacman_conf: &str) -> impl Iterator<Item = &str> {
    let mut in_crownos = false;
    pacman_conf.lines().filter_map(move |line| {
        let line = line.trim();
        if line.starts_with('[') {
            in_crownos = line == CROWNOS_SECTION;
            return None;
        }
        in_crownos.then(|| server_value(line)).flatten()
    })
}

/// The value of a `Server = ...` line.
pub fn server_value(line: &str) -> Option<&str> {
    let (key, value) = line.trim().split_once('=')?;
    (key.trim() == SERVER_KEY).then(|| value.trim())
}

/// `https://repo.crownos.org/v0.4.0/$arch` → (`https://repo.crownos.org`, `v0.4.0`).
pub fn split_server_url(url: &str) -> Option<(&str, &str)> {
    url.strip_suffix(ARCHITECTURE_SUFFIX)?.rsplit_once('/')
}

pub fn server_url(base: &str, channel: Channel) -> String {
    format!("{base}/{channel}{ARCHITECTURE_SUFFIX}")
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn index(version: &str, releases: &[&str]) -> Result<ReleaseManifest, crate::ManifestError> {
        let releases = releases
            .iter()
            .map(|release| format!("\"{release}\""))
            .collect::<Vec<_>>()
            .join(", ");
        ReleaseManifest::parse(
            format!(
                "version = \"{version}\"\nbuild_id = \"x\"\nchannel = \"stable\"\n\
                 releases = [{releases}]\npackages = []\n"
            )
            .as_bytes(),
        )
    }

    fn next(
        installed: &str,
        channel: &str,
        index: &ReleaseManifest,
    ) -> Result<Option<String>, Box<dyn std::error::Error>> {
        Ok(next_release(&installed.parse()?, channel.parse()?, index).map(ToString::to_string))
    }

    #[test]
    fn a_stable_install_takes_the_first_greater_release() -> TestResult {
        let stable = index("0.4.2", &["0.4.0", "0.4.1", "0.4.2"])?;
        assert_eq!(next("0.4.0", "v0.4.0", &stable)?.as_deref(), Some("0.4.1"));
        assert_eq!(next("0.3.9", "v0.3.9", &stable)?.as_deref(), Some("0.4.0"));
        assert_eq!(next("0.4.1", "stable", &stable)?.as_deref(), Some("0.4.2"));
        assert_eq!(next("0.4.2", "v0.4.2", &stable)?, None);
        Ok(())
    }

    #[test]
    fn a_testing_install_follows_the_newest_testing_build() -> TestResult {
        let testing = index(
            "0.4.1.r2.gabcdef0",
            &["0.4.0", "0.4.1", "0.4.1.r2.gabcdef0"],
        )?;
        assert_eq!(
            next("0.4.0.r3.g1234567", "testing", &testing)?.as_deref(),
            Some("0.4.1.r2.gabcdef0")
        );
        assert_eq!(next("0.4.1.r2.gabcdef0", "testing", &testing)?, None);
        assert_eq!(next("0.4.1.r5.g7654321", "testing", &testing)?, None);
        Ok(())
    }

    #[test]
    fn channels_follow_the_version_kind() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(channel_of(&"0.4.1".parse()?).to_string(), "v0.4.1");
        assert_eq!(channel_of(&"0.4.1.r2.gabcdef0".parse()?), Channel::Testing);
        assert_eq!(index_channel("v0.4.1".parse()?), Channel::Stable);
        assert_eq!(index_channel(Channel::Testing), Channel::Testing);
        assert_eq!(index_channel(Channel::Stable), Channel::Stable);
        Ok(())
    }

    const CONF: &str = "[options]\nServer = https://elsewhere/v9.9.9/$arch\n\n\
        [core]\nInclude = /etc/pacman.d/mirrorlist\n\n\
        [crownos]\nSigLevel = Required\n  Server  =  https://repo.crownos.org/v0.4.0/$arch  \n";

    #[test]
    fn reads_only_the_crownos_server_line() -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(installed_channel(CONF), Some("v0.4.0".parse()?));
        let testing = CONF.replace("v0.4.0", "testing");
        assert_eq!(installed_channel(&testing), Some(Channel::Testing));
        assert_eq!(installed_channel("[core]\nServer = x/v0.4.0/$arch\n"), None);
        assert_eq!(installed_channel(&CONF.replace("/$arch", "")), None);
        Ok(())
    }

    #[test]
    fn server_urls_round_trip() -> Result<(), Box<dyn std::error::Error>> {
        let url = "https://repo.crownos.org/v0.4.0/$arch";
        let (base, channel) = split_server_url(url).ok_or("no channel")?;
        assert_eq!(server_url(base, channel.parse()?), url);
        Ok(())
    }
}
