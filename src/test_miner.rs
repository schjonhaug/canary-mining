use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, anyhow, bail};
use async_channel::{Receiver, Sender};
use tokio::{net::TcpStream, sync::mpsc, task::JoinHandle};
use tracing::{debug, error, info, warn};

use crate::{
    app_config::AppConfig,
    keys::{AuthorityKeys, authority_key_path},
};
use stratum_apps::{
    network_helpers::noise_connection::Connection,
    stratum_core::{
        bitcoin::{
            CompactTarget, Target, TxMerkleNode,
            block::{Header, Version},
            hash_types::BlockHash,
            hashes::Hash,
        },
        codec_sv2::{HandshakeRole, StandardEitherFrame, StandardSv2Frame},
        common_messages_sv2::{Protocol, SetupConnection},
        mining_sv2::{
            NewMiningJob, OpenStandardMiningChannel, SetNewPrevHash, SubmitSharesStandard,
        },
        noise_sv2::Initiator,
        parsers_sv2::{CommonMessages, Mining, MiningDeviceMessages},
    },
};

pub type MinerMessage = MiningDeviceMessages<'static>;
type StdFrame = StandardSv2Frame<MinerMessage>;
type EitherFrame = StandardEitherFrame<MinerMessage>;

#[derive(Clone, Debug)]
pub struct TestMinerConfig {
    pub pool_address: SocketAddr,
    pub authority_public_key: stratum_apps::key_utils::Secp256k1PublicKey,
    pub user_identity: String,
    pub device_id: String,
    pub nominal_hashrate: f32,
    pub timeout: Duration,
    pub stop_after_accepted_blocks: u32,
    pub linger_after_accepted: Duration,
    pub cores: u32,
    pub share_mode: ShareMode,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum ShareMode {
    BlockOnly,
    PoolTarget,
}

impl TestMinerConfig {
    pub fn from_app_config(
        app_config: &AppConfig,
        pool_override: Option<SocketAddr>,
        authority_public_key: Option<stratum_apps::key_utils::Secp256k1PublicKey>,
    ) -> anyhow::Result<Self> {
        let pool_address = pool_override
            .unwrap_or_else(|| client_address_for_listen(app_config.sv2.listen_address));
        let authority_public_key = match authority_public_key {
            Some(key) => key,
            None => {
                AuthorityKeys::load_or_create(&app_config.data_dir)
                    .with_context(|| {
                        format!(
                            "failed to load authority key from {}",
                            authority_key_path(&app_config.data_dir).display()
                        )
                    })?
                    .public_key
            }
        };

        Ok(Self {
            pool_address,
            authority_public_key,
            user_identity: format!(
                "{}.regtest_sv2_test_miner",
                app_config.default_pool_payout_address()
            ),
            device_id: "canary-mining-test-miner".to_owned(),
            nominal_hashrate: 10_000_000.0,
            timeout: Duration::from_secs(60),
            stop_after_accepted_blocks: 1,
            linger_after_accepted: Duration::ZERO,
            cores: 1,
            share_mode: ShareMode::BlockOnly,
        })
    }
}

#[derive(Clone, Debug)]
struct MiningWork {
    channel_id: u32,
    job_id: u32,
    version: u32,
    prev_hash: [u8; 32],
    merkle_root: [u8; 32],
    nbits: u32,
    ntime: u32,
    target: Target,
}

#[derive(Clone, Debug)]
struct FoundShare {
    channel_id: u32,
    sequence_number: u32,
    job_id: u32,
    nonce: u32,
    ntime: u32,
    version: u32,
}

#[derive(Debug)]
struct MinerState {
    user_identity: String,
    nominal_hashrate: f32,
    share_mode: ShareMode,
    channel_id: Option<u32>,
    sequence_number: u32,
    pool_target: Option<Target>,
    jobs: Vec<NewMiningJob<'static>>,
    prev_hash: Option<SetNewPrevHash<'static>>,
}

impl MinerState {
    fn new(user_identity: String, nominal_hashrate: f32, share_mode: ShareMode) -> Self {
        Self {
            user_identity,
            nominal_hashrate,
            share_mode,
            channel_id: None,
            sequence_number: 0,
            pool_target: None,
            jobs: Vec::new(),
            prev_hash: None,
        }
    }

    fn next_sequence_number(&mut self) -> u32 {
        self.sequence_number = self.sequence_number.wrapping_add(1);
        self.sequence_number
    }

    fn open_channel(&self) -> OpenStandardMiningChannel<'static> {
        OpenStandardMiningChannel {
            request_id: 1.into(),
            user_identity: self.user_identity.clone().into_bytes().try_into().unwrap(),
            nominal_hash_rate: self.nominal_hashrate,
            max_target: vec![0xff; 32].try_into().unwrap(),
        }
    }

    fn update_job(&mut self, job: NewMiningJob<'_>) -> anyhow::Result<Option<MiningWork>> {
        let job = job.as_static();
        if job.is_future() {
            self.jobs.retain(|existing| existing.job_id != job.job_id);
            self.jobs.push(job);
            return Ok(None);
        }

        self.jobs.retain(|existing| existing.job_id != job.job_id);
        self.jobs.push(job);
        self.active_work()
    }

    fn update_prev_hash(
        &mut self,
        prev_hash: SetNewPrevHash<'_>,
    ) -> anyhow::Result<Option<MiningWork>> {
        self.prev_hash = Some(prev_hash.as_static());
        self.active_work()
    }

    fn update_pool_target(&mut self, target: &[u8]) -> anyhow::Result<Option<MiningWork>> {
        let target: [u8; 32] = target.try_into().context("pool target was not 32 bytes")?;
        self.pool_target = Some(Target::from_le_bytes(target));
        self.active_work()
    }

    fn active_work(&self) -> anyhow::Result<Option<MiningWork>> {
        let Some(channel_id) = self.channel_id else {
            return Ok(None);
        };
        let Some(prev_hash) = self.prev_hash.as_ref() else {
            return Ok(None);
        };
        let Some(pool_target) = self.pool_target else {
            return Ok(None);
        };
        let Some(job) = self.jobs.iter().find(|job| job.job_id == prev_hash.job_id) else {
            return Ok(None);
        };

        let block_target = Target::from(CompactTarget::from_consensus(prev_hash.nbits));
        let target = match self.share_mode {
            ShareMode::BlockOnly => block_target,
            ShareMode::PoolTarget => pool_target,
        };

        Ok(Some(MiningWork {
            channel_id,
            job_id: job.job_id,
            version: job.version,
            prev_hash: prev_hash
                .prev_hash
                .to_vec()
                .try_into()
                .map_err(|_| anyhow!("prev_hash was not 32 bytes"))?,
            merkle_root: job
                .merkle_root
                .to_vec()
                .try_into()
                .map_err(|_| anyhow!("merkle_root was not 32 bytes"))?,
            nbits: prev_hash.nbits,
            ntime: current_ntime().max(prev_hash.min_ntime),
            target,
        }))
    }
}

pub async fn run_test_miner(config: TestMinerConfig) -> anyhow::Result<u32> {
    info!(
        "Connecting native SV2 test miner to {}",
        config.pool_address
    );
    let socket = tokio::time::timeout(config.timeout, TcpStream::connect(config.pool_address))
        .await
        .context("timed out connecting to SV2 pool")?
        .with_context(|| format!("failed to connect to {}", config.pool_address))?;

    let initiator = Initiator::new(Some(config.authority_public_key.0));
    let (mut receiver, mut sender) = Connection::new(socket, HandshakeRole::Initiator(initiator))
        .await
        .map_err(|error| anyhow!("failed to establish SV2 Noise connection: {error:?}"))?;
    info!("SV2 Noise connection established");

    setup_connection(
        &mut receiver,
        &mut sender,
        config.pool_address,
        config.device_id.clone(),
    )
    .await?;

    let mut state = MinerState::new(
        config.user_identity,
        config.nominal_hashrate,
        config.share_mode,
    );
    send_mining_message(
        &sender,
        Mining::OpenStandardMiningChannel(state.open_channel()),
    )
    .await?;
    info!("OpenStandardMiningChannel sent");

    let (share_tx, mut share_rx) = mpsc::unbounded_channel::<FoundShare>();
    let mut current_workers: Option<MiningWorkers> = None;
    let mut accepted_blocks = 0;
    let mut timeout = Box::pin(tokio::time::sleep(config.timeout));

    loop {
        tokio::select! {
            _ = &mut timeout => {
                stop_workers(current_workers.take()).await;
                bail!("native SV2 miner timed out after {:?}", config.timeout);
            }
            signal = tokio::signal::ctrl_c() => {
                stop_workers(current_workers.take()).await;
                signal.context("failed to listen for Ctrl-C")?;
                bail!("native SV2 miner interrupted");
            }
            Some(mut found) = share_rx.recv() => {
                found.sequence_number = state.next_sequence_number();
                send_share(&sender, found).await?;
            }
            frame = receiver.recv() => {
                let frame = frame.context("SV2 connection closed by pool")?;
                match handle_frame(&mut state, frame).await? {
                    FrameOutcome::NoWork => {}
                    FrameOutcome::NewWork(work) => {
                        stop_workers(current_workers.take()).await;
                        current_workers = Some(spawn_mining_workers(work, config.cores, share_tx.clone()));
                    }
                    FrameOutcome::AcceptedShare => {
                        accepted_blocks += 1;
                        info!("Accepted native SV2 block candidate {accepted_blocks}/{}", config.stop_after_accepted_blocks);
                        if accepted_blocks >= config.stop_after_accepted_blocks {
                            stop_workers(current_workers.take()).await;
                            if !config.linger_after_accepted.is_zero() {
                                info!(
                                    "Lingering for {:?} after accepted block candidate",
                                    config.linger_after_accepted
                                );
                                tokio::time::sleep(config.linger_after_accepted).await;
                            }
                            drop(sender);
                            drop(receiver);
                            return Ok(accepted_blocks);
                        }
                    }
                }
            }
        }
    }
}

async fn setup_connection(
    receiver: &mut Receiver<EitherFrame>,
    sender: &mut Sender<EitherFrame>,
    address: SocketAddr,
    device_id: String,
) -> anyhow::Result<()> {
    let setup_connection = SetupConnection {
        protocol: Protocol::MiningProtocol,
        min_version: 2,
        max_version: 2,
        flags: 0b1,
        endpoint_host: address.ip().to_string().into_bytes().try_into().unwrap(),
        endpoint_port: address.port(),
        vendor: b"canary".to_vec().try_into().unwrap(),
        hardware_version: b"test-miner".to_vec().try_into().unwrap(),
        firmware: b"dev".to_vec().try_into().unwrap(),
        device_id: device_id.into_bytes().try_into().unwrap(),
    };

    let frame: StdFrame = MiningDeviceMessages::Common(setup_connection.into())
        .try_into()
        .map_err(|error| anyhow!("failed to encode SetupConnection: {error:?}"))?;
    sender
        .send(frame.into())
        .await
        .map_err(|_| anyhow!("failed to send SetupConnection"))?;

    let mut incoming: StdFrame = receiver
        .recv()
        .await
        .context("SV2 connection closed before SetupConnection response")?
        .try_into()
        .map_err(|error| anyhow!("failed to convert SetupConnection response frame: {error:?}"))?;
    let message_type = incoming
        .get_header()
        .context("SetupConnection response missing header")?
        .msg_type();
    let payload = incoming.payload();
    match CommonMessages::try_from((message_type, payload))
        .map_err(|error| anyhow!("failed to parse SetupConnection response: {error:?}"))?
    {
        CommonMessages::SetupConnectionSuccess(success) => {
            info!(
                "SetupConnectionSuccess received: version={}, flags={:b}",
                success.used_version, success.flags
            );
            Ok(())
        }
        CommonMessages::SetupConnectionError(error) => {
            bail!(
                "SetupConnectionError: {}",
                String::from_utf8_lossy(error.error_code.as_ref())
            )
        }
        other => bail!("unexpected setup response: {other:?}"),
    }
}

enum FrameOutcome {
    NoWork,
    NewWork(MiningWork),
    AcceptedShare,
}

async fn handle_frame(state: &mut MinerState, frame: EitherFrame) -> anyhow::Result<FrameOutcome> {
    let mut frame: StdFrame = frame
        .try_into()
        .map_err(|error| anyhow!("failed to convert SV2 frame: {error:?}"))?;
    let message_type = frame
        .get_header()
        .context("SV2 frame missing header")?
        .msg_type();
    let payload = frame.payload();
    let message = Mining::try_from((message_type, payload))
        .map_err(|error| anyhow!("failed to parse mining message: {error:?}"))?;

    match message {
        Mining::OpenStandardMiningChannelSuccess(success) => {
            state.channel_id = Some(success.channel_id);
            let work = state.update_pool_target(success.target.as_ref())?;
            info!(
                "OpenStandardMiningChannelSuccess received: channel_id={}, group_channel_id={}",
                success.channel_id, success.group_channel_id
            );
            Ok(work
                .map(FrameOutcome::NewWork)
                .unwrap_or(FrameOutcome::NoWork))
        }
        Mining::SetTarget(target) => {
            info!("SetTarget received for channel_id={}", target.channel_id);
            let work = state.update_pool_target(target.maximum_target.as_ref())?;
            Ok(work
                .map(FrameOutcome::NewWork)
                .unwrap_or(FrameOutcome::NoWork))
        }
        Mining::NewMiningJob(job) => {
            info!(
                "NewMiningJob received: channel_id={}, job_id={}, future={}",
                job.channel_id,
                job.job_id,
                job.is_future()
            );
            let work = state.update_job(job)?;
            Ok(work
                .map(FrameOutcome::NewWork)
                .unwrap_or(FrameOutcome::NoWork))
        }
        Mining::SetNewPrevHash(prev_hash) => {
            info!(
                "SetNewPrevHash received: channel_id={}, job_id={}, nbits=0x{:08x}",
                prev_hash.channel_id, prev_hash.job_id, prev_hash.nbits
            );
            let work = state.update_prev_hash(prev_hash)?;
            Ok(work
                .map(FrameOutcome::NewWork)
                .unwrap_or(FrameOutcome::NoWork))
        }
        Mining::SubmitSharesSuccess(success) => {
            info!(
                "SubmitSharesSuccess received: channel_id={}, last_sequence_number={}",
                success.channel_id, success.last_sequence_number
            );
            Ok(FrameOutcome::AcceptedShare)
        }
        Mining::SubmitSharesError(error) => {
            warn!(
                "SubmitSharesError received: channel_id={}, sequence_number={}, error_code={}",
                error.channel_id,
                error.sequence_number,
                String::from_utf8_lossy(error.error_code.as_ref())
            );
            Ok(FrameOutcome::NoWork)
        }
        other => {
            debug!("Ignoring mining message: {other:?}");
            Ok(FrameOutcome::NoWork)
        }
    }
}

async fn send_share(sender: &Sender<EitherFrame>, found: FoundShare) -> anyhow::Result<()> {
    let share = SubmitSharesStandard {
        channel_id: found.channel_id,
        sequence_number: found.sequence_number,
        job_id: found.job_id,
        nonce: found.nonce,
        ntime: found.ntime,
        version: found.version,
    };
    send_mining_message(sender, Mining::SubmitSharesStandard(share)).await
}

async fn send_mining_message(
    sender: &Sender<EitherFrame>,
    message: Mining<'static>,
) -> anyhow::Result<()> {
    let frame: StdFrame = MiningDeviceMessages::Mining(message)
        .try_into()
        .map_err(|error| anyhow!("failed to encode mining message: {error:?}"))?;
    sender
        .send(frame.into())
        .await
        .map_err(|_| anyhow!("failed to send mining message"))
}

fn spawn_mining_workers(
    work: MiningWork,
    cores: u32,
    share_tx: mpsc::UnboundedSender<FoundShare>,
) -> MiningWorkers {
    let cancel = Arc::new(AtomicBool::new(false));
    let workers = cores.max(1);
    let task_cancel = cancel.clone();
    let task = tokio::task::spawn_blocking(move || {
        let mut handles = Vec::with_capacity(workers as usize);
        for index in 0..workers {
            let mut work = work.clone();
            let share_tx = share_tx.clone();
            let cancel = task_cancel.clone();
            handles.push(std::thread::spawn(move || {
                mine_worker(&mut work, index, workers, cancel, share_tx);
            }));
        }
        for handle in handles {
            if let Err(error) = handle.join() {
                error!("native SV2 miner worker panicked: {error:?}");
            }
        }
    });
    MiningWorkers { cancel, task }
}

struct MiningWorkers {
    cancel: Arc<AtomicBool>,
    task: JoinHandle<()>,
}

async fn stop_workers(workers: Option<MiningWorkers>) {
    let Some(workers) = workers else {
        return;
    };

    workers.cancel.store(true, Ordering::Relaxed);
    match tokio::time::timeout(Duration::from_secs(2), workers.task).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) if error.is_cancelled() => {}
        Ok(Err(error)) => warn!("native SV2 miner worker task failed: {error}"),
        Err(_) => warn!("native SV2 miner worker task did not stop within 2 seconds"),
    }
}

fn mine_worker(
    work: &mut MiningWork,
    worker_index: u32,
    worker_count: u32,
    cancel: Arc<AtomicBool>,
    share_tx: mpsc::UnboundedSender<FoundShare>,
) {
    let mut nonce = worker_index;
    let mut ntime = work.ntime;

    while !cancel.load(Ordering::Relaxed) {
        let header = build_header(work, nonce, ntime);
        let hash = header.block_hash();
        if work.target.is_met_by(hash) {
            if !cancel.swap(true, Ordering::Relaxed) {
                let _ = share_tx.send(FoundShare {
                    channel_id: work.channel_id,
                    sequence_number: 0,
                    job_id: work.job_id,
                    nonce,
                    ntime,
                    version: work.version,
                });
            }
            break;
        }

        let next_nonce = nonce.wrapping_add(worker_count);
        if next_nonce < nonce {
            ntime = ntime.wrapping_add(1);
        }
        nonce = next_nonce;
    }
}

fn build_header(work: &MiningWork, nonce: u32, ntime: u32) -> Header {
    Header {
        version: Version::from_consensus(work.version as i32),
        prev_blockhash: BlockHash::from_raw_hash(Hash::from_byte_array(work.prev_hash)),
        merkle_root: TxMerkleNode::from_byte_array(work.merkle_root),
        time: ntime,
        bits: CompactTarget::from_consensus(work.nbits),
        nonce,
    }
}

fn current_ntime() -> u32 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32
}

pub fn client_address_for_listen(listen: SocketAddr) -> SocketAddr {
    let ip = match listen.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, listen.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_unspecified_listen_address_to_loopback_client_address() {
        let listen: SocketAddr = "0.0.0.0:3333".parse().unwrap();
        assert_eq!(
            client_address_for_listen(listen).to_string(),
            "127.0.0.1:3333"
        );
    }

    #[test]
    fn regtest_nbits_target_accepts_easy_header() {
        let target = Target::from(CompactTarget::from_consensus(0x207fffff));
        assert!(target > Target::ZERO);
    }
}
