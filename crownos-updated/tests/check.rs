mod support;

use support::{Fixture, Options, Release, TestResult, wait_for};
use update_client::{
    ClientError, Event,
    proto::{DownloadStatus, FailedStep, Phase, RestartKind},
};

const OVERRIDE_CONF: &str = "--config ROOT/run/crownos/update/pacman.conf";

#[tokio::test]
async fn the_first_release_after_the_installed_one_is_offered_and_downloaded() -> TestResult {
    let fixture = Fixture::start(&Options::chain())?;
    let mut client = fixture.connect().await?;
    client.check().await?;
    let snapshot = wait_for(&mut client, |snapshot| {
        snapshot.is_idle()
            && snapshot
                .next
                .as_ref()
                .is_some_and(|next| next.download == DownloadStatus::Complete)
    })
    .await?;

    let next = snapshot.next.ok_or("no next release")?;
    assert_eq!(next.version.to_string(), "0.4.1");
    assert_eq!(next.restart, RestartKind::SoftReboot);
    assert_eq!(
        next.notes.as_deref(),
        Some("## CrownOS 0.4.1\n\nFaster updates.\n")
    );
    assert_eq!(
        next.new_packages
            .iter()
            .map(|name| name.as_str())
            .collect::<Vec<_>>(),
        ["crowndock"]
    );
    assert_eq!(
        snapshot
            .installed
            .map(|version| version.to_string())
            .as_deref(),
        Some("0.4.0")
    );
    assert!(snapshot.last_check.is_some());
    assert!(snapshot.failure.is_none(), "{}", fixture.daemon_log());
    assert_eq!(
        fixture.commands(),
        [
            "pacman -Qq".to_owned(),
            format!("pacman -Syuw --noconfirm {OVERRIDE_CONF} --needed -- crowndock"),
        ]
    );
    assert!(
        fixture
            .read("run/crownos/update/pacman.conf")
            .contains("/v0.4.1/$arch")
    );
    assert!(fixture.read("etc/pacman.conf").contains("/v0.4.0/$arch"));
    assert!(
        fixture
            .read("var/lib/crownos/update/state.toml")
            .contains("version = \"0.4.1\"")
    );
    assert!(
        fixture
            .read("var/lib/crownos/update/next-manifest.toml")
            .contains("version = \"0.4.1\"")
    );
    assert!(fixture.exists("var/lib/crownos/update/next-notes.md"));

    let events = client.drain_events();
    assert!(events.iter().any(|event| matches!(event, Event::State(_))));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::Progress(progress) if progress.line == "downloading crowndock 100%" && progress.fraction == Some(1.0)
    )));
    Ok(())
}

#[tokio::test]
async fn the_latest_release_has_no_next_release() -> TestResult {
    let fixture = Fixture::start(&Options {
        installed_version: "0.4.2",
        ..Options::chain()
    })?;
    let mut client = fixture.connect().await?;
    client.check().await?;
    let snapshot = wait_for(&mut client, |snapshot| {
        snapshot.is_idle() && snapshot.last_check.is_some()
    })
    .await?;
    assert!(snapshot.next.is_none());
    assert!(snapshot.failure.is_none(), "{}", fixture.daemon_log());
    assert!(fixture.commands().is_empty());
    assert!(!fixture.exists("var/lib/crownos/update/next-manifest.toml"));
    Ok(())
}

#[tokio::test]
async fn a_bad_signature_is_a_check_failure() -> TestResult {
    let fixture = Fixture::start(&Options::chain())?;
    fixture.fail("gpg")?;
    let mut client = fixture.connect().await?;
    client.check().await?;
    let snapshot = wait_for(&mut client, |snapshot| {
        snapshot.is_idle() && snapshot.failure.is_some()
    })
    .await?;
    let failure = snapshot.failure.ok_or("no failure")?;
    assert_eq!(failure.step, FailedStep::Check);
    assert!(
        failure.message.contains("not signed by CrownOS"),
        "{}",
        failure.message
    );
    assert!(snapshot.next.is_none());
    assert!(fixture.commands().is_empty());

    fixture.repair("gpg")?;
    client.check().await?;
    let snapshot = wait_for(&mut client, |snapshot| {
        snapshot.is_idle() && snapshot.next.is_some()
    })
    .await?;
    assert!(snapshot.failure.is_none());
    Ok(())
}

#[tokio::test]
async fn missing_release_notes_are_no_notes() -> TestResult {
    let fixture = Fixture::start(&Options::chain())?;
    fixture.remove("repo/v0.4.1/notes.md")?;
    let mut client = fixture.connect().await?;
    client.check().await?;
    let snapshot = wait_for(&mut client, |snapshot| {
        snapshot.is_idle() && snapshot.last_check.is_some()
    })
    .await?;
    assert!(snapshot.failure.is_none(), "{:?}", snapshot.failure);
    let next = snapshot.next.ok_or("no next release")?;
    assert_eq!(next.notes, None);
    assert!(!fixture.exists("var/lib/crownos/update/next-notes.md"));
    Ok(())
}

#[tokio::test]
async fn sigterm_lets_the_running_download_finish() -> TestResult {
    let mut fixture = Fixture::start(&Options::chain())?;
    fixture.fail("slow-download")?;
    let mut client = fixture.connect().await?;
    client.check().await?;
    wait_for(&mut client, |snapshot| snapshot.phase == Phase::Downloading).await?;
    let status = fixture.terminate().await?;
    assert!(status.success(), "{status}\n{}", fixture.daemon_log());
    let state = fixture.read("var/lib/crownos/update/state.toml");
    assert!(state.contains("download = \"complete\""), "{state}");
    Ok(())
}

#[tokio::test]
async fn a_chain_that_does_not_end_at_the_manifest_is_refused() -> TestResult {
    let fixture = Fixture::start(&Options {
        stable: Release {
            releases: &["0.4.0", "0.4.1"],
            ..Options::chain().stable
        },
        ..Options::chain()
    })?;
    let mut client = fixture.connect().await?;
    client.check().await?;
    let snapshot = wait_for(&mut client, |snapshot| {
        snapshot.is_idle() && snapshot.failure.is_some()
    })
    .await?;
    let message = snapshot
        .failure
        .map(|failure| failure.message)
        .unwrap_or_default();
    assert!(message.contains("ends at 0.4.1"), "{message}");
    Ok(())
}

#[tokio::test]
async fn a_user_outside_wheel_may_read_but_not_check() -> TestResult {
    if is_root() {
        eprintln!("root is always an administrator; skipping");
        return Ok(());
    }
    let fixture = Fixture::start(&Options {
        administrator: false,
        ..Options::chain()
    })?;
    let mut client = fixture.connect().await?;
    assert!(client.state().await?.installed.is_some());
    assert!(matches!(client.check().await, Err(ClientError::NotAllowed)));
    Ok(())
}

fn is_root() -> bool {
    use std::os::unix::fs::MetadataExt;
    tempfile::tempdir()
        .and_then(|directory| std::fs::metadata(directory.path()))
        .is_ok_and(|metadata| metadata.uid() == 0)
}
