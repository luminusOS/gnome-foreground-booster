use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;

const APP_SLICE: &str = "app.slice";

pub fn current_uid() -> io::Result<u32> {
    Ok(fs::metadata("/proc/self")?.uid())
}

pub fn unit_for_pid(pid: u32, uid: u32) -> io::Result<Option<String>> {
    let content = fs::read_to_string(format!("/proc/{pid}/cgroup"))?;
    Ok(parse_app_unit(&content, uid))
}

pub fn parse_app_unit(proc_cgroup: &str, uid: u32) -> Option<String> {
    let path = proc_cgroup
        .lines()
        .find_map(|line| line.strip_prefix("0::"))?;

    let manager = format!("user@{uid}.service");
    let mut components = path.split('/').filter(|c| !c.is_empty());
    components.find(|c| *c == manager)?;

    if components.next()? != APP_SLICE {
        return None;
    }

    components.next().filter(|c| is_unit(c)).map(str::to_owned)
}

fn is_unit(component: &str) -> bool {
    [".scope", ".service", ".slice"]
        .iter()
        .any(|suffix| component.ends_with(suffix))
}

pub fn user_manager_delegates_cpu() -> bool {
    let Ok(content) = fs::read_to_string("/proc/self/cgroup") else {
        return false;
    };
    let Some(path) = content.lines().find_map(|line| line.strip_prefix("0::")) else {
        return false;
    };

    let Some(end) = path.find("/user@") else {
        return false;
    };
    let rest = &path[end + 1..];
    let manager = rest.split('/').next().unwrap_or(rest);
    let manager_path = format!("{}/{manager}", &path[..end]);

    fs::read_to_string(format!("/sys/fs/cgroup{manager_path}/cgroup.controllers"))
        .map(|controllers| controllers.split_whitespace().any(|c| c == "cpu"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const UID: u32 = 1000;

    #[test]
    fn resolves_gnome_launched_scope() {
        let line = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-firefox-4242.scope\n";
        assert_eq!(
            parse_app_unit(line, UID).as_deref(),
            Some("app-gnome-firefox-4242.scope")
        );
    }

    #[test]
    fn resolves_the_direct_child_of_app_slice_when_nested() {
        let line = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-org.gnome.Terminal.slice/gnome-terminal-server.service\n";
        assert_eq!(
            parse_app_unit(line, UID).as_deref(),
            Some("app-org.gnome.Terminal.slice")
        );
    }

    #[test]
    fn stops_at_the_unit_when_the_process_sits_in_a_sub_cgroup() {
        let line = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-flatpak-org.mozilla.firefox-9.scope/inner/leaf\n";
        assert_eq!(
            parse_app_unit(line, UID).as_deref(),
            Some("app-flatpak-org.mozilla.firefox-9.scope")
        );
    }

    #[test]
    fn keeps_escaped_unit_names_verbatim() {
        let line = "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-dbus\\x2d:1.2\\x2dorg.gnome.Ptyxis@0.service\n";
        assert_eq!(
            parse_app_unit(line, UID).as_deref(),
            Some("app-dbus\\x2d:1.2\\x2dorg.gnome.Ptyxis@0.service")
        );
    }

    #[test]
    fn ignores_session_slice_processes() {
        let line = "0::/user.slice/user-1000.slice/user@1000.service/session.slice/org.gnome.Shell@wayland.service\n";
        assert_eq!(parse_app_unit(line, UID), None);
    }

    #[test]
    fn ignores_processes_outside_the_user_manager() {
        let line = "0::/user.slice/user-1000.slice/session-2.scope\n";
        assert_eq!(parse_app_unit(line, UID), None);
    }

    #[test]
    fn ignores_units_of_another_users_manager() {
        let line = "0::/user.slice/user-1001.slice/user@1001.service/app.slice/app-gnome-firefox-4242.scope\n";
        assert_eq!(parse_app_unit(line, UID), None);
    }

    #[test]
    fn ignores_app_slice_itself() {
        let line = "0::/user.slice/user-1000.slice/user@1000.service/app.slice\n";
        assert_eq!(parse_app_unit(line, UID), None);
    }

    #[test]
    fn ignores_cgroup_v1_lines() {
        let line = "12:cpu:/user.slice\n";
        assert_eq!(parse_app_unit(line, UID), None);
    }
}
