//! Signs a manifest with a throwaway key and checks it with the real gpg.

use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
};

use update_manifest::{Channel, Keyring, PublishedFileError, VerifyError, fetch_verified};

type TestResult = Result<(), Box<dyn std::error::Error>>;

const MANIFEST: &str = "version = \"0.4.1\"\nbuild_id = \"x\"\nchannel = \"stable\"\n\
    releases = [\"0.4.0\", \"0.4.1\"]\npackages = [\"crownbar\"]\n";

fn gpg_available() -> bool {
    Command::new("gpg")
        .arg("--version")
        .output()
        .is_ok_and(|output| output.status.success())
}

fn gpg(homedir: &Path, arguments: &[&str]) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("gpg")
        .args(["--batch", "--no-tty", "--homedir"])
        .arg(homedir)
        .args(arguments)
        .output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into());
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

struct SigningKey {
    homedir: PathBuf,
    fingerprint: String,
}

impl SigningKey {
    fn generate(homedir: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        fs::create_dir_all(homedir)?;
        fs::set_permissions(homedir, fs::Permissions::from_mode(0o700))?;
        gpg(
            homedir,
            &[
                "--passphrase",
                "",
                "--quick-generate-key",
                "CrownOS Test <test@crownos.org>",
                "ed25519",
                "sign",
                "never",
            ],
        )?;
        let listing = gpg(homedir, &["--with-colons", "--list-keys"])?;
        let fingerprint = listing
            .lines()
            .find_map(|line| line.strip_prefix("fpr:::::::::"))
            .and_then(|rest| rest.strip_suffix(':'))
            .ok_or("no fingerprint listed")?
            .to_owned();
        Ok(Self {
            homedir: homedir.to_owned(),
            fingerprint,
        })
    }

    fn sign(&self, file: &Path) -> Result<(), Box<dyn std::error::Error>> {
        let signature = format!("{}.sig", file.display());
        let file = file.to_string_lossy();
        gpg(
            &self.homedir,
            &["--detach-sign", "--output", &signature, &file],
        )?;
        Ok(())
    }
}

impl Drop for SigningKey {
    fn drop(&mut self) {
        let _ = Command::new("gpgconf")
            .arg("--homedir")
            .arg(&self.homedir)
            .args(["--kill", "all"])
            .output();
    }
}

fn keyring(root: &Path, key: &SigningKey, trusted: &str) -> Result<Keyring, std::io::Error> {
    fs::write(root.join("trusted"), format!("{trusted}:4:\n"))?;
    Ok(Keyring {
        gpg: PathBuf::from("gpg"),
        homedir: key.homedir.clone(),
        trusted_keys: root.join("trusted"),
        revoked_keys: root.join("revoked"),
    })
}

#[test]
fn real_gpg_verifies_pins_and_rejects_tampering() -> TestResult {
    if !gpg_available() {
        eprintln!("gpg is not installed; skipping");
        return Ok(());
    }
    let root = tempfile::Builder::new().prefix("cos").tempdir_in("/tmp")?;
    let key = SigningKey::generate(&root.path().join("gnupg"))?;
    let channel_directory = root.path().join("repo/stable");
    fs::create_dir_all(&channel_directory)?;
    let manifest_file = channel_directory.join("manifest.toml");
    fs::write(&manifest_file, MANIFEST)?;
    key.sign(&manifest_file)?;
    let repository = format!("file://{}", root.path().join("repo").display());

    let trusted = keyring(root.path(), &key, &key.fingerprint)?;
    let verified = fetch_verified(&repository, Channel::Stable, &trusted)?;
    assert_eq!(verified.bytes, MANIFEST.as_bytes());

    fs::write(&manifest_file, MANIFEST.replace("crownbar", "crowndock"))?;
    assert!(matches!(
        fetch_verified(&repository, Channel::Stable, &trusted),
        Err(PublishedFileError::Verify {
            source: VerifyError::BadSignature,
            ..
        })
    ));

    fs::write(&manifest_file, MANIFEST)?;
    let unlisted = keyring(
        root.path(),
        &key,
        "FEEDFACEFEEDFACEFEEDFACEFEEDFACEFEEDFACE",
    )?;
    assert!(matches!(
        fetch_verified(&repository, Channel::Stable, &unlisted),
        Err(PublishedFileError::Verify {
            source: VerifyError::Untrusted(_),
            ..
        })
    ));
    Ok(())
}
