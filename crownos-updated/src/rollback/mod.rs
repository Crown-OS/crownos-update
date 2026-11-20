//! The way back: a snapper snapshot of the root, the kept kernel pair and a
//! hidden rEFInd entry that boots them.

mod snapper;
mod stanza;

use anyhow::Context;
use tokio::process::Command;
use update_manifest::ReleaseVersion;
use update_proto::SnapperNumber;

pub use snapper::{
    create_rollback_snapshot, delete_snapshot, prune_rollback_snapshots, rollback_snapshot_exists,
};

use crate::{paths::Paths, process::capture, utils::atomic_write::write_atomically};

/// Copies the running kernel pair to the rollback directory of the ESP.
pub async fn keep_kernel(paths: &Paths) -> anyhow::Result<()> {
    capture(Command::new(&paths.tools.esp_kernel).arg("keep")).await?;
    Ok(())
}

/// Removes the menu entry. A failed `keep` can delete the old kernel pair
/// first, and an entry that names it would not boot.
pub async fn remove_rollback_entry(paths: &Paths) -> anyhow::Result<()> {
    write_atomically(
        &paths.rollback_stanza,
        stanza::NO_ROLLBACK_STANZA.as_bytes(),
    )
    .await
    .with_context(|| format!("cannot write {}", paths.rollback_stanza.display()))
}

pub async fn write_rollback_entry(
    paths: &Paths,
    from: &ReleaseVersion,
    number: SnapperNumber,
) -> anyhow::Result<()> {
    let cmdline = tokio::fs::read_to_string(&paths.kernel_cmdline)
        .await
        .with_context(|| format!("cannot read {}", paths.kernel_cmdline.display()))?;
    let entry = stanza::rollback_stanza(from, &cmdline, number);
    write_atomically(&paths.rollback_stanza, entry.as_bytes())
        .await
        .with_context(|| format!("cannot write {}", paths.rollback_stanza.display()))
}
