use anyhow::{Result, anyhow, bail};
use zbus::proxy::{Builder, CacheProperties};
use zbus::zvariant::{OwnedObjectPath, Value};
use zbus::{Connection, Proxy};

use crate::booster::{UnitControl, UnitGone};
use crate::rescue::ScopeControl;

const SYSTEMD_BUS_NAME: &str = "org.freedesktop.systemd1";
const NO_SUCH_UNIT: &str = "org.freedesktop.systemd1.NoSuchUnit";

#[zbus::proxy(
    interface = "org.freedesktop.systemd1.Manager",
    default_service = "org.freedesktop.systemd1",
    default_path = "/org/freedesktop/systemd1"
)]
trait Manager {
    fn get_unit(&self, name: &str) -> zbus::Result<OwnedObjectPath>;

    fn set_unit_properties(
        &self,
        name: &str,
        runtime: bool,
        properties: &[(&str, Value<'_>)],
    ) -> zbus::Result<()>;

    fn start_transient_unit(
        &self,
        name: &str,
        mode: &str,
        properties: &[(&str, Value<'_>)],
        aux: &[(&str, &[(&str, Value<'_>)])],
    ) -> zbus::Result<OwnedObjectPath>;
}

pub struct SystemdUser {
    connection: Connection,
    manager: ManagerProxy<'static>,
}

impl SystemdUser {
    pub async fn new(connection: Connection) -> Result<Self> {
        let manager = ManagerProxy::new(&connection).await?;
        Ok(Self {
            connection,
            manager,
        })
    }
}

fn unit_interface(unit: &str) -> Result<&'static str> {
    if unit.ends_with(".scope") {
        Ok("org.freedesktop.systemd1.Scope")
    } else if unit.ends_with(".service") {
        Ok("org.freedesktop.systemd1.Service")
    } else if unit.ends_with(".slice") {
        Ok("org.freedesktop.systemd1.Slice")
    } else {
        bail!("unsupported unit type: {unit}")
    }
}

fn classify(error: zbus::Error) -> anyhow::Error {
    match &error {
        zbus::Error::MethodError(name, _, _) if name.as_str() == NO_SUCH_UNIT => {
            anyhow!(UnitGone)
        }
        _ => anyhow!(error),
    }
}

impl UnitControl for SystemdUser {
    async fn cpu_weight(&self, unit: &str) -> Result<u64> {
        let path = self.manager.get_unit(unit).await.map_err(classify)?;
        let proxy: Proxy<'_> = Builder::new(&self.connection)
            .destination(SYSTEMD_BUS_NAME)?
            .path(path)?
            .interface(unit_interface(unit)?)?
            .cache_properties(CacheProperties::No)
            .build()
            .await?;
        Ok(proxy.get_property::<u64>("CPUWeight").await?)
    }

    async fn set_cpu_weight(&self, unit: &str, weight: u64) -> Result<()> {
        self.manager
            .set_unit_properties(unit, true, &[("CPUWeight", Value::from(weight))])
            .await
            .map_err(classify)
    }
}

impl ScopeControl for SystemdUser {
    async fn start_scope(
        &self,
        name: &str,
        slice: &str,
        description: &str,
        pids: &[u32],
    ) -> Result<()> {
        self.manager
            .start_transient_unit(
                name,
                "fail",
                &[
                    ("PIDs", Value::from(pids.to_vec())),
                    ("Slice", Value::from(slice)),
                    ("Description", Value::from(description)),
                ],
                &[],
            )
            .await?;
        Ok(())
    }
}
