# gnome-foreground-booster

A small user daemon that gives the focused GNOME application a higher CPU weight and restores it when focus moves away. GNOME counterpart of KDE's `plasma-foreground-booster`.

It needs no Shell extension. Since GNOME 41, Mutter marks the systemd unit of the focused window by setting the `user.xdg.inactive-since` extended attribute on its cgroup directory: `-1` for the focused unit, a monotonic timestamp for the ones that lost focus. The daemon watches those attributes with inotify and changes `CPUWeight` on the focused application's unit through the systemd user manager.

```
Mutter (GNOME Shell)
    │  setxattr user.xdg.inactive-since on …/app.slice/<unit>
    ▼
gnome-foreground-booster   inotify IN_ATTRIB on app.slice and its sub-slices
    │  org.freedesktop.systemd1.Manager.SetUnitProperties(CPUWeight, runtime)
    ▼
systemd --user
```

## Requirements

- GNOME 41 or later, on Wayland or X11.
- cgroup v2 and the `cpu` controller delegated to `user@.service`. Fedora and most current distributions already do this; the daemon warns at startup when it is missing. Otherwise add a drop-in at `/etc/systemd/system/user@.service.d/delegate.conf`:

  ```ini
  [Service]
  Delegate=cpu cpuset io memory pids
  ```

- Applications started inside their own systemd unit under `app.slice`. GNOME does this for apps launched from the Shell, and Flatpak does it for sandboxed apps.
- Unprivileged user namespaces, because the shipped unit uses `ProtectSystem=` and `PrivateTmp=`. Remove those lines from the unit if your system disallows them.

## Install

```sh
just install                      # system-wide under /usr
just install-user                 # or: under ~/.local and ~/.config/systemd/user
systemctl --user enable --now gnome-foreground-booster.service
```

## Options

| Flag | Default | Meaning |
| --- | --- | --- |
| `--weight` | `400` | `CPUWeight` of the focused unit. The systemd default is `100`; the range is 1–10000. |
| `--debounce-ms` | `150` | How long focus must stay on a window before it is boosted, so Alt+Tab does not churn units. |

Add flags with `systemctl --user edit gnome-foreground-booster.service`. Set `RUST_LOG=gnome_foreground_booster=debug` for diagnostics.

## Behavior

- The weight is applied to the direct child of `app.slice` that contains the focused unit. cgroup v2 weights only compete between siblings, so this is the level where the app competes with other apps.
- Terminals are the main limitation. Some terminals (Ptyxis, VTE-based ones) start each shell in its own `*-spawn-*.scope`, a sibling of the terminal's unit. Focusing the terminal boosts the terminal process, not the commands you run inside it.
- GNOME Shell moves an app into its own scope asynchronously, after the process has started. Launchers that fork right away, AppImages in particular, leave their real process behind in the Shell's own cgroup, and Mutter then attributes the window to the Shell. The daemon moves those processes into a new scope next to the launcher's, through systemd, at startup and whenever a new app unit appears. Only processes whose parent lives in an app unit, and their descendants, are moved; the Shell and its own helpers stay put. Mutter remembers a window's unit when it first sees it, so a window that was already open before the move keeps being attributed to the Shell until it is closed and reopened.
- Mutter only updates the attribute when focus moves between different units, and clears the focused mark when nothing has focus, for example in the overview. The boost is released in that case.
- The previous `CPUWeight` is read before the boost and written back afterwards. If someone else changed the weight in the meantime, it is left alone.
- Changes are runtime-only. On SIGTERM, SIGINT and SIGHUP the daemon restores the boosted unit before exiting.
- The boost is recorded in `$XDG_RUNTIME_DIR/gnome-foreground-booster/state`. After a crash the next start restores it. Unit drop-ins for transient units vanish with the unit, but a non-transient unit (for example a D-Bus activated service) can keep the boost until the daemon restarts or you log out.
- Only the current user's `user@<uid>.service/app.slice` is watched.
- Any process of your user can set the same attribute and so choose which of your own units is boosted. That gives it nothing it could not already do through systemd.
- Logs are quiet by default. Set `RUST_LOG=gnome_foreground_booster=debug` to see every boost and restore.

## Development

```sh
just check                                                  # fmt, clippy, unit tests
BIN=path/to/gnome-foreground-booster just e2e               # needs systemd --user and python3
```

The unit tests cover cgroup path handling, the focus tracking and the boost, restore and crash-recovery logic against a fake unit controller. The end-to-end script starts throwaway units in their own slice, sets the attribute the way Mutter does and runs the real daemon against the real systemd user manager. It also imitates the Shell's launch race with a fake Shell unit and checks that stranded children are moved out of it. It checks focus changes, units created later in nested slices, crash recovery and SIGTERM, and does not touch your real applications.
