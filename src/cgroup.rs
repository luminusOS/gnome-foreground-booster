use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const CGROUP_ROOT: &str = "/sys/fs/cgroup";

pub fn current_uid() -> io::Result<u32> {
    Ok(fs::metadata("/proc/self")?.uid())
}

pub fn user_manager_dir(uid: u32) -> PathBuf {
    let relative = fs::read_to_string("/proc/self/cgroup")
        .ok()
        .and_then(|content| manager_path(&content, uid))
        .unwrap_or_else(|| format!("/user.slice/user-{uid}.slice/user@{uid}.service"));
    PathBuf::from(format!("{CGROUP_ROOT}{relative}"))
}

pub fn manager_path(proc_cgroup: &str, uid: u32) -> Option<String> {
    let path = proc_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?;
    let manager = format!("/user@{uid}.service");
    let end = path.find(&manager)? + manager.len();
    let rest = &path[end..];
    (rest.is_empty() || rest.starts_with('/')).then(|| path[..end].to_owned())
}

pub fn delegates_cpu(manager_dir: &Path) -> bool {
    fs::read_to_string(manager_dir.join("cgroup.controllers"))
        .map(|controllers| controllers.split_whitespace().any(|c| c == "cpu"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_manager_of_a_process_in_session_slice() {
        let line = "0::/user.slice/user-1000.slice/user@1000.service/session.slice/gnome-foreground-booster.service\n";
        assert_eq!(
            manager_path(line, 1000).as_deref(),
            Some("/user.slice/user-1000.slice/user@1000.service")
        );
    }

    #[test]
    fn rejects_another_users_manager() {
        let line = "0::/user.slice/user-1001.slice/user@1001.service/app.slice/x.scope\n";
        assert_eq!(manager_path(line, 1000), None);
    }

    #[test]
    fn does_not_match_a_longer_uid_prefix() {
        let line = "0::/user.slice/user-10000.slice/user@10000.service/app.slice/x.scope\n";
        assert_eq!(manager_path(line, 1000), None);
    }

    #[test]
    fn ignores_processes_outside_a_user_manager() {
        let line = "0::/user.slice/user-1000.slice/session-2.scope\n";
        assert_eq!(manager_path(line, 1000), None);
    }

    #[test]
    fn ignores_cgroup_v1_lines() {
        assert_eq!(manager_path("12:cpu:/user.slice\n", 1000), None);
    }
}
