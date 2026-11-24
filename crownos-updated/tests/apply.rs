mod support;

use support::{Fixture, Options, Release, TestResult, wait_for};
use update_client::{
    ClientError, UpdateClient,
    proto::{ApplyStep, DownloadStatus, FailedStep, Failure, RestartKind, SnapperNumber, Snapshot},
};

const OVERRIDE_CONF: &str = "--config ROOT/run/crownos/update/pacman.conf";
const STANZA: &str = "menuentry \"Rollback to CrownOS 0.4.0\" {\n    \
    loader /EFI/CrownOS/rollback/vmlinuz-linux-cachyos\n    \
    initrd /EFI/CrownOS/rollback/initramfs-linux-cachyos.img\n    \
    options \"root=PARTUUID=abc rw quiet crownos.recover=7\"\n}\n";
const APPLIED_MARKER: &str = "var/lib/crownos/update/applied.toml";
const IN_PROGRESS: &str = "var/lib/crownos/update/in-progress.toml";
const ROLLBACK_ENTRY: &str = "efi/EFI/refind/crownos-rollback.conf";
const NO_ROLLBACK_ENTRY: &str = "# Written by crownos-updated before each update.\n";
const SOFT_REBOOTS_QUERY: &str = "systemctl show --property=SoftRebootsCount --value";

fn upgrade_from_the_old_channel() -> Vec<String> {
    before_install()[6..].to_vec()
}

/// Waits until the apply ends in a pending restart or in a failure other
/// than `earlier`, which the state still shows until the apply begins.
async fn wait_for_the_end(
    client: &mut UpdateClient,
    earlier: Option<&Failure>,
) -> Result<Snapshot, Box<dyn std::error::Error>> {
    wait_for(client, |snapshot| {
        snapshot.is_idle()
            && (snapshot.pending_restart.is_some()
                || snapshot.failure.is_some() && snapshot.failure.as_ref() != earlier)
    })
    .await
}

fn before_install() -> Vec<String> {
    [
        "pacman -Qq".to_owned(),
        format!("pacman -Syuw --noconfirm {OVERRIDE_CONF} --needed -- crowndock"),
        "snapper --no-dbus -c root create --type single --print-number --description CrownOS 0.4.0 before update to 0.4.1 --userdata crownos-rollback=0.4.1".to_owned(),
        "crownos-esp-kernel keep".to_owned(),
        "snapper --no-dbus -c root --jsonout list".to_owned(),
        "snapper --no-dbus -c root delete 3".to_owned(),
        format!("pacman -Syu --noconfirm {OVERRIDE_CONF} --needed -- crowndock"),
        "pacman sees Server = https://repo.crownos.org/v0.4.0/$arch".to_owned(),
    ]
    .into()
}

/// Checks, waits for the automatic download, applies and waits for the end.
async fn update(fixture: &Fixture) -> Result<(UpdateClient, Snapshot), Box<dyn std::error::Error>> {
    let mut client = fixture.connect().await?;
    client.check().await?;
    wait_for(&mut client, |snapshot| {
        snapshot.is_idle()
            && snapshot
                .next
                .as_ref()
                .is_some_and(|next| next.download == DownloadStatus::Complete)
    })
    .await?;
    client.apply().await?;
    let snapshot = wait_for_the_end(&mut client, None).await?;
    Ok((client, snapshot))
}

#[tokio::test]
async fn an_update_runs_every_step_in_order_and_restarts_as_asked() -> TestResult {
    let fixture = Fixture::start(&Options {
        post_update_exit_status: Some(0),
        ..Options::chain()
    })?;
    let (mut client, snapshot) = update(&fixture).await?;
    assert!(
        snapshot.failure.is_none(),
        "{:?}\n{}",
        snapshot.failure,
        fixture.daemon_log()
    );

    let mut expected = before_install();
    expected.extend([
        "post-update 0.4.0 0.4.1".to_owned(),
        "systemctl show --property=SoftRebootsCount --value".to_owned(),
    ]);
    assert_eq!(fixture.commands(), expected);
    assert_eq!(fixture.read("efi/EFI/refind/crownos-rollback.conf"), STANZA);
    assert!(
        fixture
            .read("etc/pacman.conf")
            .contains("Server = https://repo.crownos.org/v0.4.1/$arch\n")
    );
    assert!(
        fixture
            .read("etc/pacman.conf")
            .contains("[core]\nInclude = /etc/pacman.d/mirrorlist\n")
    );
    let marker = fixture.read(APPLIED_MARKER);
    assert!(marker.contains("boot_id = \"boot-1\""), "{marker}");
    assert!(marker.contains("soft_reboots = 0"), "{marker}");

    let pending = snapshot.pending_restart.ok_or("no pending restart")?;
    assert_eq!(pending.kind, RestartKind::SoftReboot);
    assert_eq!(
        (pending.from.to_string(), pending.version.to_string()),
        ("0.4.0".into(), "0.4.1".into())
    );
    assert_eq!(
        snapshot
            .installed
            .map(|version| version.to_string())
            .as_deref(),
        Some("0.4.1")
    );
    assert!(snapshot.next.is_none());
    assert!(!fixture.exists("var/lib/crownos/update/next-manifest.toml"));

    client.restart().await?;
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
    while !fixture
        .commands()
        .contains(&"systemctl soft-reboot".to_owned())
    {
        assert!(
            tokio::time::Instant::now() < deadline,
            "{:?}",
            fixture.commands()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    Ok(())
}

#[tokio::test]
async fn a_failed_post_update_leaves_no_applied_marker() -> TestResult {
    let fixture = Fixture::start(&Options {
        post_update_exit_status: Some(1),
        ..Options::chain()
    })?;
    let (_client, snapshot) = update(&fixture).await?;
    let failure = snapshot.failure.ok_or("no failure")?;
    assert_eq!(failure.step, FailedStep::Apply(ApplyStep::PostUpdate));
    let rollback = failure.rollback.ok_or("no rollback")?;
    assert_eq!(rollback.snapshot, SnapperNumber(7));
    assert_eq!(rollback.from.to_string(), "0.4.0");
    assert!(snapshot.pending_restart.is_none());
    assert!(!fixture.exists(APPLIED_MARKER));
    assert_eq!(
        fixture.commands().last().map(String::as_str),
        Some("post-update 0.4.0 0.4.1")
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_transaction_keeps_the_installed_channel() -> TestResult {
    let fixture = Fixture::start(&Options::chain())?;
    fixture.fail("install")?;
    let (_client, snapshot) = update(&fixture).await?;
    let failure = snapshot.failure.ok_or("no failure")?;
    assert_eq!(failure.step, FailedStep::Apply(ApplyStep::Install));
    assert!(
        failure.message.contains("failed to commit transaction"),
        "{}",
        failure.message
    );
    assert_eq!(
        failure.rollback.map(|rollback| rollback.snapshot),
        Some(SnapperNumber(7))
    );
    assert_eq!(fixture.commands(), before_install());
    assert!(fixture.read("etc/pacman.conf").contains("/v0.4.0/$arch"));
    assert!(snapshot.next.is_some());
    assert!(!fixture.exists(APPLIED_MARKER));
    Ok(())
}

#[tokio::test]
async fn a_retry_after_a_failed_install_reuses_the_rollback() -> TestResult {
    let fixture = Fixture::start(&Options::chain())?;
    fixture.fail("install")?;
    let (mut client, snapshot) = update(&fixture).await?;
    let unfinished = snapshot.in_progress.ok_or("no unfinished update")?;
    assert_eq!(unfinished.completed, ApplyStep::KeepKernel);
    assert!(matches!(
        client.check().await,
        Err(ClientError::WrongPhase(_))
    ));
    assert!(matches!(
        client.download().await,
        Err(ClientError::WrongPhase(_))
    ));

    fixture.repair("install")?;
    fixture.remove(ROLLBACK_ENTRY)?;
    client.apply().await?;
    let snapshot = wait_for_the_end(&mut client, snapshot.failure.as_ref()).await?;
    assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
    assert!(snapshot.in_progress.is_none());
    let mut expected = before_install();
    expected.extend(upgrade_from_the_old_channel());
    expected.push(SOFT_REBOOTS_QUERY.to_owned());
    assert_eq!(fixture.commands(), expected);
    assert!(!fixture.exists(ROLLBACK_ENTRY));
    assert!(fixture.exists(APPLIED_MARKER));
    assert!(!fixture.exists(IN_PROGRESS));
    Ok(())
}

#[tokio::test]
async fn a_retry_without_a_record_reuses_the_failed_rollback() -> TestResult {
    let fixture = Fixture::start(&Options {
        files: &[(
            "var/lib/crownos/update/state.toml",
            "[next]\nversion = \"0.4.1\"\nrestart = \"reboot\"\nnew_packages = [\"crowndock\"]\n\
             download = \"complete\"\n\n[failure]\nmessage = \"pacman failed\"\n\n\
             [failure.step]\napply = \"install\"\n\n[failure.rollback]\nsnapshot = 7\nfrom = \"0.4.0\"\n",
        )],
        ..Options::chain()
    })?;
    let mut client = fixture.connect().await?;
    let earlier = client.state().await?.failure;
    client.apply().await?;
    let snapshot = wait_for_the_end(&mut client, earlier.as_ref()).await?;
    assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
    let mut expected = vec!["snapper --no-dbus -c root --jsonout list".to_owned()];
    expected.extend(upgrade_from_the_old_channel());
    expected.push(SOFT_REBOOTS_QUERY.to_owned());
    assert_eq!(fixture.commands(), expected);
    assert_eq!(fixture.read(ROLLBACK_ENTRY), STANZA);
    Ok(())
}

#[tokio::test]
async fn a_new_daemon_continues_an_update_after_the_last_completed_step() -> TestResult {
    let fixture = Fixture::start(&Options {
        installed_version: "0.4.1",
        post_update_exit_status: Some(0),
        files: &[
            (
                "etc/pacman.conf",
                "[crownos]\nServer = https://repo.crownos.org/v0.4.0/$arch\n",
            ),
            (
                "var/lib/crownos/update/in-progress.toml",
                "from = \"0.4.0\"\nto = \"0.4.1\"\nrestart = \"session\"\nnew_packages = []\n\
                 completed = \"install\"\n\n[rollback]\nsnapshot = 7\nfrom = \"0.4.0\"\n",
            ),
        ],
        ..Options::chain()
    })?;
    let mut client = fixture.connect().await?;
    let snapshot = client.state().await?;
    let failure = snapshot.failure.ok_or("no failure")?;
    assert_eq!(failure.step, FailedStep::Apply(ApplyStep::SwitchChannel));
    assert_eq!(
        failure.rollback.as_ref().map(|rollback| rollback.snapshot),
        Some(SnapperNumber(7))
    );
    let next = snapshot.next.ok_or("no next release")?;
    assert_eq!(
        (next.version.to_string(), next.download),
        ("0.4.1".into(), DownloadStatus::Complete)
    );
    assert!(snapshot.in_progress.is_some());
    assert!(matches!(
        client.check().await,
        Err(ClientError::WrongPhase(_))
    ));

    client.apply().await?;
    let snapshot = wait_for_the_end(&mut client, Some(&failure)).await?;
    assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
    assert_eq!(
        snapshot.pending_restart.map(|pending| pending.kind),
        Some(RestartKind::Session)
    );
    assert_eq!(
        fixture.commands(),
        [
            "post-update 0.4.0 0.4.1".to_owned(),
            SOFT_REBOOTS_QUERY.to_owned()
        ]
    );
    assert!(fixture.read("etc/pacman.conf").contains("/v0.4.1/$arch"));
    assert!(!fixture.exists(IN_PROGRESS));
    Ok(())
}

#[tokio::test]
async fn sigterm_lets_the_running_upgrade_finish() -> TestResult {
    let mut fixture = Fixture::start(&Options {
        post_update_exit_status: Some(0),
        ..Options::chain()
    })?;
    fixture.fail("slow-install")?;
    let mut client = fixture.connect().await?;
    client.check().await?;
    wait_for(&mut client, |snapshot| {
        snapshot.is_idle()
            && snapshot
                .next
                .as_ref()
                .is_some_and(|next| next.download == DownloadStatus::Complete)
    })
    .await?;
    client.apply().await?;
    wait_for(&mut client, |snapshot| {
        snapshot.phase == update_client::proto::Phase::Applying(ApplyStep::Install)
    })
    .await?;
    let status = fixture.terminate().await?;
    assert!(status.success(), "{status}\n{}", fixture.daemon_log());
    assert!(fixture.exists(APPLIED_MARKER), "{}", fixture.daemon_log());
    assert!(
        fixture
            .commands()
            .contains(&"post-update 0.4.0 0.4.1".to_owned())
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_kernel_copy_deletes_the_snapshot_before_pacman_runs() -> TestResult {
    let fixture = Fixture::start(&Options::chain())?;
    fixture.fail("esp-kernel")?;
    let (_client, snapshot) = update(&fixture).await?;
    let failure = snapshot.failure.ok_or("no failure")?;
    assert_eq!(failure.step, FailedStep::Apply(ApplyStep::KeepKernel));
    assert!(failure.rollback.is_none());
    let commands = fixture.commands();
    assert_eq!(commands.len(), 5, "{commands:?}");
    assert_eq!(commands[2..4], before_install()[2..4]);
    assert_eq!(
        commands.last().map(String::as_str),
        Some("snapper --no-dbus -c root delete 7")
    );
    assert!(
        !commands
            .iter()
            .any(|command| command.starts_with("pacman -Syu "))
    );
    assert_eq!(fixture.read(ROLLBACK_ENTRY), NO_ROLLBACK_ENTRY);
    Ok(())
}

#[tokio::test]
async fn a_failed_kernel_copy_removes_the_old_rollback_entry() -> TestResult {
    let fixture = Fixture::start(&Options {
        files: &[(
            ROLLBACK_ENTRY,
            "menuentry \"Rollback to CrownOS 0.3.9\" {\n}\n",
        )],
        ..Options::chain()
    })?;
    fixture.fail("esp-kernel")?;
    let (_client, snapshot) = update(&fixture).await?;
    let failure = snapshot.failure.ok_or("no failure")?;
    assert_eq!(failure.step, FailedStep::Apply(ApplyStep::KeepKernel));
    assert!(
        failure
            .message
            .contains("rollback entry in the boot menu is unavailable until the next update"),
        "{}",
        failure.message
    );
    assert!(
        failure.message.contains("no kernel on the ESP"),
        "{}",
        failure.message
    );
    assert_eq!(
        fixture.commands().last().map(String::as_str),
        Some("snapper --no-dbus -c root delete 7")
    );
    assert_eq!(fixture.read(ROLLBACK_ENTRY), NO_ROLLBACK_ENTRY);
    Ok(())
}

#[tokio::test]
async fn a_session_restart_only_forgets_the_pending_restart() -> TestResult {
    let fixture = Fixture::start(&Options {
        next: Some(Release {
            restart: "session",
            ..Options::chain().next.ok_or("no next release")?
        }),
        ..Options::chain()
    })?;
    let (mut client, snapshot) = update(&fixture).await?;
    assert_eq!(
        snapshot.pending_restart.map(|pending| pending.kind),
        Some(RestartKind::Session)
    );
    client.restart().await?;
    wait_for(&mut client, |snapshot| snapshot.pending_restart.is_none()).await?;
    assert!(!fixture.exists(APPLIED_MARKER));
    assert!(
        !fixture
            .commands()
            .iter()
            .any(|command| command == "systemctl reboot" || command == "systemctl soft-reboot")
    );
    Ok(())
}

const PRESET: &str = "usr/lib/systemd/system-preset/80-crownos.preset";
const APPLIED_PRESETS: &str = "var/lib/crownos/update/applied-presets";
const PRESET_OF_THE_NEXT_RELEASE: &str = "# CrownOS\nenable NetworkManager.service\n\
    enable systemd-timesyncd.service\nenable power-profiles-daemon.service\n\
    enable snapper-cleanup.timer\nenable crownos-updated.service\n\
    enable crownos-new.service\nenable crownos-of-a-removed-package.service\n";
const NEW_UNIT_FILE: &str = "usr/lib/systemd/system/crownos-new.service";

fn after_install(changes: &[&str]) -> Vec<String> {
    let mut expected = before_install();
    expected.extend(changes.iter().map(|change| (*change).to_owned()));
    expected.extend([
        "post-update 0.4.0 0.4.1".to_owned(),
        SOFT_REBOOTS_QUERY.to_owned(),
    ]);
    expected
}

#[tokio::test]
async fn installed_retired_packages_are_removed_before_the_post_update() -> TestResult {
    let fixture = Fixture::start(&Options {
        installed_packages: &["crownbar", "crownos-oldtool"],
        next: Some(Release {
            retired: &["crownos-oldtool", "crownos-never-installed"],
            ..Options::chain().next.ok_or("no next release")?
        }),
        post_update_exit_status: Some(0),
        ..Options::chain()
    })?;
    let (_client, snapshot) = update(&fixture).await?;
    assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
    assert_eq!(
        fixture.commands(),
        after_install(&["pacman -Qq", "pacman -Rs --noconfirm -- crownos-oldtool"])
    );
    Ok(())
}

#[tokio::test]
async fn a_system_without_a_record_enables_only_units_the_old_installer_did_not() -> TestResult {
    let fixture = Fixture::start(&Options {
        post_update_exit_status: Some(0),
        files: &[
            (PRESET, PRESET_OF_THE_NEXT_RELEASE),
            (NEW_UNIT_FILE, "[Unit]\n"),
        ],
        ..Options::chain()
    })?;
    let (_client, snapshot) = update(&fixture).await?;
    assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
    assert_eq!(
        fixture.commands(),
        after_install(&["systemctl preset -- crownos-new.service"])
    );
    let record: Vec<String> = fixture
        .read(APPLIED_PRESETS)
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(record.len(), 7, "{record:?}");
    assert!(record.contains(&"crownos-of-a-removed-package.service".to_owned()));
    Ok(())
}

#[tokio::test]
async fn a_unit_in_the_record_is_not_enabled_again() -> TestResult {
    let fixture = Fixture::start(&Options {
        post_update_exit_status: Some(0),
        files: &[
            (PRESET, PRESET_OF_THE_NEXT_RELEASE),
            (NEW_UNIT_FILE, "[Unit]\n"),
            (
                "usr/lib/systemd/system/power-profiles-daemon.service",
                "[Unit]\n",
            ),
            (
                APPLIED_PRESETS,
                "NetworkManager.service\nsystemd-timesyncd.service\ncrownos-new.service\n",
            ),
        ],
        ..Options::chain()
    })?;
    let (_client, snapshot) = update(&fixture).await?;
    assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
    assert_eq!(
        fixture.commands(),
        after_install(&["systemctl preset -- power-profiles-daemon.service"])
    );
    Ok(())
}

#[tokio::test]
async fn a_failed_preset_fails_the_update_and_keeps_the_record() -> TestResult {
    let fixture = Fixture::start(&Options {
        post_update_exit_status: Some(0),
        files: &[
            (PRESET, PRESET_OF_THE_NEXT_RELEASE),
            (NEW_UNIT_FILE, "[Unit]\n"),
        ],
        ..Options::chain()
    })?;
    fixture.fail("preset")?;
    let (mut client, snapshot) = update(&fixture).await?;
    let failure = snapshot.failure.ok_or("no failure")?;
    assert_eq!(failure.step, FailedStep::Apply(ApplyStep::PostUpdate));
    assert!(failure.rollback.is_some());
    assert!(
        failure.message.contains("Failed to preset unit"),
        "{}",
        failure.message
    );
    assert!(!fixture.exists(APPLIED_PRESETS));
    assert!(!fixture.exists(APPLIED_MARKER));
    assert!(
        !fixture
            .commands()
            .contains(&"post-update 0.4.0 0.4.1".to_owned())
    );

    fixture.repair("preset")?;
    client.apply().await?;
    let snapshot = wait_for_the_end(&mut client, Some(&failure)).await?;
    assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
    assert!(
        fixture
            .read(APPLIED_PRESETS)
            .contains("crownos-new.service\n")
    );
    Ok(())
}
