use anyhow::{Context as _, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};

/// Returns `~/.claude/ide/`, creating the directory if it does not exist.
/// On Unix the directory is chmod 0700 to match the VS Code extension.
pub fn ide_dir() -> Result<PathBuf> {
    let home = dirs::home_dir().context("could not resolve home directory")?;
    let dir = home.join(".claude").join("ide");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&dir)?.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(&dir, perms).ok();
    }
    Ok(dir)
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct LockfilePayload<'a> {
    pid: u32,
    workspace_folders: &'a [String],
    ide_name: &'a str,
    transport: &'a str,
    running_in_windows: bool,
    auth_token: &'a str,
}

/// Writes the lockfile at `~/.claude/ide/<port>.lock`. On Unix, the file mode
/// is set to 0600. The file is overwritten if it already exists.
pub fn write(
    path: &Path,
    pid: u32,
    workspace_folders: &[String],
    auth_token: &str,
) -> Result<()> {
    let payload = LockfilePayload {
        pid,
        workspace_folders,
        ide_name: "Zed",
        transport: "ws",
        running_in_windows: cfg!(windows),
        auth_token,
    };
    let json = serde_json::to_vec_pretty(&payload)?;
    std::fs::write(path, &json)
        .with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)?.permissions();
        perms.set_mode(0o600);
        std::fs::set_permissions(path, perms).ok();
    }
    Ok(())
}

/// Removes the lockfile at the given path. Missing files are ignored.
pub fn remove(path: &Path) {
    if let Err(error) = std::fs::remove_file(path)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!("failed to remove lockfile {}: {error}", path.display());
    }
}
