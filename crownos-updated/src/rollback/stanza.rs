use update_manifest::ReleaseVersion;
use update_proto::SnapperNumber;

const ROLLBACK_DIRECTORY: &str = "/EFI/CrownOS/rollback";
const KERNEL: &str = "vmlinuz-linux-cachyos";
const INITRAMFS: &str = "initramfs-linux-cachyos.img";
const RECOVER_PARAMETER: &str = "crownos.recover";

/// The entry file without an entry, for when the kept kernel pair may be gone.
pub const NO_ROLLBACK_STANZA: &str = "# Written by crownos-updated before each update.\n";

/// The rEFInd entry that boots the kept kernel pair into snapshot `number`.
/// rEFInd reads `""` as one quote inside a quoted value.
#[must_use]
pub fn rollback_stanza(
    from: &ReleaseVersion,
    kernel_cmdline: &str,
    number: SnapperNumber,
) -> String {
    let cmdline = kernel_cmdline
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace('"', "\"\"");
    format!(
        "menuentry \"Rollback to CrownOS {from}\" {{\n    \
         loader {ROLLBACK_DIRECTORY}/{KERNEL}\n    \
         initrd {ROLLBACK_DIRECTORY}/{INITRAMFS}\n    \
         options \"{cmdline} {RECOVER_PARAMETER}={number}\"\n}}\n"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_the_release_kernel_pair_and_snapshot() -> Result<(), update_manifest::VersionError> {
        let stanza = rollback_stanza(
            &"0.4.0".parse()?,
            "root=PARTUUID=abc rw quiet\n",
            SnapperNumber(7),
        );
        assert_eq!(
            stanza,
            "menuentry \"Rollback to CrownOS 0.4.0\" {\n    \
             loader /EFI/CrownOS/rollback/vmlinuz-linux-cachyos\n    \
             initrd /EFI/CrownOS/rollback/initramfs-linux-cachyos.img\n    \
             options \"root=PARTUUID=abc rw quiet crownos.recover=7\"\n}\n"
        );
        Ok(())
    }

    #[test]
    fn quotes_are_doubled_and_whitespace_is_joined() -> Result<(), update_manifest::VersionError> {
        let stanza = rollback_stanza(
            &"0.4.0".parse()?,
            "root=PARTUUID=abc\t rw\n  acpi_osi=\"Windows 2020\"\n",
            SnapperNumber(3),
        );
        assert!(
            stanza.contains(
                "options \"root=PARTUUID=abc rw acpi_osi=\"\"Windows 2020\"\" crownos.recover=3\"\n"
            ),
            "{stanza}"
        );
        Ok(())
    }
}
