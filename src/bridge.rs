#[zbus::proxy(
    interface = "org.luminusos.AuroraShell.Foreground",
    default_service = "org.luminusos.AuroraShell.Foreground",
    default_path = "/org/luminusos/AuroraShell/Foreground"
)]
pub trait Foreground {
    #[zbus(signal)]
    fn focus_changed(&self, pid: u32, app_id: String) -> zbus::Result<()>;
}

pub const BOOSTER_BUS_NAME: &str = "org.luminusos.ForegroundBooster";
