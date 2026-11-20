use std::{
    io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use tokio::{fs, io::AsyncWriteExt};

const PERMISSION_BITS: u32 = 0o7777;

/// Writes `bytes` to a temporary file beside `path`, then renames it over
/// `path`, so a reader or a power cut sees the old file or the new one. The
/// new file keeps the mode of the old one.
pub async fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let staging = staging_path(path);
    let mut file = fs::File::create(&staging).await?;
    file.write_all(bytes).await?;
    file.sync_all().await?;
    drop(file);
    keep_mode_of(path, &staging).await?;
    fs::rename(&staging, path).await?;
    if let Err(error) = sync_parent(path).await {
        tracing::debug!("cannot flush the directory of {}: {error}", path.display());
    }
    Ok(())
}

/// chmod fails on the vfat ESP, so it runs only when the modes differ.
async fn keep_mode_of(existing: &Path, staging: &Path) -> io::Result<()> {
    let Ok(existing) = fs::metadata(existing).await else {
        return Ok(());
    };
    let staged_mode = fs::metadata(staging).await?.permissions().mode() & PERMISSION_BITS;
    if existing.permissions().mode() & PERMISSION_BITS == staged_mode {
        return Ok(());
    }
    fs::set_permissions(staging, existing.permissions()).await
}

/// Best effort: the rename is done, and some file systems refuse to flush a directory.
async fn sync_parent(path: &Path) -> io::Result<()> {
    let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    else {
        return Ok(());
    };
    fs::File::open(parent).await?.sync_all().await
}

fn staging_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".{}.tmp", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use std::fs::Permissions;

    use super::*;

    #[tokio::test]
    async fn replaces_the_file_and_leaves_no_staging_file() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("state.toml");
        write_atomically(&path, b"old").await?;
        write_atomically(&path, b"new").await?;
        assert_eq!(std::fs::read(&path)?, b"new");
        assert_eq!(std::fs::read_dir(directory.path())?.count(), 1);
        Ok(())
    }

    #[tokio::test]
    async fn the_new_file_keeps_the_old_mode() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("pacman.conf");
        std::fs::write(&path, "old")?;
        std::fs::set_permissions(&path, Permissions::from_mode(0o640))?;
        write_atomically(&path, b"new").await?;
        let mode = std::fs::metadata(&path)?.permissions().mode() & PERMISSION_BITS;
        assert_eq!(mode, 0o640);
        Ok(())
    }
}
