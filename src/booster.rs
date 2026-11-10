use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;

#[derive(Debug)]
pub struct UnitGone;

impl fmt::Display for UnitGone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("unit no longer exists")
    }
}

impl std::error::Error for UnitGone {}

pub trait UnitControl {
    async fn cpu_weight(&self, unit: &str) -> Result<u64>;
    async fn set_cpu_weight(&self, unit: &str, weight: u64) -> Result<()>;
}

#[derive(Debug, Clone, PartialEq)]
struct Boosted {
    unit: String,
    original_weight: u64,
    boost_weight: u64,
}

impl Boosted {
    fn serialize(&self) -> String {
        format!(
            "{}\n{}\n{}\n",
            self.unit, self.original_weight, self.boost_weight
        )
    }

    fn parse(content: &str) -> Option<Self> {
        let mut lines = content.lines();
        let unit = lines.next().filter(|unit| !unit.is_empty())?.to_owned();
        let original_weight = lines.next()?.parse().ok()?;
        let boost_weight = lines.next()?.parse().ok()?;
        Some(Self {
            unit,
            original_weight,
            boost_weight,
        })
    }
}

pub struct Booster<C> {
    control: C,
    boost_weight: u64,
    state_path: Option<PathBuf>,
    current: Option<Boosted>,
}

impl<C: UnitControl> Booster<C> {
    pub fn new(control: C, boost_weight: u64, state_path: Option<PathBuf>) -> Self {
        Self {
            control,
            boost_weight,
            state_path,
            current: None,
        }
    }

    pub async fn recover(&mut self) {
        let Some(path) = self.state_path.as_deref() else {
            return;
        };
        let Ok(content) = fs::read_to_string(path) else {
            return;
        };

        if let Some(stale) = Boosted::parse(&content) {
            tracing::info!(unit = %stale.unit, "restoring a boost left by a previous run");
            self.restore(&stale).await;
        }
        self.persist();
    }

    pub async fn focus(&mut self, unit: Option<String>) {
        if self.current.as_ref().map(|b| b.unit.as_str()) == unit.as_deref() {
            return;
        }

        self.release().await;

        if let Some(unit) = unit {
            self.boost(unit).await;
        }
    }

    pub async fn release(&mut self) {
        let Some(boosted) = self.current.take() else {
            return;
        };

        self.restore(&boosted).await;
        self.persist();
    }

    async fn boost(&mut self, unit: String) {
        let original_weight = match self.control.cpu_weight(&unit).await {
            Ok(weight) => weight,
            Err(error) => {
                log_failure("could not read CPUWeight", &unit, &error);
                return;
            }
        };

        if let Err(error) = self.control.set_cpu_weight(&unit, self.boost_weight).await {
            log_failure("could not boost CPUWeight", &unit, &error);
            return;
        }

        tracing::debug!(%unit, weight = self.boost_weight, "boosted");
        self.current = Some(Boosted {
            unit,
            original_weight,
            boost_weight: self.boost_weight,
        });
        self.persist();
    }

    async fn restore(&self, boosted: &Boosted) {
        match self.control.cpu_weight(&boosted.unit).await {
            Ok(weight) if weight == boosted.boost_weight => {
                if let Err(error) = self
                    .control
                    .set_cpu_weight(&boosted.unit, boosted.original_weight)
                    .await
                {
                    log_failure("could not restore CPUWeight", &boosted.unit, &error);
                }
            }
            Ok(weight) => {
                tracing::debug!(
                    unit = %boosted.unit,
                    weight,
                    "CPUWeight changed by someone else; leaving it"
                );
            }
            Err(error) => log_failure("could not read CPUWeight", &boosted.unit, &error),
        }
    }

    fn persist(&self) {
        let Some(path) = self.state_path.as_deref() else {
            return;
        };

        let result = match &self.current {
            Some(boosted) => write_atomically(path, &boosted.serialize()),
            None => match fs::remove_file(path) {
                Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
                _ => Ok(()),
            },
        };

        if let Err(error) = result {
            tracing::warn!(path = %path.display(), %error, "could not update the state file");
        }
    }
}

fn write_atomically(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, content)?;
    fs::rename(temporary, path)
}

fn log_failure(message: &str, unit: &str, error: &anyhow::Error) {
    if error.downcast_ref::<UnitGone>().is_some() {
        tracing::debug!(%unit, "unit is gone");
    } else {
        tracing::warn!(%unit, %error, "{message}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;
    use std::cell::RefCell;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU32, Ordering};

    const UNSET_WEIGHT: u64 = u64::MAX;

    #[derive(Default)]
    struct FakeControl {
        weights: RefCell<HashMap<String, u64>>,
        gone: RefCell<Vec<String>>,
    }

    impl FakeControl {
        fn with(unit: &str, weight: u64) -> Self {
            let fake = Self::default();
            fake.add(unit, weight);
            fake
        }

        fn add(&self, unit: &str, weight: u64) {
            self.weights.borrow_mut().insert(unit.into(), weight);
        }

        fn weight(&self, unit: &str) -> u64 {
            self.weights.borrow()[unit]
        }

        fn is_gone(&self, unit: &str) -> bool {
            self.gone.borrow().iter().any(|u| u == unit)
        }
    }

    impl UnitControl for &FakeControl {
        async fn cpu_weight(&self, unit: &str) -> Result<u64> {
            if self.is_gone(unit) {
                return Err(UnitGone.into());
            }
            self.weights
                .borrow()
                .get(unit)
                .copied()
                .ok_or_else(|| anyhow!("no such unit"))
        }

        async fn set_cpu_weight(&self, unit: &str, weight: u64) -> Result<()> {
            if self.is_gone(unit) {
                return Err(UnitGone.into());
            }
            self.weights.borrow_mut().insert(unit.into(), weight);
            Ok(())
        }
    }

    struct TempState(PathBuf);

    impl TempState {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!("gfb-test-{}-{id}", std::process::id()));
            Self(dir.join("state"))
        }

        fn path(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }

        fn exists(&self) -> bool {
            self.0.exists()
        }
    }

    impl Drop for TempState {
        fn drop(&mut self) {
            if let Some(dir) = self.0.parent() {
                let _ = fs::remove_dir_all(dir);
            }
        }
    }

    fn unit(name: &str) -> Option<String> {
        Some(name.to_owned())
    }

    #[tokio::test]
    async fn boosts_the_focused_unit() {
        let fake = FakeControl::with("a.scope", UNSET_WEIGHT);
        let mut booster = Booster::new(&fake, 400, None);

        booster.focus(unit("a.scope")).await;

        assert_eq!(fake.weight("a.scope"), 400);
    }

    #[tokio::test]
    async fn restores_the_previous_unit_when_focus_moves() {
        let fake = FakeControl::with("a.scope", UNSET_WEIGHT);
        fake.add("b.scope", 50);
        let mut booster = Booster::new(&fake, 400, None);

        booster.focus(unit("a.scope")).await;
        booster.focus(unit("b.scope")).await;

        assert_eq!(fake.weight("a.scope"), UNSET_WEIGHT);
        assert_eq!(fake.weight("b.scope"), 400);
    }

    #[tokio::test]
    async fn restores_the_user_configured_weight() {
        let fake = FakeControl::with("a.scope", 50);
        let mut booster = Booster::new(&fake, 400, None);

        booster.focus(unit("a.scope")).await;
        booster.focus(None).await;

        assert_eq!(fake.weight("a.scope"), 50);
    }

    #[tokio::test]
    async fn refocusing_the_same_unit_does_not_overwrite_the_saved_weight() {
        let fake = FakeControl::with("a.scope", 50);
        let mut booster = Booster::new(&fake, 400, None);

        booster.focus(unit("a.scope")).await;
        booster.focus(unit("a.scope")).await;
        booster.focus(None).await;

        assert_eq!(fake.weight("a.scope"), 50);
    }

    #[tokio::test]
    async fn focus_without_an_app_unit_releases_the_boost() {
        let fake = FakeControl::with("a.scope", UNSET_WEIGHT);
        let mut booster = Booster::new(&fake, 400, None);

        booster.focus(unit("a.scope")).await;
        booster.focus(None).await;

        assert_eq!(fake.weight("a.scope"), UNSET_WEIGHT);
    }

    #[tokio::test]
    async fn a_vanished_previous_unit_does_not_block_the_next_boost() {
        let fake = FakeControl::with("a.scope", UNSET_WEIGHT);
        fake.add("b.scope", UNSET_WEIGHT);
        let mut booster = Booster::new(&fake, 400, None);

        booster.focus(unit("a.scope")).await;
        fake.gone.borrow_mut().push("a.scope".into());
        booster.focus(unit("b.scope")).await;

        assert_eq!(fake.weight("b.scope"), 400);
    }

    #[tokio::test]
    async fn an_unknown_unit_is_not_tracked() {
        let fake = FakeControl::default();
        let mut booster = Booster::new(&fake, 400, None);

        booster.focus(unit("ghost.scope")).await;

        assert!(booster.current.is_none());
    }

    #[tokio::test]
    async fn release_leaves_a_weight_changed_by_someone_else() {
        let fake = FakeControl::with("a.scope", UNSET_WEIGHT);
        let mut booster = Booster::new(&fake, 400, None);

        booster.focus(unit("a.scope")).await;
        fake.add("a.scope", 700);
        booster.release().await;

        assert_eq!(fake.weight("a.scope"), 700);
    }

    #[tokio::test]
    async fn state_file_tracks_the_active_boost() {
        let state = TempState::new();
        let fake = FakeControl::with("a.scope", 50);
        let mut booster = Booster::new(&fake, 400, state.path());

        booster.focus(unit("a.scope")).await;
        assert!(state.exists());

        booster.focus(None).await;
        assert!(!state.exists());
    }

    #[tokio::test]
    async fn recover_restores_a_boost_left_by_a_previous_run() {
        let state = TempState::new();
        let fake = FakeControl::with("a.scope", 50);
        let mut crashed = Booster::new(&fake, 400, state.path());
        crashed.focus(unit("a.scope")).await;
        drop(crashed);
        assert_eq!(fake.weight("a.scope"), 400);

        let mut restarted = Booster::new(&fake, 400, state.path());
        restarted.recover().await;

        assert_eq!(fake.weight("a.scope"), 50);
        assert!(!state.exists());
    }

    #[tokio::test]
    async fn recover_leaves_a_weight_changed_since_the_crash() {
        let state = TempState::new();
        let fake = FakeControl::with("a.scope", 50);
        let mut crashed = Booster::new(&fake, 400, state.path());
        crashed.focus(unit("a.scope")).await;
        drop(crashed);
        fake.add("a.scope", 900);

        let mut restarted = Booster::new(&fake, 400, state.path());
        restarted.recover().await;

        assert_eq!(fake.weight("a.scope"), 900);
        assert!(!state.exists());
    }

    #[tokio::test]
    async fn recover_clears_state_for_a_vanished_unit() {
        let state = TempState::new();
        let fake = FakeControl::with("a.scope", 50);
        let mut crashed = Booster::new(&fake, 400, state.path());
        crashed.focus(unit("a.scope")).await;
        drop(crashed);
        fake.gone.borrow_mut().push("a.scope".into());

        let mut restarted = Booster::new(&fake, 400, state.path());
        restarted.recover().await;

        assert!(!state.exists());
    }

    #[test]
    fn state_round_trips_and_rejects_garbage() {
        let boosted = Boosted {
            unit: "app-x\\x2dy.scope".into(),
            original_weight: u64::MAX,
            boost_weight: 400,
        };
        assert_eq!(Boosted::parse(&boosted.serialize()), Some(boosted));
        assert_eq!(Boosted::parse("only-a-unit\n"), None);
        assert_eq!(Boosted::parse("\n1\n2\n"), None);
        assert_eq!(Boosted::parse("u\nx\n2\n"), None);
    }
}
