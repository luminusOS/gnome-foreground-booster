# gnome-foreground-booster

A small user daemon that gives the focused GNOME application a higher CPU weight and restores it when focus moves away. GNOME counterpart of KDE's `plasma-foreground-booster`.

It does not read the screen or the compositor directly. [Aurora Shell](https://github.com/luminusOS/aurora-shell) reports the focused window over D-Bus, and the daemon changes `CPUWeight` on that application's systemd unit through the user manager.

```
Aurora Shell (foreground-bridge module)
    │  org.luminusos.AuroraShell.Foreground.FocusChanged(pid, app_id)   unicast, session bus
    ▼
gnome-foreground-booster  (org.luminusos.ForegroundBooster)
    │  /proc/<pid>/cgroup → direct child of app.slice (….scope, ….service or ….slice)
    │  org.freedesktop.systemd1.Manager.SetUnitProperties(CPUWeight, runtime)
    ▼
systemd --user
```

## Requirements

- GNOME Shell on Wayland with the Aurora Shell extension and its `foreground-bridge` module enabled.
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
gsettings set org.gnome.shell.extensions.aurora-shell module-foreground-bridge true
systemctl --user enable --now gnome-foreground-booster.service
```

## Options

| Flag | Default | Meaning |
| --- | --- | --- |
| `--weight` | `400` | `CPUWeight` of the focused unit. The systemd default is `100`; the range is 1–10000. |
| `--debounce-ms` | `150` | How long focus must stay on a window before it is boosted, so Alt+Tab does not churn units. |

Add flags with `systemctl --user edit gnome-foreground-booster.service`. Set `RUST_LOG=gnome_foreground_booster=debug` for diagnostics.

## Behavior

- The weight is applied to the direct child of `app.slice` that contains the focused window's process. cgroup v2 weights only compete between siblings, so this is the level where the app competes with other apps.
- Terminals are the main limitation. Some terminals (Ptyxis, VTE-based ones) start each shell in its own `*-spawn-*.scope`, a sibling of the terminal's unit. Focusing the terminal boosts the terminal process, not the commands you run inside it.
- The previous `CPUWeight` is read before the boost and written back afterwards. If someone else changed the weight in the meantime, it is left alone.
- Changes are runtime-only. On SIGTERM, SIGINT and SIGHUP the daemon restores the boosted unit before exiting.
- The boost is recorded in `$XDG_RUNTIME_DIR/gnome-foreground-booster/state`. After a crash the next start restores it. Unit drop-ins for transient units vanish with the unit, but a non-transient unit (for example a D-Bus activated service) can keep the boost until the daemon restarts or you log out.
- When Aurora Shell leaves the bus (extension disabled, lock screen, Shell restart) the boost is released immediately and applied again when it returns.
- Windows whose process is not under your `user@<uid>.service`/`app.slice`, such as the Shell itself, are ignored and release the current boost. The same happens when no window has focus.
- Logs are quiet by default. Set `RUST_LOG=gnome_foreground_booster=debug` to see every boost and restore.

## D-Bus interface

Owned by Aurora Shell on the session bus: `org.luminusos.AuroraShell.Foreground` at `/org/luminusos/AuroraShell/Foreground`.

| Member | Signature | Notes |
| --- | --- | --- |
| `FocusChanged` | `(u pid, s app_id)` | Sent only to the owner of `org.luminusos.ForegroundBooster`. `pid` is `0` when nothing is focused. |
| `GetFocus()` | `→ (u pid, s app_id)` | Rejected for any caller other than the booster. Handy for debugging; the daemon relies on `FocusChanged`. |
| `Version` | `u` | Currently `1`. |

Focus is not broadcast, so other session clients cannot observe which window is active through this interface. The Shell requests the bridge name with `DO_NOT_QUEUE` and logs when another process already owns it.

This is not a hard security boundary. Any process of your user can already change the weight of your own units through systemd, and one that claims `org.luminusos.ForegroundBooster` before the daemon would receive the focus signals.

## Development

```sh
just check                                                  # fmt, clippy, unit tests
BIN=path/to/gnome-foreground-booster just e2e               # needs systemd --user, gjs and a free bus name
```

The unit tests cover cgroup path parsing and the boost, restore and crash-recovery logic against a fake unit controller. The end-to-end script runs the real daemon against the real systemd user manager with a fake bridge written in GJS, and checks focus changes, the bridge leaving the bus, crash recovery and SIGTERM. Do not run it while the real daemon is active.
