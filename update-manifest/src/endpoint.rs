use crate::Channel;

pub const REPOSITORY_URL: &str = "https://repo.crownos.org";
pub const MANIFEST_FILE: &str = "manifest.toml";

pub fn channel_file_url(repository_url: &str, channel: Channel, file: &str) -> String {
    format!("{}/{channel}/{file}", repository_url.trim_end_matches('/'))
}

pub fn signature_url(file_url: &str) -> String {
    format!("{file_url}.sig")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_follow_the_published_layout() -> Result<(), crate::ChannelError> {
        let manifest = channel_file_url(
            "https://repo.crownos.org/",
            "v0.4.1".parse()?,
            MANIFEST_FILE,
        );
        assert_eq!(manifest, "https://repo.crownos.org/v0.4.1/manifest.toml");
        assert_eq!(
            signature_url(&manifest),
            "https://repo.crownos.org/v0.4.1/manifest.toml.sig"
        );
        Ok(())
    }
}
