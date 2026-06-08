use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
    time::{SystemTime, UNIX_EPOCH},
};

use async_channel::Sender;
use stratum_apps::utils::types::{ChannelId, DownstreamId};

const CLOSED_SESSION_RETENTION_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum MinerChannelKind {
    Standard,
    Extended,
}

impl MinerChannelKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::Extended => "extended",
        }
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct MinerSessionKey {
    downstream_id: DownstreamId,
    channel_id: ChannelId,
    channel_kind: MinerChannelKind,
}

#[derive(Clone, Debug)]
struct MinerSessionStats {
    downstream_id: DownstreamId,
    channel_id: ChannelId,
    channel_kind: MinerChannelKind,
    user_identity: String,
    opened_at: u64,
    closed_at: Option<u64>,
    last_seen_at: u64,
    nominal_hashrate: f64,
    shares_accepted: u64,
    best_diff: f64,
    blocks_found: u64,
    shares_rejected: u64,
    shares_rejected_by_reason: HashMap<String, u64>,
}

#[derive(Clone, Debug, Default)]
pub struct MinerStatsRegistry {
    sessions: Arc<RwLock<HashMap<MinerSessionKey, MinerSessionStats>>>,
    event_sender: Option<Sender<MinerStatsEvent>>,
}

#[derive(Clone, Debug)]
pub struct MinerSessionSnapshot {
    pub downstream_id: DownstreamId,
    pub channel_id: ChannelId,
    pub channel_kind: &'static str,
    pub user_identity: String,
    pub opened_at: u64,
    pub closed_at: Option<u64>,
    pub last_seen_at: u64,
    pub nominal_hashrate: f64,
    pub shares_accepted: u64,
    pub best_diff: f64,
    pub blocks_found: u64,
    pub shares_rejected: u64,
    pub shares_rejected_by_reason: HashMap<String, u64>,
}

#[derive(Clone, Debug)]
pub enum MinerStatsEvent {
    ChannelOpened {
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: &'static str,
        user_identity: String,
        nominal_hashrate: f64,
        opened_at: u64,
    },
    ChannelClosed {
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: &'static str,
        closed_at: u64,
    },
    DownstreamClosed {
        downstream_id: DownstreamId,
        closed_at: u64,
    },
    ShareAccepted {
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: &'static str,
        shares_accepted: u64,
        best_diff: f64,
        blocks_found: u64,
        last_seen_at: u64,
    },
    ShareRejected {
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: &'static str,
        reason: String,
        shares_rejected: u64,
        last_seen_at: u64,
    },
    BlockFound {
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: &'static str,
        share_hash: String,
        blocks_found: u64,
        found_at: u64,
    },
}

impl MinerStatsRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_event_sender(event_sender: Sender<MinerStatsEvent>) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_sender: Some(event_sender),
        }
    }

    pub fn record_channel_opened(
        &self,
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: MinerChannelKind,
        user_identity: String,
        nominal_hashrate: f64,
    ) {
        let now = unix_now();
        let key = MinerSessionKey {
            downstream_id,
            channel_id,
            channel_kind,
        };
        let stats = MinerSessionStats {
            downstream_id,
            channel_id,
            channel_kind,
            user_identity,
            opened_at: now,
            closed_at: None,
            last_seen_at: now,
            nominal_hashrate,
            shares_accepted: 0,
            best_diff: 0.0,
            blocks_found: 0,
            shares_rejected: 0,
            shares_rejected_by_reason: HashMap::new(),
        };

        if let Ok(mut sessions) = self.sessions.write() {
            sessions.insert(key, stats.clone());
        }
        self.emit(MinerStatsEvent::ChannelOpened {
            downstream_id,
            channel_id,
            channel_kind: channel_kind.as_str(),
            user_identity: stats.user_identity,
            nominal_hashrate,
            opened_at: now,
        });
    }

    pub fn record_share_seen(
        &self,
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: MinerChannelKind,
    ) {
        let key = MinerSessionKey {
            downstream_id,
            channel_id,
            channel_kind,
        };
        if let Ok(mut sessions) = self.sessions.write() {
            if let Some(stats) = sessions.get_mut(&key) {
                stats.last_seen_at = unix_now();
            }
        }
    }

    pub fn record_share_rejected(
        &self,
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: MinerChannelKind,
        reason: &str,
    ) {
        let key = MinerSessionKey {
            downstream_id,
            channel_id,
            channel_kind,
        };
        let mut rejected = 0;
        let now = unix_now();
        if let Ok(mut sessions) = self.sessions.write() {
            if let Some(stats) = sessions.get_mut(&key) {
                stats.last_seen_at = now;
                stats.shares_rejected += 1;
                rejected = stats.shares_rejected;
                *stats
                    .shares_rejected_by_reason
                    .entry(reason.to_owned())
                    .or_default() += 1;
            }
        }
        if rejected > 0 {
            self.emit(MinerStatsEvent::ShareRejected {
                downstream_id,
                channel_id,
                channel_kind: channel_kind.as_str(),
                reason: reason.to_owned(),
                shares_rejected: rejected,
                last_seen_at: now,
            });
        }
    }

    pub fn record_share_accepted(
        &self,
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: MinerChannelKind,
        shares_accepted: u64,
        best_diff: f64,
        blocks_found: u64,
    ) {
        let key = MinerSessionKey {
            downstream_id,
            channel_id,
            channel_kind,
        };
        let now = unix_now();
        if let Ok(mut sessions) = self.sessions.write() {
            if let Some(stats) = sessions.get_mut(&key) {
                stats.last_seen_at = now;
                stats.shares_accepted = shares_accepted;
                stats.best_diff = best_diff;
                stats.blocks_found = blocks_found;
            }
        }
        self.emit(MinerStatsEvent::ShareAccepted {
            downstream_id,
            channel_id,
            channel_kind: channel_kind.as_str(),
            shares_accepted,
            best_diff,
            blocks_found,
            last_seen_at: now,
        });
    }

    pub fn record_block_found(
        &self,
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: MinerChannelKind,
        share_hash: String,
        blocks_found: u64,
    ) {
        let now = unix_now();
        self.emit(MinerStatsEvent::BlockFound {
            downstream_id,
            channel_id,
            channel_kind: channel_kind.as_str(),
            share_hash,
            blocks_found,
            found_at: now,
        });
    }

    pub fn record_channel_closed(
        &self,
        downstream_id: DownstreamId,
        channel_id: ChannelId,
        channel_kind: MinerChannelKind,
    ) {
        let key = MinerSessionKey {
            downstream_id,
            channel_id,
            channel_kind,
        };
        let now = unix_now();
        let mut closed = false;
        if let Ok(mut sessions) = self.sessions.write() {
            if let Some(stats) = sessions.get_mut(&key) {
                stats.closed_at = Some(now);
                stats.last_seen_at = now;
                closed = true;
            }
        }
        if closed {
            self.emit(MinerStatsEvent::ChannelClosed {
                downstream_id,
                channel_id,
                channel_kind: channel_kind.as_str(),
                closed_at: now,
            });
        }
    }

    pub fn record_downstream_closed(&self, downstream_id: DownstreamId) {
        let now = unix_now();
        let mut closed = false;
        if let Ok(mut sessions) = self.sessions.write() {
            for stats in sessions.values_mut() {
                if stats.downstream_id == downstream_id && stats.closed_at.is_none() {
                    stats.closed_at = Some(now);
                    stats.last_seen_at = now;
                    closed = true;
                }
            }
        }
        if closed {
            self.emit(MinerStatsEvent::DownstreamClosed {
                downstream_id,
                closed_at: now,
            });
        }
    }

    pub fn snapshots(&self) -> Vec<MinerSessionSnapshot> {
        let now = unix_now();
        let Ok(mut sessions) = self.sessions.write() else {
            return Vec::new();
        };
        sessions.retain(|_, stats| {
            stats.closed_at.is_none_or(|closed_at| {
                now.saturating_sub(closed_at) <= CLOSED_SESSION_RETENTION_SECS
            })
        });

        sessions
            .values()
            .map(|stats| MinerSessionSnapshot {
                downstream_id: stats.downstream_id,
                channel_id: stats.channel_id,
                channel_kind: stats.channel_kind.as_str(),
                user_identity: stats.user_identity.clone(),
                opened_at: stats.opened_at,
                closed_at: stats.closed_at,
                last_seen_at: stats.last_seen_at,
                nominal_hashrate: stats.nominal_hashrate,
                shares_accepted: stats.shares_accepted,
                best_diff: stats.best_diff,
                blocks_found: stats.blocks_found,
                shares_rejected: stats.shares_rejected,
                shares_rejected_by_reason: stats.shares_rejected_by_reason.clone(),
            })
            .collect()
    }

    pub fn forget_closed_sessions_for_user(&self, user_identity: &str) {
        let Ok(mut sessions) = self.sessions.write() else {
            return;
        };
        sessions.retain(|_, stats| stats.user_identity != user_identity || stats.closed_at.is_none());
    }

    fn emit(&self, event: MinerStatsEvent) {
        if let Some(sender) = &self.event_sender {
            let _ = sender.try_send(event);
        }
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_rejections_and_disconnects() {
        let registry = MinerStatsRegistry::new();
        registry.record_channel_opened(1, 2, MinerChannelKind::Standard, "worker".to_owned(), 0.0);
        registry.record_share_rejected(1, 2, MinerChannelKind::Standard, "stale-share");
        registry.record_downstream_closed(1);

        let snapshots = registry.snapshots();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].shares_rejected, 1);
        assert_eq!(
            snapshots[0].shares_rejected_by_reason.get("stale-share"),
            Some(&1)
        );
        assert!(snapshots[0].closed_at.is_some());
    }

    #[test]
    fn prunes_old_closed_sessions() {
        let registry = MinerStatsRegistry::new();
        registry.record_channel_opened(1, 2, MinerChannelKind::Standard, "worker".to_owned(), 0.0);
        registry.record_downstream_closed(1);

        {
            let mut sessions = registry.sessions.write().unwrap();
            for stats in sessions.values_mut() {
                stats.closed_at = Some(unix_now().saturating_sub(CLOSED_SESSION_RETENTION_SECS + 1));
            }
        }

        assert!(registry.snapshots().is_empty());
    }
}
