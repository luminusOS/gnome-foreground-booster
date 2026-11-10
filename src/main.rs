mod booster;
mod cgroup;
mod focus;
mod systemd;

use std::env;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::signal::unix::{SignalKind, signal};
use tokio::time::{Instant, sleep_until};
use zbus::connection;

use crate::booster::Booster;
use crate::focus::FocusWatcher;
use crate::systemd::SystemdUser;

const BUS_NAME: &str = "org.luminusos.ForegroundBooster";

#[derive(Parser)]
#[command(version, about)]
struct Args {
    /// CPUWeight applied to the focused application's systemd unit (default is 100)
    #[arg(long, default_value_t = 400, value_parser = clap::value_parser!(u64).range(1..=10000))]
    weight: u64,

    /// Time focus must stay on a window before the boost moves to it
    #[arg(long, default_value_t = 150)]
    debounce_ms: u64,

    /// Slice cgroup to watch instead of the user's app.slice (for tests)
    #[arg(long, hide = true)]
    app_slice: Option<PathBuf>,
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

    let uid = cgroup::current_uid().context("reading the current uid")?;
    let manager_dir = cgroup::user_manager_dir(uid);

    if !cgroup::delegates_cpu(&manager_dir) {
        tracing::warn!(
            "the systemd user manager does not have the cpu controller delegated; \
             boosts will have no effect (see README: Delegate=cpu)"
        );
    }

    let mut watcher = FocusWatcher::new(
        args.app_slice
            .unwrap_or_else(|| manager_dir.join("app.slice")),
    )?;

    let connection = connection::Builder::session()
        .context("connecting to the session bus")?
        .build()
        .await?;
    connection
        .request_name(BUS_NAME)
        .await
        .context("another gnome-foreground-booster is already running")?;

    let mut booster = Booster::new(
        SystemdUser::new(connection.clone())
            .await
            .context("connecting to the systemd user manager")?,
        args.weight,
        state_path(),
    );
    booster.recover().await;

    let debounce = Duration::from_millis(args.debounce_ms);

    let mut sigterm = signal(SignalKind::terminate())?;
    let mut sigint = signal(SignalKind::interrupt())?;
    let mut sighup = signal(SignalKind::hangup())?;

    let mut pending = Some((watcher.current(), Instant::now()));

    tracing::info!(weight = args.weight, "running");

    let result: Result<()> = loop {
        let deadline = pending.as_ref().map(|(_, at)| *at);
        tokio::select! {
            change = watcher.next() => match change {
                Ok(unit) => pending = Some((unit, Instant::now() + debounce)),
                Err(error) => break Err(error.context("watching focus changes")),
            },
            _ = async { sleep_until(deadline.unwrap()).await }, if deadline.is_some() => {
                if let Some((unit, _)) = pending.take() {
                    booster.focus(unit).await;
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
