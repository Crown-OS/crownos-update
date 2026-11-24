//! A release can add an `enable` line to the CrownOS preset file. The update
//! enables only the units of lines that are new since the last applied
//! release, so a unit that the user disabled stays disabled.

use std::{collections::HashSet, io, path::Path};

use anyhow::Context;
use tokio::{fs, process::Command};
use update_proto::{ApplyStep, Phase};

use super::Engine;
use crate::{
    process::stream,
    utils::{
        atomic_write::write_atomically,
        preset::{enabled_units, unit_file_name},
    },
};

/// The units that the installer of 0.3.0 and earlier enabled. Those systems
/// have no record of applied preset lines.
const ENABLED_BEFORE_THE_PRESET_FILE: &[&str] = &[
    "NetworkManager.service",
    "systemd-timesyncd.service",
    "power-profiles-daemon.service",
    "snapper-cleanup.timer",
    "crownos-updated.service",
];

impl Engine {
    /// Runs `systemctl preset` for the new units of the preset file, then
    /// records every unit of the file as applied. A new unit without a unit
    /// file belongs to a package the user removed and is only recorded.
    pub(super) async fn enable_new_preset_units(&self) -> anyhow::Result<()> {
        let paths = &self.paths;
        let Some(preset) = read_if_present(&paths.crownos_preset).await? else {
            return Ok(());
        };
        let record = read_if_present(&paths.applied_presets).await?;
        let applied: HashSet<&str> = match &record {
            Some(record) => record.lines().map(str::trim).collect(),
            None => ENABLED_BEFORE_THE_PRESET_FILE.iter().copied().collect(),
        };
        let mut new_units = Vec::new();
        for unit in enabled_units(&preset).filter(|unit| !applied.contains(unit)) {
            if self.has_unit_file(unit).await {
                new_units.push(unit);
            } else {
                tracing::info!("not enabling {unit}: it has no unit file");
            }
        }
        if !new_units.is_empty() {
            let mut command = Command::new(&paths.tools.systemctl);
            command.args(["preset", "--"]).args(&new_units);
            stream(&mut command, |line| {
                self.published
                    .line(Phase::Applying(ApplyStep::PostUpdate), line)
            })
            .await?;
        }
        let mut applied_record: String = enabled_units(&preset).collect::<Vec<_>>().join("\n");
        applied_record.push('\n');
        if record.as_deref() == Some(applied_record.as_str()) {
            return Ok(());
        }
        if let Some(directory) = paths.applied_presets.parent() {
            fs::create_dir_all(directory)
                .await
                .with_context(|| format!("cannot create {}", directory.display()))?;
        }
        write_atomically(&paths.applied_presets, applied_record.as_bytes())
            .await
            .with_context(|| format!("cannot write {}", paths.applied_presets.display()))
    }

    async fn has_unit_file(&self, unit: &str) -> bool {
        let file_name = unit_file_name(unit);
        for directory in &self.paths.unit_directories {
            if fs::try_exists(directory.join(file_name.as_ref()))
                .await
                .unwrap_or(false)
            {
                return true;
            }
        }
        false
    }
}

async fn read_if_present(path: &Path) -> anyhow::Result<Option<String>> {
    match fs::read_to_string(path).await {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}
