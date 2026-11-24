use std::{
    env,
    path::{Path, PathBuf},
};

use update_manifest::{Keyring, endpoint::REPOSITORY_URL};
use update_proto::SOCKET_DIR;

const ROOT_OVERRIDE: &str = "CROWNOS_UPDATE_ROOT";
const TOOLS_OVERRIDE: &str = "CROWNOS_UPDATE_TOOLS";
const REPOSITORY_OVERRIDE: &str = "CROWNOS_REPOSITORY_URL";
const SYSTEM_TOOLS: &str = "/usr/bin";

/// Every file the daemon reads or writes and every program it runs, fixed at
/// startup. `CROWNOS_UPDATE_ROOT` prefixes each file and
/// `CROWNOS_UPDATE_TOOLS` replaces the tools directory, so a test or a
/// rehearsal runs against a fixture.
#[derive(Clone, Debug)]
pub struct Paths {
    pub repository_url: String,
    pub socket_directory: PathBuf,
    pub runtime_directory: PathBuf,
    pub state_directory: PathBuf,
    pub os_release: PathBuf,
    pub pacman_conf: PathBuf,
    pub override_pacman_conf: PathBuf,
    pub kernel_cmdline: PathBuf,
    pub rollback_stanza: PathBuf,
    pub post_update: PathBuf,
    pub crownos_preset: PathBuf,
    pub applied_presets: PathBuf,
    pub unit_directories: [PathBuf; 2],
    pub boot_id: PathBuf,
    pub passwd: PathBuf,
    pub group: PathBuf,
    pub keyring: Keyring,
    pub tools: Tools,
}

#[derive(Clone, Debug)]
pub struct Tools {
    pub pacman: PathBuf,
    pub snapper: PathBuf,
    pub systemctl: PathBuf,
    pub esp_kernel: PathBuf,
}

impl Paths {
    pub fn from_environment() -> Self {
        let root = env::var_os(ROOT_OVERRIDE).map_or_else(|| PathBuf::from("/"), PathBuf::from);
        let tools =
            env::var_os(TOOLS_OVERRIDE).map_or_else(|| PathBuf::from(SYSTEM_TOOLS), PathBuf::from);
        let repository_url =
            env::var(REPOSITORY_OVERRIDE).unwrap_or_else(|_| REPOSITORY_URL.to_owned());
        Self::new(&root, &tools, repository_url)
    }

    pub fn new(root: &Path, tools: &Path, repository_url: String) -> Self {
        let file = |absolute: &str| root.join(absolute.trim_start_matches('/'));
        Self {
            repository_url,
            socket_directory: file(SOCKET_DIR),
            runtime_directory: file("/run/crownos/update"),
            state_directory: file("/var/lib/crownos/update"),
            os_release: file("/etc/os-release"),
            pacman_conf: file("/etc/pacman.conf"),
            override_pacman_conf: file("/run/crownos/update/pacman.conf"),
            kernel_cmdline: file("/etc/kernel/cmdline"),
            rollback_stanza: file("/efi/EFI/refind/crownos-rollback.conf"),
            post_update: file("/usr/lib/crownos/post-update"),
            crownos_preset: file("/usr/lib/systemd/system-preset/80-crownos.preset"),
            applied_presets: file("/var/lib/crownos/update/applied-presets"),
            unit_directories: [file("/etc/systemd/system"), file("/usr/lib/systemd/system")],
            boot_id: file("/proc/sys/kernel/random/boot_id"),
            passwd: file("/etc/passwd"),
            group: file("/etc/group"),
            keyring: Keyring {
                gpg: tools.join("gpg"),
                homedir: file(Keyring::HOMEDIR),
                trusted_keys: file(Keyring::TRUSTED_KEYS),
                revoked_keys: file(Keyring::REVOKED_KEYS),
            },
            tools: Tools {
                pacman: tools.join("pacman"),
                snapper: tools.join("snapper"),
                systemctl: tools.join("systemctl"),
                esp_kernel: tools.join("crownos-esp-kernel"),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_production_layout_has_no_prefix() {
        let paths = Paths::new(
            Path::new("/"),
            Path::new(SYSTEM_TOOLS),
            REPOSITORY_URL.to_owned(),
        );
        assert_eq!(paths.socket_directory, Path::new("/run/crownos"));
        assert_eq!(
            paths.override_pacman_conf,
            Path::new("/run/crownos/update/pacman.conf")
        );
        assert_eq!(
            paths.rollback_stanza,
            Path::new("/efi/EFI/refind/crownos-rollback.conf")
        );
        assert_eq!(
            paths.tools.esp_kernel,
            Path::new("/usr/bin/crownos-esp-kernel")
        );
        assert_eq!(paths.keyring.gpg, Path::new("/usr/bin/gpg"));
        assert_eq!(
            paths.applied_presets,
            Path::new("/var/lib/crownos/update/applied-presets")
        );
    }

    #[test]
    fn a_fixture_root_prefixes_every_file() {
        let paths = Paths::new(
            Path::new("/tmp/fixture"),
            Path::new("/tmp/tools"),
            String::new(),
        );
        assert_eq!(paths.pacman_conf, Path::new("/tmp/fixture/etc/pacman.conf"));
        assert_eq!(
            paths.keyring.trusted_keys,
            Path::new("/tmp/fixture/usr/share/pacman/keyrings/crownos-trusted")
        );
        assert_eq!(paths.tools.pacman, Path::new("/tmp/tools/pacman"));
    }
}
