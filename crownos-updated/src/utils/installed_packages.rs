use std::{collections::HashSet, path::Path};

use tokio::process::Command;

use crate::process::capture;

/// The names of every installed package, as `pacman -Qq` lists them.
pub async fn installed_packages(pacman: &Path) -> anyhow::Result<HashSet<String>> {
    let listing = capture(Command::new(pacman).arg("-Qq")).await?;
    Ok(listing
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect())
}
