use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    thread,
};

use anyhow::Context;
use pool_sv2::miner_stats::MinerStatsEvent;
use rusqlite::{Connection, params};
use tracing::warn;

use crate::{
    app_config::{AppConfig, Network},
    miner_identity::parse_miner_identity,
};

const SCHEMA_VERSION: i64 = 1;

#[derive(Clone, Debug)]
pub struct MinerHistory {
    path: PathBuf,
    network: Network,
}

#[derive(Clone, Debug)]
pub struct KnownMiner {
    pub miner_id: String,
    pub user_identity: String,
    pub payout_address: String,
    pub label: String,
    pub connected: bool,
    pub first_seen_at: u64,
    pub last_seen_at: u64,
    pub opened_at: Option<u64>,
    pub closed_at: Option<u64>,
    pub uptime_seconds: Option<u64>,
    pub nominal_hashrate: f64,
    pub shares_accepted: u64,
    pub best_diff: f64,
    pub blocks_found: u64,
    pub shares_rejected: u64,
}

impl MinerHistory {
    pub fn open(config: &AppConfig) -> anyhow::Result<Self> {
        let path = database_path(config);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("failed to create miner database dir {}", parent.display())
            })?;
        }
        let history = Self {
            path,
            network: config.network,
        };
        history.with_connection(|db| {
            initialize(db)?;
            mark_miners_disconnected(db)
        })?;
        Ok(history)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn start_writer(
        &self,
        receiver: async_channel::Receiver<MinerStatsEvent>,
    ) -> thread::JoinHandle<()> {
        let path = self.path.clone();
        let network = self.network;
        thread::spawn(move || {
            let db = match Connection::open(&path) {
                Ok(db) => db,
                Err(error) => {
                    warn!("failed to open miner database writer: {error}");
                    return;
                }
            };
            if let Err(error) = initialize(&db) {
                warn!("failed to initialize miner database writer: {error}");
                return;
            }

            let mut sessions = HashMap::<SessionKey, String>::new();
            while let Ok(event) = receiver.recv_blocking() {
                if let Err(error) = apply_event(&db, network, event, &mut sessions) {
                    warn!("failed to persist miner event: {error}");
                }
            }
        })
    }

    pub async fn miners(&self) -> anyhow::Result<Vec<KnownMiner>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let db = Connection::open(path)?;
            initialize(&db)?;
            query_miners(&db)
        })
        .await
        .context("known miners task panicked")?
    }

    pub async fn delete_miner(&self, miner_id: String) -> anyhow::Result<bool> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let db = Connection::open(path)?;
            initialize(&db)?;
            delete_offline_miner(&db, &miner_id)
        })
        .await
        .context("delete miner task panicked")?
    }

    fn with_connection<T>(
        &self,
        f: impl FnOnce(&Connection) -> anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        let db = Connection::open(&self.path)
            .with_context(|| format!("failed to open {}", self.path.display()))?;
        f(&db)
    }
}

pub fn database_path(config: &AppConfig) -> PathBuf {
    config.data_dir.join("canary-ui.sqlite")
}

pub fn miner_id_from_identity(network: Network, user_identity: &str) -> Option<String> {
    parse_miner_identity(user_identity, network)
        .ok()
        .map(|identity| miner_id(network, &identity.payout_address, &identity.label))
}

pub fn miner_id(network: Network, payout_address: &str, label: &str) -> String {
    format!("{network}:{payout_address}:{}", label.trim())
}

fn initialize(db: &Connection) -> anyhow::Result<()> {
    db.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS known_miners (
            miner_id TEXT PRIMARY KEY,
            user_identity TEXT NOT NULL,
            payout_address TEXT NOT NULL,
            label TEXT NOT NULL,
            connected INTEGER NOT NULL DEFAULT 0,
            first_seen_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            opened_at INTEGER,
            closed_at INTEGER,
            uptime_seconds INTEGER,
            nominal_hashrate REAL NOT NULL DEFAULT 0,
            shares_accepted INTEGER NOT NULL DEFAULT 0,
            best_diff REAL NOT NULL DEFAULT 0,
            blocks_found INTEGER NOT NULL DEFAULT 0,
            shares_rejected INTEGER NOT NULL DEFAULT 0
        );

        CREATE INDEX IF NOT EXISTS idx_known_miners_last_seen
            ON known_miners(last_seen_at DESC);
        "#,
    )?;
    db.pragma_update(None, "user_version", SCHEMA_VERSION)?;
    Ok(())
}

fn mark_miners_disconnected(db: &Connection) -> anyhow::Result<()> {
    db.execute(
        "UPDATE known_miners
         SET connected = 0,
             nominal_hashrate = 0,
             closed_at = COALESCE(closed_at, last_seen_at)",
        [],
    )?;
    Ok(())
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct SessionKey {
    downstream_id: u64,
    channel_id: u32,
    channel_kind: String,
}

fn apply_event(
    db: &Connection,
    network: Network,
    event: MinerStatsEvent,
    sessions: &mut HashMap<SessionKey, String>,
) -> anyhow::Result<()> {
    match event {
        MinerStatsEvent::ChannelOpened {
            downstream_id,
            channel_id,
            channel_kind,
            user_identity,
            nominal_hashrate,
            opened_at,
        } => {
            let Some(miner_id) = miner_id_from_identity(network, &user_identity) else {
                return Ok(());
            };
            sessions.insert(
                SessionKey {
                    downstream_id: downstream_id as u64,
                    channel_id,
                    channel_kind: channel_kind.to_owned(),
                },
                miner_id.clone(),
            );
            upsert_open_miner(
                db,
                network,
                &miner_id,
                &user_identity,
                nominal_hashrate,
                opened_at,
            )
        }
        MinerStatsEvent::ChannelClosed {
            downstream_id,
            channel_id,
            channel_kind,
            closed_at,
        } => {
            let key = SessionKey {
                downstream_id: downstream_id as u64,
                channel_id,
                channel_kind: channel_kind.to_owned(),
            };
            let Some(miner_id) = sessions.remove(&key) else {
                return Ok(());
            };
            close_miner(db, &miner_id, closed_at)
        }
        MinerStatsEvent::DownstreamClosed {
            downstream_id,
            closed_at,
        } => {
            let miner_ids = sessions
                .extract_if(|key, _| key.downstream_id == downstream_id as u64)
                .map(|(_, miner_id)| miner_id)
                .collect::<Vec<_>>();
            for miner_id in miner_ids {
                close_miner(db, &miner_id, closed_at)?;
            }
            Ok(())
        }
        MinerStatsEvent::ShareAccepted {
            downstream_id,
            channel_id,
            channel_kind,
            shares_accepted,
            best_diff,
            blocks_found,
            last_seen_at,
        } => update_known_session(
            sessions,
            downstream_id as u64,
            channel_id,
            channel_kind,
            |miner_id| {
                let shares_accepted = i64::try_from(shares_accepted)
                    .context("shares_accepted exceeds SQLite integer range")?;
                let blocks_found = i64::try_from(blocks_found)
                    .context("blocks_found exceeds SQLite integer range")?;
                db.execute(
                    "UPDATE known_miners
                     SET shares_accepted = ?1, best_diff = ?2, blocks_found = ?3, last_seen_at = ?4
                     WHERE miner_id = ?5",
                    params![
                        shares_accepted,
                        best_diff,
                        blocks_found,
                        last_seen_at as i64,
                        miner_id
                    ],
                )?;
                Ok(())
            },
        ),
        MinerStatsEvent::ShareRejected {
            downstream_id,
            channel_id,
            channel_kind,
            shares_rejected,
            last_seen_at,
            ..
        } => update_known_session(
            sessions,
            downstream_id as u64,
            channel_id,
            channel_kind,
            |miner_id| {
                let shares_rejected = i64::try_from(shares_rejected)
                    .context("shares_rejected exceeds SQLite integer range")?;
                db.execute(
                    "UPDATE known_miners
                     SET shares_rejected = ?1, last_seen_at = ?2
                     WHERE miner_id = ?3",
                    params![shares_rejected, last_seen_at as i64, miner_id],
                )?;
                Ok(())
            },
        ),
        MinerStatsEvent::BlockFound { .. } => Ok(()),
    }
}

fn upsert_open_miner(
    db: &Connection,
    network: Network,
    miner_id: &str,
    user_identity: &str,
    nominal_hashrate: f64,
    opened_at: u64,
) -> anyhow::Result<()> {
    let identity = parse_miner_identity(user_identity, network)?;
    db.execute(
        "INSERT INTO known_miners (
            miner_id, user_identity, payout_address, label, connected, first_seen_at,
            last_seen_at, opened_at, closed_at, uptime_seconds, nominal_hashrate,
            shares_accepted, best_diff, blocks_found, shares_rejected
         ) VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5, ?5, NULL, NULL, ?6, 0, 0, 0, 0)
         ON CONFLICT(miner_id) DO UPDATE SET
            user_identity = excluded.user_identity,
            payout_address = excluded.payout_address,
            label = excluded.label,
            connected = 1,
            last_seen_at = excluded.last_seen_at,
            opened_at = excluded.opened_at,
            closed_at = NULL,
            uptime_seconds = NULL,
            nominal_hashrate = excluded.nominal_hashrate,
            shares_accepted = 0,
            best_diff = 0,
            blocks_found = 0,
            shares_rejected = 0",
        params![
            miner_id,
            user_identity,
            identity.payout_address,
            identity.label,
            opened_at as i64,
            nominal_hashrate
        ],
    )?;
    Ok(())
}

fn close_miner(db: &Connection, miner_id: &str, closed_at: u64) -> anyhow::Result<()> {
    db.execute(
        "UPDATE known_miners
         SET connected = 0,
             last_seen_at = ?1,
             closed_at = ?1,
             uptime_seconds = CASE
                WHEN opened_at IS NULL THEN uptime_seconds
                ELSE MAX(?1 - opened_at, 0)
             END,
             nominal_hashrate = 0
         WHERE miner_id = ?2",
        params![closed_at as i64, miner_id],
    )?;
    Ok(())
}

fn update_known_session(
    sessions: &HashMap<SessionKey, String>,
    downstream_id: u64,
    channel_id: u32,
    channel_kind: &str,
    f: impl FnOnce(&str) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let key = SessionKey {
        downstream_id,
        channel_id,
        channel_kind: channel_kind.to_owned(),
    };
    let Some(miner_id) = sessions.get(&key) else {
        return Ok(());
    };
    f(miner_id)
}

fn delete_offline_miner(db: &Connection, miner_id: &str) -> anyhow::Result<bool> {
    let deleted = db.execute(
        "DELETE FROM known_miners WHERE miner_id = ?1 AND connected = 0",
        params![miner_id],
    )?;
    Ok(deleted > 0)
}

fn query_miners(db: &Connection) -> anyhow::Result<Vec<KnownMiner>> {
    let mut stmt = db.prepare(
        "SELECT miner_id, user_identity, payout_address, label, connected, first_seen_at,
                last_seen_at, opened_at, closed_at, uptime_seconds, nominal_hashrate,
                shares_accepted, best_diff, blocks_found, shares_rejected
         FROM known_miners
         ORDER BY connected DESC, last_seen_at DESC, label ASC, payout_address ASC",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok(KnownMiner {
            miner_id: row.get(0)?,
            user_identity: row.get(1)?,
            payout_address: row.get(2)?,
            label: row.get(3)?,
            connected: row.get::<_, i64>(4)? != 0,
            first_seen_at: row.get::<_, i64>(5)? as u64,
            last_seen_at: row.get::<_, i64>(6)? as u64,
            opened_at: row.get::<_, Option<i64>>(7)?.map(|value| value as u64),
            closed_at: row.get::<_, Option<i64>>(8)?.map(|value| value as u64),
            uptime_seconds: row.get::<_, Option<i64>>(9)?.map(|value| value as u64),
            nominal_hashrate: row.get(10)?,
            shares_accepted: row.get::<_, i64>(11)? as u64,
            best_diff: row.get(12)?,
            blocks_found: row.get::<_, i64>(13)? as u64,
            shares_rejected: row.get::<_, i64>(14)? as u64,
        })
    })?;
    Ok(rows.collect::<Result<Vec<_>, _>>()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const REGTEST_ADDRESS: &str = "bcrt1q2nfxmhd4n3c8834pj72xagvyr9gl57n5r94fsl";

    #[test]
    fn canonical_id_matches_supported_identity_forms() {
        let dotted = format!("{REGTEST_ADDRESS}.garage/s19");
        let sri = format!("sri/solo/{REGTEST_ADDRESS}/garage/s19");

        assert_eq!(
            miner_id_from_identity(Network::Regtest, &dotted),
            miner_id_from_identity(Network::Regtest, &sri)
        );
    }

    #[test]
    fn persists_reconnect_as_single_known_miner() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let mut sessions = HashMap::new();
        let dotted = format!("{REGTEST_ADDRESS}.garage");
        let sri = format!("sri/solo/{REGTEST_ADDRESS}/garage");

        apply_event(
            &db,
            Network::Regtest,
            MinerStatsEvent::ChannelOpened {
                downstream_id: 1,
                channel_id: 7,
                channel_kind: "standard",
                user_identity: dotted,
                nominal_hashrate: 100.0,
                opened_at: 10,
            },
            &mut sessions,
        )
        .unwrap();
        apply_event(
            &db,
            Network::Regtest,
            MinerStatsEvent::ChannelClosed {
                downstream_id: 1,
                channel_id: 7,
                channel_kind: "standard",
                closed_at: 40,
            },
            &mut sessions,
        )
        .unwrap();
        apply_event(
            &db,
            Network::Regtest,
            MinerStatsEvent::ChannelOpened {
                downstream_id: 2,
                channel_id: 9,
                channel_kind: "standard",
                user_identity: sri,
                nominal_hashrate: 200.0,
                opened_at: 100,
            },
            &mut sessions,
        )
        .unwrap();

        let miners = query_miners(&db).unwrap();
        assert_eq!(miners.len(), 1);
        assert!(miners[0].connected);
        assert_eq!(miners[0].opened_at, Some(100));
        assert_eq!(miners[0].uptime_seconds, None);
        assert_eq!(miners[0].nominal_hashrate, 200.0);
    }

    #[test]
    fn close_records_last_session_uptime() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let mut sessions = HashMap::new();
        let user_identity = format!("{REGTEST_ADDRESS}.garage");

        apply_event(
            &db,
            Network::Regtest,
            MinerStatsEvent::ChannelOpened {
                downstream_id: 1,
                channel_id: 7,
                channel_kind: "standard",
                user_identity,
                nominal_hashrate: 100.0,
                opened_at: 10,
            },
            &mut sessions,
        )
        .unwrap();
        apply_event(
            &db,
            Network::Regtest,
            MinerStatsEvent::ChannelClosed {
                downstream_id: 1,
                channel_id: 7,
                channel_kind: "standard",
                closed_at: 40,
            },
            &mut sessions,
        )
        .unwrap();

        let miners = query_miners(&db).unwrap();
        assert!(!miners[0].connected);
        assert_eq!(miners[0].last_seen_at, 40);
        assert_eq!(miners[0].uptime_seconds, Some(30));
    }

    #[test]
    fn delete_removes_offline_miner_only() {
        let db = Connection::open_in_memory().unwrap();
        initialize(&db).unwrap();
        let id = miner_id(Network::Regtest, REGTEST_ADDRESS, "garage");
        upsert_open_miner(
            &db,
            Network::Regtest,
            &id,
            &format!("{REGTEST_ADDRESS}.garage"),
            1.0,
            1,
        )
        .unwrap();

        assert!(!delete_offline_miner(&db, &id).unwrap());
        close_miner(&db, &id, 2).unwrap();
        assert!(delete_offline_miner(&db, &id).unwrap());
        assert!(query_miners(&db).unwrap().is_empty());
    }
}
