use std::borrow::Cow;

/// The units that the `enable` lines of a systemd preset file name, in file
/// order. Blank lines and `#` or `;` comments are skipped.
pub fn enabled_units(preset: &str) -> impl Iterator<Item = &str> {
    preset.lines().filter_map(|line| {
        let mut words = line.split_whitespace();
        match (words.next(), words.next()) {
            (Some("enable"), Some(unit)) => Some(unit),
            _ => None,
        }
    })
}

/// The file that holds `unit`: an instance such as `getty@tty1.service` is
/// read from its template `getty@.service`.
pub fn unit_file_name(unit: &str) -> Cow<'_, str> {
    match (unit.split_once('@'), unit.rsplit_once('.')) {
        (Some((template, _)), Some((_, suffix))) if !unit.contains("@.") => {
            Cow::Owned(format!("{template}@.{suffix}"))
        }
        _ => Cow::Borrowed(unit),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_the_enable_lines() {
        let preset = "# CrownOS\nenable NetworkManager.service\n\n; old\ndisable foo.service\n  enable  snapper-cleanup.timer  \nenable\n";
        assert_eq!(
            enabled_units(preset).collect::<Vec<_>>(),
            ["NetworkManager.service", "snapper-cleanup.timer"]
        );
    }

    #[test]
    fn an_instance_is_read_from_its_template() {
        assert_eq!(unit_file_name("getty@tty1.service"), "getty@.service");
        assert_eq!(unit_file_name("getty@.service"), "getty@.service");
        assert_eq!(
            unit_file_name("crownos-updated.service"),
            "crownos-updated.service"
        );
    }
}
