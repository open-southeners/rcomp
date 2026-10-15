//! Putting the bundled `rcomp` command-line tool on the user's PATH.
//!
//! Every desktop bundle carries the CLI (built by `scripts/build-cli.ts`):
//!
//! - **macOS** — `Rcomp.app/Contents/MacOS/rcomp-cli` (not `rcomp`, which
//!   would collide with the `Rcomp` app binary on a case-insensitive volume).
//!   Linking creates the symlink `/usr/local/bin/rcomp`, asking for an
//!   administrator password when that folder is not writable.
//! - **Windows** — `<install dir>\bin\rcomp.exe`, in its own folder so adding
//!   it to PATH does not also expose `Rcomp.exe` (which `rcomp` would match).
//!   Linking adds that folder to the user's `Path`; the NSIS uninstaller runs
//!   the app with [`UNLINK_FLAG`] to take it out again.
//! - **Linux** — `/usr/bin/rcomp` from the .deb/.rpm, already on PATH (state
//!   [`CliLinkState::Managed`]). From an AppImage, whose mount point changes
//!   every launch, linking copies the binary to `~/.local/bin/rcomp`.
//!
//! The Settings view shows [`cli_link_status`] and calls [`link_cli`] /
//! [`unlink_cli`].

use std::path::{Path, PathBuf};

use crate::error::IpcError;

/// Launch flag that removes the PATH link and exits without opening a window
/// (run by the Windows uninstaller).
pub const UNLINK_FLAG: &str = "--unlink-cli";

/// Where the command stands, as shown in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CliLinkState {
    /// This build has no bundled CLI (e.g. a dev build that never built it).
    Unavailable,
    /// The package manager installed it on PATH; nothing to toggle.
    Managed,
    /// Bundled but not on PATH.
    NotLinked,
    /// On PATH and pointing at this app's copy.
    Linked,
    /// Ours, but from another (moved or older) copy of the app.
    Stale,
    /// Something else already uses the link path; left untouched.
    Conflict,
}

/// The CLI's link status, returned by every command in this module.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CliLinkStatus {
    /// Where the command stands.
    pub state: CliLinkState,
    /// The bundled CLI binary, when this build has one.
    pub bundled: Option<String>,
    /// The symlink, copy or PATH folder that makes `rcomp` available.
    pub link: Option<String>,
    /// Whether the link's folder is on the PATH new shells get. Only ever
    /// false for `~/.local/bin` on Linux, which not every distro adds.
    pub on_path: bool,
}

/// The bundled CLI next to the running executable, if present.
fn bundled_cli() -> Option<PathBuf> {
    let dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    #[cfg(target_os = "macos")]
    // `rcomp` covers dev builds, where the CLI sits beside the app in target/.
    let candidates = [dir.join("rcomp-cli"), dir.join("rcomp")];
    #[cfg(target_os = "windows")]
    let candidates = [dir.join("bin").join("rcomp.exe")];
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let candidates = [dir.join("rcomp")];
    candidates.into_iter().find(|p| p.is_file())
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn io_error(context: &str, err: std::io::Error) -> IpcError {
    IpcError::new("io", format!("{context}: {err}"))
}

/// Report whether `rcomp` is on PATH.
#[tauri::command]
pub async fn cli_link_status() -> Result<CliLinkStatus, IpcError> {
    crate::commands::blocking(|| Ok(imp::status(bundled_cli()))).await
}

/// Put the bundled CLI on PATH (replacing a stale link) and return the new
/// status. Fails with `conflict` rather than replace someone else's `rcomp`,
/// and with `cancelled` when the user dismisses the password prompt.
#[tauri::command]
pub async fn link_cli() -> Result<CliLinkStatus, IpcError> {
    crate::commands::blocking(|| {
        let Some(bundled) = bundled_cli() else {
            return Err(IpcError::new(
                "unavailable",
                "this build of Rcomp does not include the command-line tool",
            ));
        };
        let current = imp::status(Some(bundled.clone()));
        match current.state {
            CliLinkState::Linked | CliLinkState::Managed => return Ok(current),
            CliLinkState::Conflict => {
                return Err(IpcError::new(
                    "conflict",
                    format!(
                        "{} already exists and is not Rcomp's; remove it first",
                        current.link.unwrap_or_default()
                    ),
                ));
            }
            _ => imp::link(&bundled)?,
        }
        Ok(imp::status(Some(bundled)))
    })
    .await
}

/// Take the CLI off PATH (only a link this app made) and return the new status.
#[tauri::command]
pub async fn unlink_cli() -> Result<CliLinkStatus, IpcError> {
    crate::commands::blocking(|| {
        let bundled = bundled_cli();
        let current = imp::status(bundled.clone());
        if matches!(current.state, CliLinkState::Linked | CliLinkState::Stale) {
            imp::unlink(bundled.as_deref())?;
        }
        Ok(imp::status(bundled))
    })
    .await
}

/// Handle [`UNLINK_FLAG`]: when it is the only argument, remove the link and
/// return the process exit code; otherwise `None` and the app starts normally.
pub fn handle_unlink_flag() -> Option<i32> {
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() != Some(UNLINK_FLAG) || args.next().is_some() {
        return None;
    }
    let bundled = bundled_cli();
    let current = imp::status(bundled.clone());
    if !matches!(current.state, CliLinkState::Linked | CliLinkState::Stale) {
        return Some(0);
    }
    match imp::unlink(bundled.as_deref()) {
        Ok(()) => Some(0),
        Err(err) => {
            eprintln!("rcomp: {}", err.message);
            Some(1)
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use std::{
        io::ErrorKind,
        path::{Path, PathBuf},
        process::Command,
    };

    use super::{CliLinkState, CliLinkStatus, display, io_error};
    use crate::error::IpcError;

    /// On the default PATH through `/etc/paths`, for every shell.
    const LINK: &str = "/usr/local/bin/rcomp";

    pub fn status(bundled: Option<PathBuf>) -> CliLinkStatus {
        let link = Path::new(LINK);
        let state = match (&bundled, std::fs::read_link(link)) {
            (Some(b), Ok(target)) if target == *b => CliLinkState::Linked,
            (_, Ok(target)) if is_app_cli(&target) => CliLinkState::Stale,
            (_, Ok(_)) => CliLinkState::Conflict,
            (_, Err(_)) if link.symlink_metadata().is_ok() => CliLinkState::Conflict,
            (None, Err(_)) => CliLinkState::Unavailable,
            (Some(_), Err(_)) => CliLinkState::NotLinked,
        };
        CliLinkStatus {
            state,
            bundled: bundled.as_deref().map(display),
            link: Some(LINK.into()),
            on_path: true,
        }
    }

    /// A symlink into some copy of Rcomp.app, e.g. one since moved or deleted.
    fn is_app_cli(target: &Path) -> bool {
        target.ends_with("Contents/MacOS/rcomp-cli")
    }

    pub fn link(bundled: &Path) -> Result<(), IpcError> {
        let link = Path::new(LINK);
        let direct = std::fs::create_dir_all("/usr/local/bin").and_then(|()| {
            match std::fs::remove_file(link) {
                Err(e) if e.kind() != ErrorKind::NotFound => Err(e),
                _ => std::os::unix::fs::symlink(bundled, link),
            }
        });
        match direct {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::PermissionDenied => privileged(
                r#"do shell script "/bin/mkdir -p /usr/local/bin && /bin/ln -sfn " & quoted form of item 1 of argv & " " & quoted form of item 2 of argv with administrator privileges"#,
                &[&super::display(bundled), LINK],
            ),
            Err(e) => Err(io_error("could not create /usr/local/bin/rcomp", e)),
        }
    }

    pub fn unlink(_bundled: Option<&Path>) -> Result<(), IpcError> {
        match std::fs::remove_file(LINK) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) if e.kind() == ErrorKind::PermissionDenied => privileged(
                r#"do shell script "/bin/rm -f " & quoted form of item 1 of argv with administrator privileges"#,
                &[LINK],
            ),
            Err(e) => Err(io_error("could not remove /usr/local/bin/rcomp", e)),
        }
    }

    /// Run `script` through `osascript`, which shows the system administrator
    /// password prompt. Paths travel as arguments, never spliced into the
    /// script, and are shell-quoted by `quoted form of`.
    fn privileged(script: &str, args: &[&str]) -> Result<(), IpcError> {
        let output = Command::new("/usr/bin/osascript")
            .args(["-e", "on run argv", "-e", script, "-e", "end run"])
            .args(args)
            .output()
            .map_err(|e| io_error("could not run osascript", e))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        // -128: "User canceled." from the password prompt.
        if stderr.contains("(-128)") {
            return Err(IpcError::new("cancelled", "cancelled"));
        }
        Err(IpcError::new("io", stderr.trim().to_string()))
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::path::{Path, PathBuf};

    use winreg::{RegKey, RegValue, enums::*};

    use super::{CliLinkState, CliLinkStatus, display, io_error};
    use crate::error::IpcError;

    fn bin_dir(bundled: &Path) -> Option<&Path> {
        bundled.parent()
    }

    fn same_dir(entry: &str, dir: &Path) -> bool {
        let entry = entry.trim().trim_end_matches(['\\', '/']);
        let dir = display(dir);
        entry.eq_ignore_ascii_case(dir.trim_end_matches(['\\', '/']))
    }

    /// The user's `Path` (`HKCU\Environment`) and its registry value type.
    fn read_path() -> std::io::Result<(String, RegType)> {
        let env = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Environment")?;
        match env.get_raw_value("Path") {
            Ok(raw) => {
                let wide: Vec<u16> = raw
                    .bytes
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&c| u16::from_le_bytes(c))
                    .take_while(|&c| c != 0)
                    .collect();
                Ok((String::from_utf16_lossy(&wide), raw.vtype))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                Ok((String::new(), REG_EXPAND_SZ))
            }
            Err(e) => Err(e),
        }
    }

    /// Write `value` back with its original type, so `%VAR%` entries keep
    /// expanding, then tell running programs (Explorer, terminals) to reload.
    fn write_path(value: &str, vtype: RegType) -> std::io::Result<()> {
        let env = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Environment", KEY_SET_VALUE)?;
        let bytes = value
            .encode_utf16()
            .chain(std::iter::once(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        env.set_raw_value("Path", &RegValue { bytes, vtype })?;
        broadcast_environment_change();
        Ok(())
    }

    fn broadcast_environment_change() {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            HWND_BROADCAST, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_SETTINGCHANGE,
        };
        let name: Vec<u16> = "Environment\0".encode_utf16().collect();
        let mut result = 0;
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the
        // call; the result pointer is a valid local.
        unsafe {
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                name.as_ptr() as isize,
                SMTO_ABORTIFHUNG,
                5000,
                &mut result,
            );
        }
    }

    pub fn status(bundled: Option<PathBuf>) -> CliLinkStatus {
        let dir = bundled.as_deref().and_then(bin_dir).map(Path::to_path_buf);
        let linked = dir.as_deref().is_some_and(|dir| {
            read_path().is_ok_and(|(path, _)| path.split(';').any(|e| same_dir(e, dir)))
        });
        let state = match (&bundled, linked) {
            (None, _) => CliLinkState::Unavailable,
            (Some(_), true) => CliLinkState::Linked,
            (Some(_), false) => CliLinkState::NotLinked,
        };
        CliLinkStatus {
            state,
            bundled: bundled.as_deref().map(display),
            link: dir.as_deref().map(display),
            on_path: true,
        }
    }

    pub fn link(bundled: &Path) -> Result<(), IpcError> {
        let dir = bin_dir(bundled).ok_or_else(|| IpcError::new("io", "no install folder"))?;
        let (path, vtype) = read_path().map_err(|e| io_error("could not read Path", e))?;
        let mut entries: Vec<&str> = path.split(';').filter(|e| !e.trim().is_empty()).collect();
        let dir = display(dir);
        entries.push(&dir);
        write_path(&entries.join(";"), vtype).map_err(|e| io_error("could not update Path", e))
    }

    pub fn unlink(bundled: Option<&Path>) -> Result<(), IpcError> {
        let Some(dir) = bundled.and_then(bin_dir) else {
            return Ok(());
        };
        let (path, vtype) = read_path().map_err(|e| io_error("could not read Path", e))?;
        let kept: Vec<&str> = path.split(';').filter(|e| !same_dir(e, dir)).collect();
        write_path(&kept.join(";"), vtype).map_err(|e| io_error("could not update Path", e))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
mod imp {
    use std::{
        io::ErrorKind,
        path::{Path, PathBuf},
        process::Command,
    };

    use super::{CliLinkState, CliLinkStatus, display, io_error};
    use crate::error::IpcError;

    /// `~/.local/bin/rcomp`, the per-user bin folder most distros put on PATH.
    fn link_path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/bin/rcomp"))
    }

    fn on_path(dir: &Path) -> bool {
        std::env::var_os("PATH")
            .is_some_and(|path| std::env::split_paths(&path).any(|entry| entry == dir))
    }

    /// Whether `file` is some version of the rcomp CLI (an older copy made by
    /// an earlier AppImage), judged by its `--version` output.
    fn is_rcomp(file: &Path) -> bool {
        Command::new(file)
            .arg("--version")
            .output()
            .is_ok_and(|out| out.status.success() && out.stdout.starts_with(b"rcomp "))
    }

    pub fn status(bundled: Option<PathBuf>) -> CliLinkStatus {
        let Some(bundled) = bundled else {
            return CliLinkStatus {
                state: CliLinkState::Unavailable,
                bundled: None,
                link: None,
                on_path: false,
            };
        };
        // Installed by the .deb/.rpm into a folder already on PATH.
        if bundled.parent().is_some_and(on_path) {
            return CliLinkStatus {
                state: CliLinkState::Managed,
                bundled: Some(display(&bundled)),
                link: Some(display(&bundled)),
                on_path: true,
            };
        }
        let link = link_path();
        let state = match link.as_deref().map(std::fs::read) {
            None => CliLinkState::Unavailable,
            Some(Err(_)) => CliLinkState::NotLinked,
            Some(Ok(copy)) if std::fs::read(&bundled).is_ok_and(|b| b == copy) => {
                CliLinkState::Linked
            }
            Some(Ok(_)) if is_rcomp(link.as_deref().unwrap_or(Path::new(""))) => {
                CliLinkState::Stale
            }
            Some(Ok(_)) => CliLinkState::Conflict,
        };
        CliLinkStatus {
            state,
            bundled: Some(display(&bundled)),
            on_path: link.as_deref().and_then(Path::parent).is_some_and(on_path),
            link: link.as_deref().map(display),
        }
    }

    /// Copy (not symlink: an AppImage's mount point changes every launch)
    /// through a temporary file and a rename, so a running `rcomp` is never
    /// overwritten in place.
    pub fn link(bundled: &Path) -> Result<(), IpcError> {
        use std::os::unix::fs::PermissionsExt as _;

        let link = link_path().ok_or_else(|| IpcError::new("io", "HOME is not set"))?;
        let dir = link.parent().unwrap_or(Path::new("/"));
        std::fs::create_dir_all(dir).map_err(|e| io_error("could not create ~/.local/bin", e))?;
        let tmp = dir.join(".rcomp.tmp");
        std::fs::copy(bundled, &tmp)
            .and_then(|_| std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755)))
            .and_then(|()| std::fs::rename(&tmp, &link))
            .map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                io_error("could not copy rcomp to ~/.local/bin", e)
            })
    }

    pub fn unlink(_bundled: Option<&Path>) -> Result<(), IpcError> {
        let Some(link) = link_path() else {
            return Ok(());
        };
        match std::fs::remove_file(&link) {
            Err(e) if e.kind() != ErrorKind::NotFound => {
                Err(io_error("could not remove ~/.local/bin/rcomp", e))
            }
            _ => Ok(()),
        }
    }
}
