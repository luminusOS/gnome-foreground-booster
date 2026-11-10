use std::collections::HashMap;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::mem::MaybeUninit;
use std::os::fd::OwnedFd;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use rustix::io::Errno;
use tokio::io::Interest;
use tokio::io::unix::AsyncFd;

// Mutter stores focus on the window's unit cgroup since GNOME 41: "-1" marks
// the focused unit, a monotonic timestamp marks when the others lost focus.
const INACTIVE_SINCE: &str = "user.xdg.inactive-since";
const FOCUSED: &[u8] = b"-1";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Slice,
    Unit,
}

pub fn kind_of(name: &OsStr) -> Option<Kind> {
    let name = name.as_bytes();
    if name.ends_with(b".slice") {
        Some(Kind::Slice)
    } else if name.ends_with(b".scope") || name.ends_with(b".service") {
        Some(Kind::Unit)
    } else {
        None
    }
}

pub fn boost_target(app_slice: &Path, unit_dir: &Path) -> Option<String> {
    let first = unit_dir.strip_prefix(app_slice).ok()?.components().next()?;
    first.as_os_str().to_str().map(str::to_owned)
}

fn is_focused(unit_dir: &Path) -> bool {
    let mut value = [0u8; 32];
    match rustix::fs::getxattr(unit_dir, INACTIVE_SINCE, &mut value[..]) {
        Ok(len) => &value[..len] == FOCUSED,
        Err(_) => false,
    }
}

#[derive(Default)]
struct Tracker {
    focused: Option<PathBuf>,
}

impl Tracker {
    fn observe(&mut self, unit_dir: &Path, focused: bool) {
        if focused {
            self.focused = Some(unit_dir.to_path_buf());
        } else {
            self.forget(unit_dir);
        }
    }

    fn forget(&mut self, unit_dir: &Path) {
        if self.focused.as_deref() == Some(unit_dir) {
            self.focused = None;
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Focus(Option<String>),
    UnitAdded,
}

struct RawEvent {
    wd: i32,
    flags: ReadFlags,
    name: Option<OsString>,
}

pub struct FocusWatcher {
    inotify: AsyncFd<OwnedFd>,
    app_slice: PathBuf,
    watches: HashMap<i32, (PathBuf, Kind)>,
    tracker: Tracker,
    reported: Option<String>,
    unit_added: bool,
}

impl FocusWatcher {
    pub fn new(app_slice: PathBuf) -> Result<Self> {
        let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)
            .context("creating an inotify instance")?;
        // SAFETY: `fd` is a freshly created, owned inotify descriptor that the
        // AsyncFd keeps open and unchanged until it is dropped.
        let inotify = unsafe { AsyncFd::register_with_interest(fd, Interest::READABLE) }
            .map_err(|error| error.into_parts().1)?;
        let mut watcher = Self {
            inotify,
            app_slice,
            watches: HashMap::new(),
            tracker: Tracker::default(),
            reported: None,
            unit_added: false,
        };
        watcher.rescan()?;
        Ok(watcher)
    }

    pub fn current(&mut self) -> Option<String> {
        self.reported = self.target();
        self.reported.clone()
    }

    pub async fn next(&mut self) -> Result<Change> {
        loop {
            if std::mem::take(&mut self.unit_added) {
                return Ok(Change::UnitAdded);
            }
            for event in self.read_events().await? {
                self.handle(event)?;
            }
            let target = self.target();
            if target != self.reported {
                self.reported.clone_from(&target);
                return Ok(Change::Focus(target));
            }
        }
    }

    fn target(&self) -> Option<String> {
        let unit_dir = self.tracker.focused.as_deref()?;
        boost_target(&self.app_slice, unit_dir)
    }

    fn rescan(&mut self) -> Result<()> {
        for (wd, _) in self.watches.drain() {
            let _ = inotify::remove_watch(self.inotify.get_ref(), wd);
        }
        self.tracker = Tracker::default();

        let app_slice = self.app_slice.clone();
        let mut focused = Vec::new();
        self.watch_slice(&app_slice, &mut focused)
            .with_context(|| format!("watching {}", app_slice.display()))?;

        match focused.len() {
            0 => {}
            1 => self.tracker.focused = focused.pop(),
            count => tracing::debug!(
                count,
                "several units claim focus; waiting for the next change"
            ),
        }
        Ok(())
    }

    fn watch_slice(&mut self, dir: &Path, focused: &mut Vec<PathBuf>) -> Result<()> {
        let wd = inotify::add_watch(
            self.inotify.get_ref(),
            dir,
            WatchFlags::CREATE | WatchFlags::ONLYDIR,
        )?;
        self.watches.insert(wd, (dir.to_path_buf(), Kind::Slice));

        for entry in fs::read_dir(dir)?.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let child = entry.path();
            match kind_of(&entry.file_name()) {
                Some(Kind::Slice) => {
                    let _ = self.watch_slice(&child, focused);
                }
                Some(Kind::Unit) => {
                    let unit_focused = self.watch_unit(&child);
                    focused.extend(unit_focused.then_some(child));
                }
                None => {}
            }
        }
        Ok(())
    }

    fn watch_unit(&mut self, dir: &Path) -> bool {
        match inotify::add_watch(
            self.inotify.get_ref(),
            dir,
            WatchFlags::ATTRIB | WatchFlags::ONLYDIR,
        ) {
            Ok(wd) => {
                self.watches.insert(wd, (dir.to_path_buf(), Kind::Unit));
                is_focused(dir)
            }
            Err(_) => false,
        }
    }

    async fn read_events(&self) -> Result<Vec<RawEvent>> {
        loop {
            let mut guard = self.inotify.readable().await?;
            let mut buffer = [MaybeUninit::<u8>::uninit(); 4096];
            let mut reader = inotify::Reader::new(self.inotify.get_ref(), &mut buffer);
            let mut events = Vec::new();

            loop {
                match reader.next() {
                    Ok(event) => events.push(RawEvent {
                        wd: event.wd(),
                        flags: event.events(),
                        name: event
                            .file_name()
                            .map(|name| OsStr::from_bytes(name.to_bytes()).to_owned()),
                    }),
                    Err(Errno::AGAIN) => {
                        guard.clear_ready();
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }

            if !events.is_empty() {
                return Ok(events);
            }
        }
    }

    fn handle(&mut self, event: RawEvent) -> Result<()> {
        if event.flags.contains(ReadFlags::QUEUE_OVERFLOW) {
            tracing::debug!("inotify queue overflowed; rescanning");
            return self.rescan();
        }

        let Some((dir, kind)) = self.watches.get(&event.wd).cloned() else {
            return Ok(());
        };

        if event.flags.contains(ReadFlags::IGNORED) {
            self.watches.remove(&event.wd);
            self.tracker.forget(&dir);
            return Ok(());
        }

        match kind {
            Kind::Unit if event.name.is_none() && event.flags.contains(ReadFlags::ATTRIB) => {
                self.tracker.observe(&dir, is_focused(&dir));
            }
            Kind::Slice if event.flags.contains(ReadFlags::CREATE | ReadFlags::ISDIR) => {
                let Some(name) = event.name else {
                    return Ok(());
                };
                let child = dir.join(&name);
                match kind_of(&name) {
                    Some(Kind::Slice) => {
                        let mut focused = Vec::new();
                        let _ = self.watch_slice(&child, &mut focused);
                        for unit_dir in focused {
                            self.tracker.observe(&unit_dir, true);
                        }
                    }
                    Some(Kind::Unit) => {
                        let unit_focused = self.watch_unit(&child);
                        self.tracker.observe(&child, unit_focused);
                        self.unit_added = true;
                    }
                    None => {}
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP_SLICE: &str = "/sys/fs/cgroup/user.slice/user-1000.slice/user@1000.service/app.slice";

    fn target(relative: &str) -> Option<String> {
        boost_target(Path::new(APP_SLICE), &Path::new(APP_SLICE).join(relative))
    }

    #[test]
    fn boosts_a_top_level_scope() {
        assert_eq!(
            target("app-gnome-firefox-4242.scope").as_deref(),
            Some("app-gnome-firefox-4242.scope")
        );
    }

    #[test]
    fn boosts_the_slice_that_competes_with_other_apps() {
        assert_eq!(
            target("app-org.gnome.Terminal.slice/gnome-terminal-server.service").as_deref(),
            Some("app-org.gnome.Terminal.slice")
        );
    }

    #[test]
    fn keeps_escaped_unit_names_verbatim() {
        assert_eq!(
            target("app-dbus\\x2d:1.2\\x2dorg.gnome.Ptyxis.slice/app-dbus\\x2d:1.2\\x2dorg.gnome.Ptyxis@0.service").as_deref(),
            Some("app-dbus\\x2d:1.2\\x2dorg.gnome.Ptyxis.slice")
        );
    }

    #[test]
    fn ignores_paths_outside_app_slice() {
        let session = Path::new(APP_SLICE)
            .parent()
            .unwrap()
            .join("session.slice/org.gnome.Shell@wayland.service");
        assert_eq!(boost_target(Path::new(APP_SLICE), &session), None);
        assert_eq!(
            boost_target(Path::new(APP_SLICE), Path::new(APP_SLICE)),
            None
        );
    }

    #[test]
    fn classifies_cgroup_directories() {
        assert_eq!(kind_of(OsStr::new("a.slice")), Some(Kind::Slice));
        assert_eq!(kind_of(OsStr::new("a.scope")), Some(Kind::Unit));
        assert_eq!(kind_of(OsStr::new("a.service")), Some(Kind::Unit));
        assert_eq!(kind_of(OsStr::new("cgroup.procs")), None);
        assert_eq!(kind_of(OsStr::new("inner")), None);
    }

    #[test]
    fn tracker_follows_mutter_write_order() {
        let a = Path::new("/a.scope");
        let b = Path::new("/b.scope");
        let mut tracker = Tracker::default();

        tracker.observe(a, true);
        tracker.observe(a, false);
        assert_eq!(tracker.focused, None);
        tracker.observe(b, true);
        assert_eq!(tracker.focused.as_deref(), Some(b));
    }

    #[test]
    fn tracker_ignores_a_stale_timestamp_on_another_unit() {
        let a = Path::new("/a.scope");
        let b = Path::new("/b.scope");
        let mut tracker = Tracker::default();

        tracker.observe(b, true);
        tracker.observe(a, false);
        assert_eq!(tracker.focused.as_deref(), Some(b));
    }

    #[test]
    fn tracker_forgets_a_removed_unit() {
        let a = Path::new("/a.scope");
        let mut tracker = Tracker::default();

        tracker.observe(a, true);
        tracker.forget(a);
        assert_eq!(tracker.focused, None);
    }
}
