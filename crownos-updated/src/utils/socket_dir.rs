use std::{
    fs::{self, DirBuilder, Permissions},
    io,
    os::unix::fs::{DirBuilderExt, PermissionsExt},
    path::Path,
};

const SHARED_DIRECTORY_MODE: u32 = 0o755;

/// crownos-ipc creates a missing socket directory 0700, which would lock out
/// every user but root. tmpfiles creates `/run/crownos` 0755 in production;
/// this does the same when the daemon starts first or runs on a fixture.
pub fn ensure_socket_directory(path: &Path) -> io::Result<()> {
    DirBuilder::new()
        .recursive(true)
        .mode(SHARED_DIRECTORY_MODE)
        .create(path)?;
    fs::set_permissions(path, Permissions::from_mode(SHARED_DIRECTORY_MODE))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_directory_every_user_can_enter() -> io::Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("run/crownos");
        ensure_socket_directory(&path)?;
        ensure_socket_directory(&path)?;
        let mode = fs::metadata(&path)?.permissions().mode() & 0o777;
        assert_eq!(mode, SHARED_DIRECTORY_MODE);
        Ok(())
    }
}
