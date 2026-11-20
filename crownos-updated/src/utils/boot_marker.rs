use serde::{Deserialize, Serialize};
use tokio::process::Command;

use crate::{paths::Paths, process::capture};

/// Which boot of the machine this is. A reboot changes `boot_id`; a soft
/// reboot keeps it and increments `soft_reboots`.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
pub struct BootMarker {
    pub boot_id: Option<String>,
    pub soft_reboots: Option<u32>,
}

impl BootMarker {
    pub async fn current(paths: &Paths) -> Self {
        let boot_id = tokio::fs::read_to_string(&paths.boot_id)
            .await
            .ok()
            .map(|text| text.trim().to_owned())
            .filter(|id| !id.is_empty());
        let soft_reboots = capture(Command::new(&paths.tools.systemctl).args([
            "show",
            "--property=SoftRebootsCount",
            "--value",
        ]))
        .await
        .ok()
        .and_then(|text| text.trim().parse().ok());
        Self {
            boot_id,
            soft_reboots,
        }
    }

    /// The machine started again after `earlier` was taken.
    #[must_use]
    pub fn restarted_since(&self, earlier: &Self) -> bool {
        let rebooted =
            matches!((&self.boot_id, &earlier.boot_id), (Some(now), Some(then)) if now != then);
        let soft_rebooted = matches!((self.soft_reboots, earlier.soft_reboots), (Some(now), Some(then)) if now > then);
        rebooted || soft_rebooted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marker(boot_id: &str, soft_reboots: Option<u32>) -> BootMarker {
        BootMarker {
            boot_id: Some(boot_id.to_owned()),
            soft_reboots,
        }
    }

    #[test]
    fn a_new_boot_id_or_more_soft_reboots_is_a_restart() {
        let applied = marker("a", Some(0));
        assert!(!marker("a", Some(0)).restarted_since(&applied));
        assert!(marker("b", Some(0)).restarted_since(&applied));
        assert!(marker("a", Some(1)).restarted_since(&applied));
        assert!(!marker("a", None).restarted_since(&applied));
        assert!(!BootMarker::default().restarted_since(&applied));
    }
}
