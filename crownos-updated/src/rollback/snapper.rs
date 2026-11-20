use std::collections::HashMap;

use anyhow::Context;
use serde::Deserialize;
use tokio::process::Command;
use update_manifest::ReleaseVersion;
use update_proto::SnapperNumber;

use crate::{paths::Paths, process::capture};

const ROLLBACK_USERDATA: &str = "crownos-rollback";

#[derive(Deserialize)]
struct ListedSnapshot {
    number: u32,
    userdata: Option<HashMap<String, String>>,
}

fn snapper(paths: &Paths) -> Command {
    let mut command = Command::new(&paths.tools.snapper);
    command.args(["--no-dbus", "-c", "root"]);
    command
}

/// A single snapshot without a cleanup algorithm, so snapper never deletes it.
pub async fn create_rollback_snapshot(
    paths: &Paths,
    from: &ReleaseVersion,
    to: &ReleaseVersion,
) -> anyhow::Result<SnapperNumber> {
    let output = capture(snapper(paths).args([
        "create",
        "--type",
        "single",
        "--print-number",
        "--description",
        &format!("CrownOS {from} before update to {to}"),
        "--userdata",
        &format!("{ROLLBACK_USERDATA}={to}"),
    ]))
    .await?;
    let number = output
        .trim()
        .parse()
        .with_context(|| format!("snapper printed {:?}, not a snapshot number", output.trim()))?;
    Ok(SnapperNumber(number))
}

pub async fn delete_snapshot(paths: &Paths, number: SnapperNumber) -> anyhow::Result<()> {
    capture(snapper(paths).args(["delete", &number.to_string()])).await?;
    Ok(())
}

async fn listed_rollback_snapshots(paths: &Paths) -> anyhow::Result<Vec<SnapperNumber>> {
    let listing = capture(snapper(paths).args(["--jsonout", "list"])).await?;
    rollback_snapshots(&listing)
}

pub async fn rollback_snapshot_exists(
    paths: &Paths,
    number: SnapperNumber,
) -> anyhow::Result<bool> {
    Ok(listed_rollback_snapshots(paths).await?.contains(&number))
}

/// Deletes every rollback snapshot except `keep`, so exactly one stays.
pub async fn prune_rollback_snapshots(paths: &Paths, keep: SnapperNumber) -> anyhow::Result<()> {
    for number in listed_rollback_snapshots(paths)
        .await?
        .into_iter()
        .filter(|number| *number != keep)
    {
        delete_snapshot(paths, number).await?;
    }
    Ok(())
}

fn rollback_snapshots(listing: &str) -> anyhow::Result<Vec<SnapperNumber>> {
    let configurations: HashMap<String, Vec<ListedSnapshot>> =
        serde_json::from_str(listing).context("snapper printed an unexpected snapshot list")?;
    Ok(configurations
        .into_values()
        .flatten()
        .filter(|snapshot| {
            snapshot
                .userdata
                .as_ref()
                .is_some_and(|userdata| userdata.contains_key(ROLLBACK_USERDATA))
        })
        .map(|snapshot| SnapperNumber(snapshot.number))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_only_rollback_snapshots() -> anyhow::Result<()> {
        let listing = r#"{"root": [
            {"number": 0, "description": "current", "userdata": null},
            {"number": 3, "userdata": {"crownos-rollback": "0.4.0"}},
            {"number": 5, "userdata": {"important": "yes"}},
            {"number": 7, "userdata": {"crownos-rollback": "0.4.1"}}
        ]}"#;
        assert_eq!(
            rollback_snapshots(listing)?,
            [SnapperNumber(3), SnapperNumber(7)]
        );
        assert!(rollback_snapshots("not json").is_err());
        Ok(())
    }
}
