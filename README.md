<div align="center">

# gnome-foreground-booster

*Give the GNOME app you are using a bigger share of the CPU*

[![CI](https://github.com/luminusOS/gnome-foreground-booster/actions/workflows/ci.yml/badge.svg)](https://github.com/luminusOS/gnome-foreground-booster/actions/workflows/ci.yml)
[![GNOME](https://img.shields.io/badge/GNOME-41%2B-4a86cf?style=flat-square&logo=gnome&logoColor=white)](https://www.gnome.org)
[![Rust](https://img.shields.io/badge/Rust-1.85%2B-b7410e?style=flat-square&logo=rust)](https://www.rust-lang.org)
[![License](https://img.shields.io/badge/License-GPL--3.0-blue?style=flat-square)](LICENSE)

[Features](#features) • [Installation](#installation) • [Usage](#usage) • [How it works](#how-it-works) • [Limitations](#limitations) • [Development](#development)

</div>

A build, an export or a video encode running in the background gets the same CPU share as the window you are typing in, so the app in front of you can stutter. gnome-foreground-booster is a small user daemon that raises the systemd `CPUWeight` of the focused application and puts it back when focus moves on. It is the GNOME counterpart of KDE's `plasma-foreground-booster`, and it needs no Shell extension.

## Features

- Follows focus through information Mutter already publishes, on Wayland and X11.
- Boosts at the level where apps compete with each other, including apps that live in their own sub-slice.
- Restores the previous weight, and leaves it alone if something else changed it in the meantime.
- Survives crashes: the active boost is recorded in the runtime directory and undone on the next start.
- Repairs apps that GNOME Shell leaves inside its own cgroup, which happens with AppImages such as Ghostty.
- Runs as a sandboxed `systemd --user` service using about 5 MB of memory and no CPU while idle.

## Requirements

- GNOME 41 or later.
- cgroup v2 with the `cpu` controller delegated to `user@.service`. Fedora and most current distributions do this already, and the daemon warns at startup when it is missing.
- Rust 1.85 or later and [`just`](https://github.com/casey/just) to build.

> [!NOTE]
> If the startup warning appears, delegate the controller with a drop-in at `/etc/systemd/system/user@.service.d/delegate.conf`, then log out and back in:
>
> ```ini
> [Service]
> Delegate=cpu cpuset io memory pids
> ```

## Installation

Build the release binary:

```sh
just build
```

Install it for your user only, under `~/.local/bin` and `~/.config/systemd/user`:

```sh
just install-user
```

Or system-wide, under `/usr`. `sudo` usually drops `~/.cargo/bin` from `PATH`, hence the full path to `just`:

```sh
sudo "$(command -v just)" install
```

Then start the service and enable it for every login:

```sh
systemctl --user daemon-reload
systemctl --user enable --now gnome-foreground-booster.service
```

To remove it, run `just uninstall` with the same `PREFIX` and `UNITDIR` you installed with, for example `PREFIX=$HOME/.local UNITDIR=$HOME/.config/systemd/user just uninstall`.

## Usage

The service needs no configuration. To see it react, stop it and run the binary in a terminal with debug logging, then switch between a few windows:

```sh
systemctl --user stop gnome-foreground-booster.service
RUST_LOG=gnome_foreground_booster=debug gnome-foreground-booster
```

Each switch prints a line such as `boosted unit=app-flatpak-org.mozilla.firefox-270084972.scope weight=400`. You can also read a unit's weight directly; it shows `400` while the app has focus and `[not set]` after it loses it:

```sh
systemctl --user show app-flatpak-org.mozilla.firefox-270084972.scope -p CPUWeight --value
```

### Options

| Flag | Default | Meaning |
| --- | --- | --- |
| `--weight` | `400` | `CPUWeight` given to the focused app. The systemd default is `100` and the range is 1 to 10000. |
| `--debounce-ms` | `150` | How long focus must stay on a window before the boost moves, so Alt+Tab does not churn units. |

To change them, run `systemctl --user edit gnome-foreground-booster.service` and override the command line:

```ini
[Service]
ExecStart=
ExecStart=/usr/bin/gnome-foreground-booster --weight 800
```

Use `~/.local/bin/gnome-foreground-booster` instead if you installed with `just install-user`.

## How it works

Since GNOME 41, Mutter sets the `user.xdg.inactive-since` extended attribute on the cgroup of the focused window's systemd unit: `-1` on the focused unit and a timestamp on the unit that lost focus. The daemon watches those attributes with inotify and asks the systemd user manager to change `CPUWeight` on the focused app.

```
Mutter (GNOME Shell)
    │  setxattr user.xdg.inactive-since on …/app.slice/<unit>
    ▼
gnome-foreground-booster      inotify on app.slice and its sub-slices
    │  SetUnitProperties(CPUWeight, runtime)
    ▼
systemd --user
```

cgroup v2 weights only compete between siblings, so the weight goes to the direct child of `app.slice` that contains the focused unit. Changes are runtime-only and are undone on SIGTERM, SIGINT and SIGHUP. When nothing has focus, for example in the overview, the boost is released.

GNOME Shell moves a launched app into its own scope asynchronously, after the process has started. A launcher that forks right away, as AppImages do, leaves the real app behind in the Shell's cgroup, where Mutter attributes its windows to the Shell. The daemon looks for processes in `org.gnome.Shell@*.service` whose parent sits in an app unit, along with their descendants, and moves them into a new scope next to the launcher's with `StartTransientUnit`. It does this at startup and each time a new app unit appears. The Shell and the helpers it starts itself are left alone.

Only the current user's `user@<uid>.service/app.slice` is watched. Any process of your user could set the same attribute to pick which of your own units gets boosted, which it could already do through systemd directly.

## Limitations

> [!IMPORTANT]
> Terminals boost themselves, not your commands. Ptyxis and other VTE-based terminals start each shell in its own `*-spawn-*.scope`, a sibling of the terminal's unit, so a build started from a focused terminal does not get the extra weight.

- Mutter remembers a window's unit the first time it sees it. A window that was open before the daemon moved its process out of the Shell's cgroup keeps counting as the Shell until it is closed and reopened. New windows of the same app are fine.
- A stranded process whose launcher has already exited cannot be attributed to an app, so it stays in the Shell's cgroup.
- After a crash, a non-transient unit, such as a D-Bus activated service, can keep its boost until the daemon starts again or you log out. Transient units drop it when they exit.
- The shipped unit uses `ProtectSystem=` and `PrivateTmp=`, which need unprivileged user namespaces. Remove those lines from the unit if your system disallows them.

## Development

```sh
just check                                         # rustfmt, clippy, unit tests
just build && BIN=build/gnome-foreground-booster just e2e
```

The unit tests cover cgroup path handling, focus tracking, the rescue planner and the boost, restore and recovery logic against a fake unit controller. The end-to-end script needs `systemd --user` and `python3`. It starts throwaway units in a private slice, sets the attribute the way Mutter does and imitates the Shell's launch race with a fake Shell unit, then runs the real daemon against the real user manager. It does not touch your running applications, and it refuses to start while another instance of the daemon is running.
