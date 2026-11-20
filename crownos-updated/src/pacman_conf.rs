use anyhow::bail;
use update_manifest::{
    Channel,
    chain::{server_url, server_value, split_server_url},
};

const CROWNOS_SECTION: &str = "[crownos]";

/// `conf` with every `Server` line of `[crownos]` pointed at `channel`. The
/// rest of the file stays byte for byte.
pub fn with_channel(conf: &str, channel: Channel) -> anyhow::Result<String> {
    let mut in_crownos = false;
    let mut rewritten = 0;
    let mut output = String::with_capacity(conf.len() + 16);
    for line in conf.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_crownos = trimmed == CROWNOS_SECTION;
        }
        let base = in_crownos
            .then(|| server_value(trimmed).and_then(split_server_url))
            .flatten()
            .map(|(base, _)| base);
        match base {
            Some(base) => {
                output.push_str("Server = ");
                output.push_str(&server_url(base, channel));
                if line.ends_with('\n') {
                    output.push('\n');
                }
                rewritten += 1;
            }
            None => output.push_str(line),
        }
    }
    if rewritten == 0 {
        bail!("pacman.conf has no [crownos] Server line of the form <repository>/<channel>/$arch");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use update_manifest::chain::installed_channel;

    use super::*;

    const CONF: &str = "[options]\nArchitecture = auto\n\n[core]\nServer = https://mirror/v1.0.0/$arch\n\n\
        [crownos]\nSigLevel = Required DatabaseOptional\nServer = https://repo.crownos.org/v0.4.0/$arch\n\n\
        [extra]\nInclude = /etc/pacman.d/mirrorlist\n";

    #[test]
    fn rewrites_only_the_crownos_server() -> anyhow::Result<()> {
        let rewritten = with_channel(CONF, "v0.4.1".parse()?)?;
        assert_eq!(
            rewritten,
            CONF.replace("repo.crownos.org/v0.4.0", "repo.crownos.org/v0.4.1")
        );
        assert_eq!(installed_channel(&rewritten), Some("v0.4.1".parse()?));
        Ok(())
    }

    #[test]
    fn a_conf_without_the_pin_is_refused() {
        let unpinned = CONF.replace("Server = https://repo.crownos.org/v0.4.0/$arch\n", "");
        assert!(with_channel(&unpinned, Channel::Testing).is_err());
    }
}
