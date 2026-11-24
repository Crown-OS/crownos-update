//! A fixture root with fake tools and a `file://` repository, and the daemon
//! running against it.

use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    time::Duration,
};

use tokio::time::{Instant, sleep};
use update_client::UpdateClient;
use update_client::proto::Snapshot;

pub type TestResult = Result<(), Box<dyn std::error::Error>>;
type FixtureResult<T> = Result<T, Box<dyn std::error::Error>>;

const FINGERPRINT: &str = "AB12CD34AB12CD34AB12CD34AB12CD34AB12CD34";
const WAIT_LIMIT: Duration = Duration::from_secs(20);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Writing an executable while another thread forks makes the exec fail
/// with "text file busy"; fixtures are written and started one at a time.
static SERIAL: Mutex<()> = Mutex::new(());

pub struct Release<'a> {
    pub version: &'a str,
    pub restart: &'a str,
    pub releases: &'a [&'a str],
    pub packages: &'a [&'a str],
    pub retired: &'a [&'a str],
}

pub struct Options<'a> {
    pub installed_version: &'a str,
    pub installed_packages: &'a [&'a str],
    pub stable: Release<'a>,
    pub next: Option<Release<'a>>,
    /// `Some(status)` installs a post-update script that exits with it.
    pub post_update_exit_status: Option<u8>,
    pub administrator: bool,
    /// Files written under the root after the standard ones.
    pub files: &'a [(&'a str, &'a str)],
}

pub struct Fixture {
    _directory: tempfile::TempDir,
    pub root: PathBuf,
    daemon: Child,
}

impl Options<'_> {
    pub fn chain() -> Self {
        Options {
            installed_version: "0.4.0",
            installed_packages: &["crownbar"],
            stable: Release {
                version: "0.4.2",
                restart: "reboot",
                releases: &["0.4.0", "0.4.1", "0.4.2"],
                packages: &["crownbar", "crowndock"],
                retired: &[],
            },
            next: Some(Release {
                version: "0.4.1",
                restart: "soft-reboot",
                releases: &["0.4.0", "0.4.1"],
                packages: &["crownbar", "crowndock"],
                retired: &[],
            }),
            post_update_exit_status: None,
            administrator: true,
            files: &[],
        }
    }
}

impl Fixture {
    pub fn start(options: &Options<'_>) -> FixtureResult<Self> {
        let _serial = SERIAL
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let directory = tempfile::Builder::new().prefix("cu").tempdir_in("/tmp")?;
        let root = directory.path().to_owned();
        write_root(&root, options)?;
        write_repository(&root, options)?;
        write_tools(&root)?;
        let daemon = Command::new(env!("CARGO_BIN_EXE_crownos-updated"))
            .env("CROWNOS_UPDATE_ROOT", &root)
            .env("CROWNOS_UPDATE_TOOLS", root.join("tools"))
            .env(
                "CROWNOS_REPOSITORY_URL",
                format!("file://{}", root.join("repo").display()),
            )
            .env("RUST_LOG", "debug")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(fs::File::create(root.join("daemon.log"))?)
            .spawn()?;
        Ok(Self {
            _directory: directory,
            root,
            daemon,
        })
    }

    pub async fn connect(&self) -> FixtureResult<UpdateClient> {
        let deadline = Instant::now() + WAIT_LIMIT;
        loop {
            match UpdateClient::connect_in(&self.root.join("run/crownos")).await {
                Ok(client) => return Ok(client),
                Err(error) if Instant::now() > deadline => return Err(error.into()),
                Err(_) => sleep(POLL_INTERVAL).await,
            }
        }
    }

    /// Lines the fake tools and the post-update script wrote, with the
    /// fixture root shortened to `ROOT`.
    pub fn commands(&self) -> Vec<String> {
        let root = self.root.display().to_string();
        fs::read_to_string(self.root.join("commands.log"))
            .unwrap_or_default()
            .lines()
            .map(|line| line.replace(&root, "ROOT"))
            .collect()
    }

    pub fn read(&self, path: &str) -> String {
        fs::read_to_string(self.root.join(path)).unwrap_or_default()
    }

    pub fn exists(&self, path: &str) -> bool {
        self.root.join(path).exists()
    }

    /// Makes the fake tool behind `knob` fail, or misbehave, from now on.
    pub fn fail(&self, knob: &str) -> std::io::Result<()> {
        fs::write(self.root.join("fail").join(knob), "")
    }

    pub fn repair(&self, knob: &str) -> std::io::Result<()> {
        fs::remove_file(self.root.join("fail").join(knob))
    }

    pub fn remove(&self, path: &str) -> std::io::Result<()> {
        fs::remove_file(self.root.join(path))
    }

    /// Sends SIGTERM to the daemon and waits until it exits.
    pub async fn terminate(&mut self) -> FixtureResult<std::process::ExitStatus> {
        let status = Command::new("kill")
            .args(["-TERM", &self.daemon.id().to_string()])
            .status()?;
        if !status.success() {
            return Err("kill failed".into());
        }
        let deadline = Instant::now() + WAIT_LIMIT;
        loop {
            if let Some(status) = self.daemon.try_wait()? {
                return Ok(status);
            }
            if Instant::now() > deadline {
                return Err("the daemon did not stop".into());
            }
            sleep(POLL_INTERVAL).await;
        }
    }

    pub fn daemon_log(&self) -> String {
        self.read("daemon.log")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.daemon.kill();
        let _ = self.daemon.wait();
    }
}

/// Polls the state until `done` holds.
pub async fn wait_for(
    client: &mut UpdateClient,
    done: impl Fn(&Snapshot) -> bool,
) -> FixtureResult<Snapshot> {
    let deadline = Instant::now() + WAIT_LIMIT;
    loop {
        let snapshot = client.state().await?;
        if done(&snapshot) {
            return Ok(snapshot);
        }
        if Instant::now() > deadline {
            return Err(format!("timed out in {snapshot:#?}").into());
        }
        sleep(POLL_INTERVAL).await;
    }
}

fn write_file(root: &Path, path: &str, contents: &str) -> std::io::Result<()> {
    let path = root.join(path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, contents)
}

fn write_script(path: &Path, body: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, format!("#!/bin/sh\n{body}"))?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

fn write_root(root: &Path, options: &Options<'_>) -> FixtureResult<()> {
    let metadata = fs::metadata(root)?;
    let wheel_members = if options.administrator { "tester" } else { "" };
    write_file(
        root,
        "etc/os-release",
        &format!("NAME=CrownOS\nVERSION_ID={}\n", options.installed_version),
    )?;
    write_file(
        root,
        "etc/pacman.conf",
        &format!(
            "[options]\nArchitecture = auto\n\n[crownos]\nSigLevel = Required\nServer = https://repo.crownos.org/v{}/$arch\n\n[core]\nInclude = /etc/pacman.d/mirrorlist\n",
            options.installed_version
        ),
    )?;
    write_file(
        root,
        "etc/passwd",
        &format!(
            "tester:x:{}:{}::/home/tester:/bin/sh\n",
            metadata.uid(),
            metadata.gid()
        ),
    )?;
    write_file(
        root,
        "etc/group",
        &format!("wheel:x:4242:{wheel_members}\n"),
    )?;
    write_file(root, "etc/kernel/cmdline", "root=PARTUUID=abc rw quiet\n")?;
    write_file(root, "proc/sys/kernel/random/boot_id", "boot-1\n")?;
    write_file(
        root,
        "usr/share/pacman/keyrings/crownos-trusted",
        &format!("{FINGERPRINT}:4:\n"),
    )?;
    write_file(root, "installed", &options.installed_packages.join("\n"))?;
    write_file(
        root,
        "snapper-list.json",
        r#"{"root": [{"number": 0, "userdata": null}, {"number": 3, "userdata": {"crownos-rollback": "0.4.0"}}, {"number": 5, "userdata": {"important": "yes"}}, {"number": 7, "userdata": {"crownos-rollback": "0.4.1"}}]}"#,
    )?;
    fs::create_dir_all(root.join("efi/EFI/refind"))?;
    fs::create_dir_all(root.join("fail"))?;
    for (path, contents) in options.files {
        write_file(root, path, contents)?;
    }
    let Some(exit_status) = options.post_update_exit_status else {
        return Ok(());
    };
    write_script(
        &root.join("usr/lib/crownos/post-update"),
        &format!(
            "echo \"post-update $CROWNOS_FROM_VERSION $CROWNOS_TO_VERSION\" >> '{}'\nexit {exit_status}\n",
            root.join("commands.log").display()
        ),
    )?;
    Ok(())
}

fn manifest(release: &Release<'_>) -> String {
    let quoted = |items: &[&str]| {
        items
            .iter()
            .map(|item| format!("\"{item}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    format!(
        "version = \"{}\"\nbuild_id = \"fixture\"\nchannel = \"stable\"\nrestart = \"{}\"\nnotes = \"notes.md\"\nreleases = [{}]\npackages = [{}]\nretired = [{}]\n",
        release.version,
        release.restart,
        quoted(release.releases),
        quoted(release.packages),
        quoted(release.retired)
    )
}

fn write_repository(root: &Path, options: &Options<'_>) -> std::io::Result<()> {
    let channels = std::iter::once(("stable".to_owned(), &options.stable)).chain(
        options
            .next
            .iter()
            .map(|next| (format!("v{}", next.version), next)),
    );
    for (channel, release) in channels {
        for (file, contents) in [
            ("manifest.toml", manifest(release)),
            (
                "notes.md",
                format!("## CrownOS {}\n\nFaster updates.\n", release.version),
            ),
        ] {
            write_file(root, &format!("repo/{channel}/{file}"), &contents)?;
            write_file(root, &format!("repo/{channel}/{file}.sig"), "signature")?;
        }
    }
    Ok(())
}

fn write_tools(root: &Path) -> std::io::Result<()> {
    let log = root.join("commands.log");
    let log = log.display();
    let root_text = root.display();
    let status = format!(
        "[GNUPG:] GOODSIG 1 CrownOS\n[GNUPG:] VALIDSIG {FINGERPRINT} 2026 1 0 4 0 22 10 00 {FINGERPRINT}\n"
    );
    let tools = root.join("tools");
    write_script(
        &tools.join("gpg"),
        &format!(
            "cat >/dev/null\n\
             if [ -e '{root_text}/fail/gpg' ]; then echo '[GNUPG:] BADSIG 1 CrownOS'; exit 1; fi\n\
             printf '%s' '{status}'\n"
        ),
    )?;
    write_script(
        &tools.join("pacman"),
        &format!(
            "echo \"pacman $*\" >> '{log}'\n\
             case \"$1\" in\n\
             -Qq) cat '{root_text}/installed' ;;\n\
             -Q) exit 1 ;;\n\
             -Syuw)\n\
               if [ -e '{root_text}/fail/slow-download' ]; then sleep 1; fi\n\
               echo 'downloading crowndock 100%' ;;\n\
             -Syu)\n\
               grep '^Server' '{root_text}/etc/pacman.conf' | sed 's/^/pacman sees /' >> '{log}'\n\
               if [ -e '{root_text}/fail/install' ]; then echo 'error: failed to commit transaction' >&2; exit 1; fi\n\
               if [ -e '{root_text}/fail/slow-install' ]; then sleep 1; fi\n\
               echo '(1/2) upgrading crownbar'; echo '(2/2) installing crowndock' ;;\n\
             esac\n"
        ),
    )?;
    write_script(
        &tools.join("snapper"),
        &format!(
            "echo \"snapper $*\" >> '{log}'\n\
             case \"$*\" in\n\
             *' create '*) echo 7 ;;\n\
             *'--jsonout list'*) cat '{root_text}/snapper-list.json' ;;\n\
             esac\n"
        ),
    )?;
    write_script(
        &tools.join("crownos-esp-kernel"),
        &format!(
            "echo \"crownos-esp-kernel $*\" >> '{log}'\n\
             if [ -e '{root_text}/fail/esp-kernel' ]; then echo 'no kernel on the ESP' >&2; exit 1; fi\n"
        ),
    )?;
    write_script(
        &tools.join("systemctl"),
        &format!(
            "echo \"systemctl $*\" >> '{log}'\n\
             if [ \"$1\" = show ]; then echo 0; fi\n\
             if [ \"$1\" = preset ] && [ -e '{root_text}/fail/preset' ]; then echo 'Failed to preset unit' >&2; exit 1; fi\n"
        ),
    )
}
