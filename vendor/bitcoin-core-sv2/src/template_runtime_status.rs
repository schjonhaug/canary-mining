use std::{
    sync::{
        Arc, RwLock,
        atomic::{AtomicUsize, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use tokio::sync::Notify;

#[derive(Clone, Debug, Default)]
pub struct TemplateRuntimeStatus {
    inner: Arc<TemplateRuntimeStatusInner>,
}

#[derive(Debug, Default)]
struct TemplateRuntimeStatusInner {
    template: RwLock<Option<TemplateStatusSnapshot>>,
    miner_activity: MinerActivity,
}

#[derive(Clone, Debug, Default)]
pub struct MinerActivity {
    active_downstreams: Arc<AtomicUsize>,
    notify: Arc<Notify>,
}

#[derive(Clone, Debug)]
pub struct TemplateStatusSnapshot {
    pub height: u64,
    pub reward_sats: Option<u64>,
    pub fee_sats: Option<u64>,
    pub weight: Option<u64>,
    pub weight_percent: Option<f64>,
    pub transaction_count: usize,
    pub updated_at: u64,
    pub source: TemplateStatusSource,
    pub status: TemplateStatusKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemplateStatusSource {
    IpcBootstrap,
    IpcChainTip,
    IpcMempool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TemplateStatusKind {
    Available,
}

impl TemplateRuntimeStatus {
    pub fn snapshot(&self) -> Option<TemplateStatusSnapshot> {
        self.inner
            .template
            .read()
            .expect("template runtime status lock poisoned")
            .clone()
    }

    pub fn set_template(&self, mut template: TemplateStatusSnapshot) {
        template.updated_at = unix_now();
        *self
            .inner
            .template
            .write()
            .expect("template runtime status lock poisoned") = Some(template);
    }

    pub fn miner_activity(&self) -> MinerActivity {
        self.inner.miner_activity.clone()
    }
}

impl MinerActivity {
    pub fn active_downstream_count(&self) -> usize {
        self.active_downstreams.load(Ordering::SeqCst)
    }

    pub fn has_active_downstreams(&self) -> bool {
        self.active_downstream_count() > 0
    }

    pub fn increment_active_downstreams(&self) -> usize {
        let previous = self.active_downstreams.fetch_add(1, Ordering::SeqCst);
        if previous == 0 {
            self.notify.notify_waiters();
        }
        previous + 1
    }

    pub fn decrement_active_downstreams(&self) -> usize {
        let mut current = self.active_downstreams.load(Ordering::SeqCst);
        loop {
            if current == 0 {
                return 0;
            }

            match self.active_downstreams.compare_exchange(
                current,
                current - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return current - 1,
                Err(actual) => current = actual,
            }
        }
    }

    pub async fn wait_for_active_downstream(&self) {
        while !self.has_active_downstreams() {
            self.notify.notified().await;
        }
    }
}

pub fn should_wait_for_active_miners(activity: &MinerActivity) -> bool {
    !activity.has_active_downstreams()
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn miner_activity_is_inactive_by_default() {
        let activity = MinerActivity::default();

        assert_eq!(activity.active_downstream_count(), 0);
        assert!(!activity.has_active_downstreams());
    }

    #[test]
    fn miner_activity_is_active_after_increment() {
        let activity = MinerActivity::default();

        assert_eq!(activity.increment_active_downstreams(), 1);
        assert_eq!(activity.active_downstream_count(), 1);
        assert!(activity.has_active_downstreams());
    }

    #[test]
    fn miner_activity_is_inactive_after_final_decrement() {
        let activity = MinerActivity::default();

        activity.increment_active_downstreams();
        assert_eq!(activity.decrement_active_downstreams(), 0);
        assert_eq!(activity.active_downstream_count(), 0);
        assert!(!activity.has_active_downstreams());
    }

    #[test]
    fn wait_next_idle_gate_waits_without_active_miners() {
        let activity = MinerActivity::default();

        assert!(should_wait_for_active_miners(&activity));
        activity.increment_active_downstreams();
        assert!(!should_wait_for_active_miners(&activity));
    }
}
