use std::{fs, path::Path};

/// The `VERSION_ID` value of an os-release file, without its optional quotes.
pub fn version_id(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.trim().strip_prefix("VERSION_ID="))
        .map(|value| value.trim().trim_matches(['"', '\'']))
        .filter(|value| !value.is_empty())
}

pub fn read_version_id(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    version_id(&text).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_quoted_and_unquoted_values() {
        assert_eq!(
            version_id("NAME=CrownOS\nVERSION_ID=0.1.12\n"),
            Some("0.1.12")
        );
        assert_eq!(version_id("VERSION_ID=\"0.1.12\"\n"), Some("0.1.12"));
        assert_eq!(version_id("VERSION_ID='0.1.12'\n"), Some("0.1.12"));
    }

    #[test]
    fn ignores_other_keys_and_stock_arch() {
        assert_eq!(version_id("BUILD_ID=rolling\nNAME=\"Arch Linux\"\n"), None);
        assert_eq!(version_id("NAME=x\nXVERSION_ID=1.2.3\n"), None);
        assert_eq!(version_id("VERSION_ID=\n"), None);
    }
}
