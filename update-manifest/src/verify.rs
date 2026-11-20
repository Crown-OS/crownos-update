use std::{
    collections::HashSet,
    ffi::OsString,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use tempfile::NamedTempFile;
use thiserror::Error;

const STATUS_PREFIX: &str = "[GNUPG:] ";

/// The pacman keyring files that pin the CrownOS signing keys.
#[derive(Clone, Debug)]
pub struct Keyring {
    pub gpg: PathBuf,
    pub homedir: PathBuf,
    pub trusted_keys: PathBuf,
    pub revoked_keys: PathBuf,
}

impl Keyring {
    pub const GPG: &str = "/usr/bin/gpg";
    pub const HOMEDIR: &str = "/etc/pacman.d/gnupg";
    pub const TRUSTED_KEYS: &str = "/usr/share/pacman/keyrings/crownos-trusted";
    pub const REVOKED_KEYS: &str = "/usr/share/pacman/keyrings/crownos-revoked";
}

impl Default for Keyring {
    fn default() -> Self {
        Self {
            gpg: Self::GPG.into(),
            homedir: Self::HOMEDIR.into(),
            trusted_keys: Self::TRUSTED_KEYS.into(),
            revoked_keys: Self::REVOKED_KEYS.into(),
        }
    }
}

#[derive(Debug, Error)]
pub enum VerifyError {
    #[error("cannot run gpg")]
    Gpg(#[source] io::Error),
    #[error("gpg failed with status {0:?}")]
    GpgStatus(Option<i32>),
    #[error(
        "the CrownOS keyring is not installed; install crownos-keyring and run pacman-key --populate crownos"
    )]
    UnknownKey,
    #[error("the signature does not match the file")]
    BadSignature,
    #[error("the file was signed with an expired key or signature")]
    Expired,
    #[error("the file was signed with a revoked key")]
    RevokedKey,
    #[error("the file carries no valid signature")]
    Unsigned,
    #[error("{0} lists no trusted CrownOS signing key")]
    NoTrustedKeys(PathBuf),
    #[error("cannot read the trusted keys {path}")]
    TrustStore {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("the file was signed by {0}, which is not a CrownOS signing key")]
    Untrusted(String),
}

/// `trust-model always` keeps this check independent of pacman's trustdb; the
/// signer is pinned to the CrownOS fingerprints instead.
pub fn gpg_arguments(keyring: &Keyring, signature: &Path) -> Vec<OsString> {
    let mut arguments: Vec<OsString> = [
        "--batch",
        "--no-tty",
        "--no-autostart",
        "--no-permission-warning",
        "--trust-model",
        "always",
        "--homedir",
    ]
    .map(OsString::from)
    .into();
    arguments.push(keyring.homedir.clone().into());
    arguments.extend(["--status-fd", "1", "--verify"].map(OsString::from));
    arguments.push(signature.into());
    arguments.push("-".into());
    arguments
}

/// The primary fingerprint of the key that validly signed, from gpg's `--status-fd` output.
pub fn signing_fingerprint(status: &str) -> Result<&str, VerifyError> {
    let keywords = || {
        status
            .lines()
            .filter_map(|line| line.strip_prefix(STATUS_PREFIX))
    };
    for line in keywords() {
        match line.split_whitespace().next() {
            Some("BADSIG") => return Err(VerifyError::BadSignature),
            Some("EXPKEYSIG" | "EXPSIG") => return Err(VerifyError::Expired),
            Some("REVKEYSIG") => return Err(VerifyError::RevokedKey),
            Some("NO_PUBKEY" | "ERRSIG") => return Err(VerifyError::UnknownKey),
            _ => {}
        }
    }
    let good = keywords().any(|line| line.starts_with("GOODSIG "));
    let primary = keywords()
        .find(|line| line.starts_with("VALIDSIG "))
        .and_then(|line| line.split_whitespace().last());
    match primary {
        Some(fingerprint) if good => Ok(fingerprint),
        _ => Err(VerifyError::Unsigned),
    }
}

/// The fingerprints of a pacman keyring list: `FPR:level:` lines, or bare ones.
pub fn trusted_fingerprints(text: &str) -> HashSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter_map(|line| line.split(':').next())
        .map(|fingerprint| fingerprint.trim().to_ascii_uppercase())
        .collect()
}

fn read_fingerprints(path: &Path) -> Result<HashSet<String>, VerifyError> {
    fs::read_to_string(path)
        .map(|text| trusted_fingerprints(&text))
        .map_err(|source| VerifyError::TrustStore {
            path: path.to_owned(),
            source,
        })
}

fn require_pinned(
    fingerprint: &str,
    trusted: &HashSet<String>,
    revoked: &HashSet<String>,
) -> Result<(), VerifyError> {
    let fingerprint = fingerprint.to_ascii_uppercase();
    if revoked.contains(&fingerprint) {
        return Err(VerifyError::RevokedKey);
    }
    if trusted.contains(&fingerprint) {
        Ok(())
    } else {
        Err(VerifyError::Untrusted(fingerprint))
    }
}

fn run_gpg(keyring: &Keyring, file: &[u8], signature: &Path) -> Result<String, VerifyError> {
    let mut child = Command::new(&keyring.gpg)
        .args(gpg_arguments(keyring, signature))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(VerifyError::Gpg)?;
    if let Some(mut stdin) = child.stdin.take() {
        match stdin.write_all(file) {
            Err(error) if error.kind() != io::ErrorKind::BrokenPipe => {
                return Err(VerifyError::Gpg(error));
            }
            _ => {}
        }
    }
    let output = child.wait_with_output().map_err(VerifyError::Gpg)?;
    let status = String::from_utf8_lossy(&output.stdout).into_owned();
    signing_fingerprint(&status)?;
    if output.status.success() {
        Ok(status)
    } else {
        Err(VerifyError::GpgStatus(output.status.code()))
    }
}

/// Blocking: runs gpg.
pub fn verify_detached(
    keyring: &Keyring,
    file: &[u8],
    signature: &[u8],
) -> Result<(), VerifyError> {
    let trusted = read_fingerprints(&keyring.trusted_keys)?;
    if trusted.is_empty() {
        return Err(VerifyError::NoTrustedKeys(keyring.trusted_keys.clone()));
    }
    let revoked = read_fingerprints(&keyring.revoked_keys).unwrap_or_default();
    let mut signature_file = NamedTempFile::new().map_err(VerifyError::Gpg)?;
    signature_file
        .write_all(signature)
        .map_err(VerifyError::Gpg)?;
    let status = run_gpg(keyring, file, signature_file.path())?;
    require_pinned(signing_fingerprint(&status)?, &trusted, &revoked)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FINGERPRINT: &str = "AB12CD34AB12CD34AB12CD34AB12CD34AB12CD34";

    fn good_status(fingerprint: &str) -> String {
        format!(
            "[GNUPG:] NEWSIG\n[GNUPG:] KEY_CONSIDERED {fingerprint} 0\n\
             [GNUPG:] SIG_ID abc 2026-01-01 1767225600\n\
             [GNUPG:] GOODSIG 12CD34AB12CD34AB CrownOS Packages <packages@crownos.org>\n\
             [GNUPG:] VALIDSIG {fingerprint} 2026-01-01 1767225600 0 4 0 22 10 00 {fingerprint}\n\
             [GNUPG:] TRUST_UNDEFINED 0 pgp\n"
        )
    }

    #[test]
    fn a_good_signature_yields_the_primary_fingerprint() {
        let status = good_status(FINGERPRINT);
        assert_eq!(signing_fingerprint(&status).ok(), Some(FINGERPRINT));
    }

    #[test]
    fn a_subkey_signature_yields_the_primary_fingerprint() {
        let status = format!(
            "[GNUPG:] GOODSIG 1 x\n[GNUPG:] VALIDSIG SUBKEY 2026-01-01 1 0 4 0 22 10 00 {FINGERPRINT}\n"
        );
        assert_eq!(signing_fingerprint(&status).ok(), Some(FINGERPRINT));
    }

    fn failure(status: &str) -> Option<VerifyError> {
        signing_fingerprint(status).err()
    }

    #[test]
    fn failures_are_told_apart() {
        assert!(matches!(
            failure("[GNUPG:] BADSIG 1 x\n"),
            Some(VerifyError::BadSignature)
        ));
        assert!(matches!(
            failure("[GNUPG:] EXPKEYSIG 1 x\n"),
            Some(VerifyError::Expired)
        ));
        assert!(matches!(
            failure("[GNUPG:] REVKEYSIG 1 x\n"),
            Some(VerifyError::RevokedKey)
        ));
        assert!(matches!(
            failure("[GNUPG:] ERRSIG 1 22 10 00 1 9\n[GNUPG:] NO_PUBKEY 1\n"),
            Some(VerifyError::UnknownKey)
        ));
        assert!(matches!(failure(""), Some(VerifyError::Unsigned)));
    }

    #[test]
    fn a_valid_signature_without_a_good_one_is_unsigned() {
        let status = format!("[GNUPG:] VALIDSIG {FINGERPRINT} x 1 0 4 0 22 10 00 {FINGERPRINT}\n");
        assert!(matches!(
            signing_fingerprint(&status),
            Err(VerifyError::Unsigned)
        ));
    }

    #[test]
    fn keyring_lists_parse_with_blank_comment_and_lower_case_lines() {
        let list = "# CrownOS\n\n  ab12cd34ab12cd34ab12cd34ab12cd34ab12cd34:4:  \nFEEDFACE:4:\n";
        let parsed = trusted_fingerprints(list);
        assert_eq!(parsed.len(), 2);
        assert!(parsed.contains(FINGERPRINT));
        assert!(trusted_fingerprints("# nothing\n\n").is_empty());
    }

    #[test]
    fn only_trusted_and_unrevoked_keys_are_pinned() {
        let trusted: HashSet<String> = [FINGERPRINT.to_owned(), "FEEDFACE".to_owned()].into();
        let revoked: HashSet<String> = ["FEEDFACE".to_owned()].into();
        assert!(require_pinned(&FINGERPRINT.to_lowercase(), &trusted, &revoked).is_ok());
        assert!(matches!(
            require_pinned("FEEDFACE", &trusted, &revoked),
            Err(VerifyError::RevokedKey)
        ));
        assert!(matches!(
            require_pinned("0000", &trusted, &revoked),
            Err(VerifyError::Untrusted(_))
        ));
    }

    #[test]
    fn gpg_is_told_the_homedir_and_reads_the_file_from_stdin() {
        let arguments = gpg_arguments(&Keyring::default(), Path::new("/tmp/sig"));
        let text: Vec<_> = arguments.iter().map(|a| a.to_string_lossy()).collect();
        assert!(
            text.windows(2)
                .any(|pair| pair[0] == "--homedir" && pair[1] == "/etc/pacman.d/gnupg")
        );
        assert!(
            text.windows(2)
                .any(|pair| pair[0] == "--trust-model" && pair[1] == "always")
        );
        assert_eq!(text[text.len() - 2..], ["/tmp/sig", "-"]);
    }
}
