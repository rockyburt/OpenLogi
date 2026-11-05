//! The agent's Linux notification-area icon, over StatusNotifierItem.
//!
//! Mirrors the Windows icon ([`crate::tray_windows`]): the always-on agent
//! hosts the icon, the GUI is on-demand. Without it the app has no visible
//! presence once the GUI window is closed — the agent keeps working, but the
//! user has no way to tell, or to get the window back.
//!
//! The menu is the Windows one: "Show Main Window" (also the left-click
//! action) and "Quit OpenLogi". Show launches the sibling `openlogi-desktop`
//! when no GUI is running. A running GUI already has its window open —
//! closing the last window quits it — and a second launch would only exit on
//! the `openlogi.lock` singleton, so Show leaves it be: raising another
//! process's window is not something a Wayland client may do. Quit
//! terminates the GUI first — its IPC retry loop would otherwise respawn the
//! agent being quit — then shuts the agent down.
//!
//! `ksni` speaks the StatusNotifierItem D-Bus protocol directly, on a thread
//! of its own. GNOME shows the item through the AppIndicator extension; KDE,
//! Cinnamon, XFCE and others natively. With no StatusNotifierWatcher on the
//! bus there is simply no icon, and the agent carries on.

use std::path::Path;

use ksni::blocking::TrayMethods as _;
use tracing::{info, warn};

use crate::shutdown::{self, ShutdownRequestSender};

/// The icon a package installs into the hicolor theme.
const APP_ICON: &str = "openlogi";
/// A stock themed icon for a build with no installed app icon.
const FALLBACK_ICON: &str = "input-mouse";

/// Start the icon unless the user turned it off.
pub fn spawn(show_in_tray: bool, shutdown_tx: ShutdownRequestSender) {
    if !show_in_tray {
        info!("tray icon disabled by preference — agent stays invisible");
        return;
    }
    let tray = AgentTray {
        icon: if app_icon_installed() {
            APP_ICON
        } else {
            FALLBACK_ICON
        },
        shutdown_tx,
    };
    match tray.spawn() {
        // The handle only updates or shuts the item down; neither is needed,
        // and dropping it leaves the service running for the agent's life.
        Ok(_handle) => info!("tray icon registered"),
        Err(error) => warn!(%error, "no tray icon — is a StatusNotifierWatcher running?"),
    }
}

struct AgentTray {
    icon: &'static str,
    shutdown_tx: ShutdownRequestSender,
}

impl ksni::Tray for AgentTray {
    fn id(&self) -> String {
        "openlogi".into()
    }

    fn title(&self) -> String {
        "OpenLogi".into()
    }

    fn icon_name(&self) -> String {
        self.icon.into()
    }

    fn category(&self) -> ksni::Category {
        ksni::Category::Hardware
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_gui();
    }

    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::{MenuItem, StandardItem};
        vec![
            StandardItem {
                label: rust_i18n::t!("app.show_main_window").into_owned(),
                activate: Box::new(|_: &mut Self| open_gui()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: rust_i18n::t!("app.quit_openlogi").into_owned(),
                activate: Box::new(|tray: &mut Self| quit(&tray.shutdown_tx)),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Whether the hicolor theme holds the app icon in any data directory.
fn app_icon_installed() -> bool {
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".local/share")));
    let system =
        std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    home.into_iter()
        .chain(
            system
                .split(':')
                .filter(|dir| !dir.is_empty())
                .map(Into::into),
        )
        .any(|dir| theme_has_icon(&dir.join("icons/hicolor"), APP_ICON))
}

/// Whether `theme` has `name` at any size.
fn theme_has_icon(theme: &Path, name: &str) -> bool {
    let Ok(sizes) = std::fs::read_dir(theme) else {
        return false;
    };
    sizes.flatten().any(|size| {
        ["png", "svg"].iter().any(|ext| {
            size.path()
                .join("apps")
                .join(format!("{name}.{ext}"))
                .is_file()
        })
    })
}

fn open_gui() {
    if gui_pids().is_empty() {
        spawn_gui();
    } else {
        info!("tray Show — the GUI is already running with its window open");
    }
}

/// This user's running GUI processes. Matched on the executable's file name:
/// the kernel truncates a process name to 15 bytes, which would cut
/// `openlogi-desktop` short.
fn gui_pids() -> Vec<sysinfo::Pid> {
    use sysinfo::{Pid, Process, ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};
    let mut system = System::new();
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_exe(UpdateKind::Always)
            .with_user(UpdateKind::Always),
    );
    let own_user = system
        .process(Pid::from_u32(std::process::id()))
        .and_then(Process::user_id)
        .cloned();
    system
        .processes()
        .values()
        .filter(|process| {
            process.exe().is_some_and(is_gui_executable)
                && (own_user.is_none() || process.user_id() == own_user.as_ref())
        })
        .map(Process::pid)
        .collect()
}

fn is_gui_executable(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == "openlogi-desktop")
}

fn spawn_gui() {
    let Ok(exe) = std::env::current_exe() else {
        warn!("could not resolve the agent's own path — cannot launch the GUI");
        return;
    };
    let Some(gui) = exe.parent().map(|dir| dir.join("openlogi-desktop")) else {
        return;
    };
    if !gui.is_file() {
        warn!(path = %gui.display(), "no GUI binary next to the agent");
        return;
    }
    match std::process::Command::new(&gui).spawn() {
        Ok(_) => info!(path = %gui.display(), "tray — launched the GUI"),
        Err(error) => warn!(%error, path = %gui.display(), "tray — could not launch the GUI"),
    }
}

/// Quit the whole app: GUI first (its IPC retry loop would otherwise respawn
/// the agent being quit), then the overlay, then the agent. The GUI holds no
/// unsaved state — config writes are immediate — so a polite SIGTERM is all
/// it is owed.
fn quit(shutdown_tx: &ShutdownRequestSender) -> ! {
    use sysinfo::{ProcessesToUpdate, Signal, System};
    let pids = gui_pids();
    let mut system = System::new();
    system.refresh_processes(ProcessesToUpdate::Some(&pids), true);
    for pid in pids {
        if let Some(process) = system.process(pid) {
            if process.kill_with(Signal::Term) == Some(true) {
                info!(%pid, "tray Quit — terminated the GUI");
            } else {
                warn!(%pid, "tray Quit — could not terminate the GUI");
            }
        }
    }
    crate::overlay::evict_on_quit();
    info!("tray Quit — requesting graceful agent shutdown");
    shutdown::request_tray_quit(Some(shutdown_tx), 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_gui_binary_counts_as_the_gui() {
        assert!(is_gui_executable(Path::new("/usr/bin/openlogi-desktop")));
        assert!(is_gui_executable(Path::new(
            "/home/u/OpenLogi/target/release/openlogi-desktop"
        )));
        assert!(!is_gui_executable(Path::new("/usr/bin/openlogi")));
        assert!(!is_gui_executable(Path::new("/usr/bin/openlogi-agent")));
        assert!(!is_gui_executable(Path::new(
            "/usr/bin/openlogi-desktop-old"
        )));
    }

    #[test]
    fn an_icon_is_found_at_any_size_and_format() {
        let theme = tempfile::tempdir().expect("temp dir");
        assert!(!theme_has_icon(theme.path(), APP_ICON), "empty theme");
        let apps = theme.path().join("48x48/apps");
        std::fs::create_dir_all(&apps).expect("size dir");
        std::fs::write(apps.join("other.png"), b"").expect("other icon");
        assert!(
            !theme_has_icon(theme.path(), APP_ICON),
            "another app's icon"
        );
        std::fs::write(apps.join("openlogi.png"), b"").expect("app icon");
        assert!(theme_has_icon(theme.path(), APP_ICON));
        assert!(!theme_has_icon(&theme.path().join("missing"), APP_ICON));
    }
}
