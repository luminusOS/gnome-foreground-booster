use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::focus::{Kind, kind_of};

pub trait ScopeControl {
    async fn start_scope(
        &self,
        name: &str,
        slice: &str,
        description: &str,
        pids: &[u32],
    ) -> Result<()>;
}

#[derive(Debug, Clone, Copy)]
pub struct Member {
    pub pid: u32,
    pub ppid: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Rescue {
    pub launcher: PathBuf,
    pub pids: Vec<u32>,
}

impl Rescue {
    pub fn slice(&self) -> Option<&str> {
        self.launcher.parent()?.file_name()?.to_str()
    }

    pub fn launcher_name(&self) -> Option<&str> {
        self.launcher.file_name()?.to_str()
    }

    pub fn scope_name(&self) -> Option<String> {
        let launcher = self.launcher_name()?;
        let stem = launcher.rsplit_once('.').map_or(launcher, |(stem, _)| stem);
        let stem: String = stem
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '.' | '-' | '\\') {
                    c
                } else {
                    '_'
                }
            })
            .take(200)
            .collect();
        let root = self.pids.first()?;
        Some(format!("{stem}-{root}.scope"))
    }
}

// GNOME Shell moves an app it launched into its own scope only after the
// spawn, asynchronously. A launcher that forks right away (AppImages do)
// leaves those children behind in the Shell's cgroup, so Mutter attributes
// their windows to the Shell. Children whose parent already sits in an app
// unit, and their descendants, are the ones to move back.
pub fn plan(members: &[Member], unit_of: impl Fn(u32) -> Option<PathBuf>) -> Vec<Rescue> {
    let in_shell: HashSet<u32> = members.iter().map(|m| m.pid).collect();
    let mut owner: HashMap<u32, PathBuf> = HashMap::new();

    for member in members {
        if !in_shell.contains(&member.ppid) {
            if let Some(unit) = unit_of(member.ppid) {
                owner.insert(member.pid, unit);
            }
        }
    }

    loop {
        let mut changed = false;
        for member in members {
            if owner.contains_key(&member.pid) {
                continue;
            }
            if let Some(unit) = owner.get(&member.ppid).cloned() {
                owner.insert(member.pid, unit);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    let mut groups: BTreeMap<PathBuf, Vec<u32>> = BTreeMap::new();
    for (pid, unit) in owner {
        groups.entry(unit).or_default().push(pid);
    }
    groups
        .into_iter()
        .map(|(launcher, mut pids)| {
            pids.sort_unstable();
            Rescue { launcher, pids }
        })
        .collect()
}

pub fn unit_in(app_slice: &Path, cgroup_dir: &Path) -> Option<PathBuf> {
    let relative = cgroup_dir.strip_prefix(app_slice).ok()?;
    let mut unit = app_slice.to_path_buf();
    for component in relative.components() {
        let name = component.as_os_str();
        unit.push(name);
        match kind_of(name)? {
            Kind::Slice => continue,
            Kind::Unit => return Some(unit),
        }
    }
    None
}

fn parse_ppid(stat: &str) -> Option<u32> {
    let after_comm = &stat[stat.rfind(')')? + 1..];
    after_comm.split_whitespace().nth(1)?.parse().ok()
}

fn cgroup_dir_of(pid: u32) -> Option<PathBuf> {
    let content = fs::read_to_string(format!("/proc/{pid}/cgroup")).ok()?;
    let path = content.lines().find_map(|line| line.strip_prefix("0::"))?;
    Some(PathBuf::from(format!("/sys/fs/cgroup{path}")))
}

fn members_of(cgroup_dir: &Path) -> Vec<Member> {
    let Ok(procs) = fs::read_to_string(cgroup_dir.join("cgroup.procs")) else {
        return Vec::new();
    };
    procs
        .lines()
        .filter_map(|line| line.trim().parse().ok())
        .filter_map(|pid: u32| {
            let stat = fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            Some(Member {
                pid,
                ppid: parse_ppid(&stat)?,
            })
        })
        .collect()
}

pub struct Rescuer {
    app_slice: PathBuf,
    session_slice: PathBuf,
    shell_cgroup: Option<PathBuf>,
}

impl Rescuer {
    pub fn new(app_slice: PathBuf, session_slice: PathBuf, shell_cgroup: Option<PathBuf>) -> Self {
        Self {
            app_slice,
            session_slice,
            shell_cgroup,
        }
    }

    fn shell_cgroups(&self) -> Vec<PathBuf> {
        if let Some(dir) = &self.shell_cgroup {
            return vec![dir.clone()];
        }
        let Ok(entries) = fs::read_dir(&self.session_slice) else {
            return Vec::new();
        };
        entries
            .flatten()
            .filter(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.starts_with("org.gnome.Shell@") && name.ends_with(".service")
            })
            .map(|entry| entry.path())
            .collect()
    }

    pub async fn run<C: ScopeControl>(&self, control: &C) {
        for shell in self.shell_cgroups() {
            let members = members_of(&shell);
            let rescues = plan(&members, |ppid| {
                unit_in(&self.app_slice, &cgroup_dir_of(ppid)?)
            });

            for rescue in rescues {
                let (Some(name), Some(slice), Some(launcher)) =
                    (rescue.scope_name(), rescue.slice(), rescue.launcher_name())
                else {
                    continue;
                };
                let description = format!("Processes of {launcher} left in GNOME Shell's cgroup");
                match control
                    .start_scope(&name, slice, &description, &rescue.pids)
                    .await
                {
                    Ok(()) => tracing::info!(
                        %launcher,
                        scope = %name,
                        pids = ?rescue.pids,
                        "moved processes out of GNOME Shell's cgroup"
                    ),
                    Err(error) => tracing::debug!(%launcher, %error, "could not move processes"),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const APP: &str = "/cg/app.slice";

    fn member(pid: u32, ppid: u32) -> Member {
        Member { pid, ppid }
    }

    fn launcher_units(pid: u32) -> Option<PathBuf> {
        match pid {
            17197 => Some(PathBuf::from(format!(
                "{APP}/app-gnome-ghostty-17197.scope"
            ))),
            900 => Some(PathBuf::from(format!(
                "{APP}/app-x.slice/launcher@1.service"
            ))),
            _ => None,
        }
    }

    #[test]
    fn moves_children_of_a_launcher_that_sits_in_its_scope() {
        let members = [
            member(5470, 1519),
            member(8797, 5470),
            member(17199, 17197),
            member(17226, 17197),
        ];

        let rescues = plan(&members, launcher_units);

        assert_eq!(
            rescues,
            vec![Rescue {
                launcher: PathBuf::from(format!("{APP}/app-gnome-ghostty-17197.scope")),
                pids: vec![17199, 17226],
            }]
        );
        assert_eq!(rescues[0].slice(), Some("app.slice"));
        assert_eq!(
            rescues[0].scope_name().as_deref(),
            Some("app-gnome-ghostty-17197-17199.scope")
        );
    }

    #[test]
    fn follows_descendants_that_stayed_behind() {
        let members = [
            member(5470, 1519),
            member(17226, 17197),
            member(30000, 17226),
            member(30001, 30000),
        ];

        let rescues = plan(&members, launcher_units);

        assert_eq!(rescues.len(), 1);
        assert_eq!(rescues[0].pids, vec![17226, 30000, 30001]);
    }

    #[test]
    fn leaves_the_shell_and_its_own_helpers_alone() {
        let members = [
            member(5470, 1519),
            member(8797, 5470),
            member(52107, 5470),
            member(52113, 52107),
            member(2814393, 1519),
        ];

        assert!(plan(&members, launcher_units).is_empty());
    }

    #[test]
    fn keeps_the_launcher_slice_and_sanitizes_the_name() {
        let rescues = plan(&[member(901, 900)], launcher_units);

        assert_eq!(rescues[0].slice(), Some("app-x.slice"));
        assert_eq!(
            rescues[0].scope_name().as_deref(),
            Some("launcher_1-901.scope")
        );
    }

    #[test]
    fn resolves_the_unit_that_contains_a_cgroup() {
        let app = Path::new(APP);
        assert_eq!(
            unit_in(app, &app.join("app-gnome-ghostty-17197.scope")),
            Some(app.join("app-gnome-ghostty-17197.scope"))
        );
        assert_eq!(
            unit_in(app, &app.join("app-x.slice/y.service/inner")),
            Some(app.join("app-x.slice/y.service"))
        );
        assert_eq!(unit_in(app, &app.join("app-x.slice")), None);
        assert_eq!(
            unit_in(
                app,
                Path::new("/cg/session.slice/org.gnome.Shell@user.service")
            ),
            None
        );
    }

    #[test]
    fn parses_the_parent_pid_from_stat() {
        assert_eq!(parse_ppid("17226 (ghostty) S 17197 17226 ..."), Some(17197));
        assert_eq!(parse_ppid("42 (a) b (c)) R 7 42 ..."), Some(7));
        assert_eq!(parse_ppid("garbage"), None);
    }
}
