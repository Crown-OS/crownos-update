use serde::{Deserialize, Serialize};

/// What a release needs after its packages are installed, from
/// `releases/<version>/release.toml` in crownos-iso.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestartKind {
    Session,
    SoftReboot,
    #[default]
    Reboot,
}

impl RestartKind {
    #[must_use]
    pub fn button_label(self) -> &'static str {
        match self {
            Self::Session => "Restart desktop",
            Self::SoftReboot => "Quick restart",
            Self::Reboot => "Restart computer",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Deserialize)]
    struct Release {
        restart: RestartKind,
    }

    #[test]
    fn reads_the_release_toml_spelling() -> Result<(), toml::de::Error> {
        for (text, kind) in [
            ("session", RestartKind::Session),
            ("soft-reboot", RestartKind::SoftReboot),
            ("reboot", RestartKind::Reboot),
        ] {
            let release: Release = toml::from_str(&format!("restart = \"{text}\""))?;
            assert_eq!(release.restart, kind);
        }
        assert!(toml::from_str::<Release>("restart = \"shutdown\"").is_err());
        Ok(())
    }
}
