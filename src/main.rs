mod booster;
mod bridge;
mod cgroup;
mod systemd;

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use clap::Parser;
use futures_util::StreamExt;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{Instant, sleep_until};
use zbus::connection;

use crate::booster::Booster;
use crate::bridge::{BOOSTER_BUS_NAME, ForegroundProxy};
use crate::systemd::SystemdUser;

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// CPUWeight applied to the focused application's systemd unit (default is 100)
    #[arg(long, default_value_t = 400, value_parser = clap::value_parser!(u64).range(1..=10000))]
    weight: u64,

    /// Time focus must stay on a window before the boost moves to it
    #[arg(long, default_value_t = 150)]
    debounce_ms: u64,
}

fn unit_for_focus(pid: u32, uid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    match cgroup::unit_for_pid(pid, uid) {
        Ok(unit) => unit,
        Err(error) => {
            tracing::debug!(pid, %error, "could not read the cgroup of the focused process");
            None
        }
    }
}

fn state_path() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("RUNTIME_DIRECTORY") {
        return Some(PathBuf::from(dir).join("state"));
    }
    env::var_os("XDG_RUNTIME_DIR")
        .map(|dir| PathBuf::from(dir).join("gnome-foreground-booster/state"))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt()
        .without_time()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gnome_foreground_booster=info".into()),
        )
        .init();

    if !cgroup::user_manager_delegates_cpu() {
        tracing::warn!(
            "the systemd user manager does not have the cpu controller delegated; \
             boosts will have no effect (see README: Delegate=cpu)"
        );
    }

    let uid = cgroup::current_uid().context("reading the current uid")?;

    let connection = connection::Builder::session()
        .context("connecting to the session bus")?
        .build()
        .await?;

    let bridge = ForegroundProxy::new(&connection).await?;
    let mut changes = bridge.receive_focus_changed().await?;
    let mut owner_changes = bridge.inner().receive_owner_changed().await?;

    let mut booster = Booster::new(
        SystemdUser::new(connection.clone())
            .await
            .context("connecting to the systemd user manager")?,
        args.weight,
        state_path(),
    );
    booster.recover().await;

    connection
        .request_name(BOOSTER_BUS_NAME)
        .await
        .context("another gnome-foreground-booster is already running")?;

    let debounce = Duration::from_millis(args.debounce_ms);

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sighup = signal(SignalKind::hangup())?;

    let mut pending: Option<(u32, Instant)> = None;

    tracing::info!(weight = args.weight, "running");

    let result: Result<()> = loop {
        let deadline = pending.map(|(_, at)| at);
        tokio::select! {
            focus = changes.next() => {
                let Some(focus) = focus else {
                    break Err(anyhow!("the FocusChanged stream ended"));
                };
                match focus.args() {
                    Ok(args) => pending = Some((args.pid, Instant::now() + debounce)),
                    Err(error) => tracing::warn!(%error, "malformed FocusChanged"),
                }
            }
            owner = owner_changes.next() => {
                let Some(owner) = owner else {
                    break Err(anyhow!("the bridge owner stream ended"));
                };
                if owner.is_none() {
                    tracing::debug!("bridge left the bus; releasing the boost");
                    pending = None;
                    booster.release().await;
                }
            }
            _ = async { sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                if let Some((pid, _)) = pending.take() {
                    booster.focus(unit_for_focus(pid, uid)).await;
                }
            }
            _ = sigterm.recv() => break Ok(()),
            _ = sigint.recv() => break Ok(()),
            _ = sighup.recv() => break Ok(()),
        }
    };

    booster.release().await;
    result
}
