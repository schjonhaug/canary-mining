use crate::template_distribution_protocol::BitcoinCoreSv2TDP;

use std::time::{Duration, Instant};
use stratum_core::parsers_sv2::TemplateDistribution;
use tracing::info;
use crate::template_runtime_status::{TemplateStatusSource, should_wait_for_active_miners};

fn remaining_wait_next_delay(
    last_wait_next_request_instant: Option<Instant>,
    min_interval: u8,
    now: Instant,
) -> Option<Duration> {
    let last_wait_next_request_instant = last_wait_next_request_instant?;
    let min_interval = Duration::from_secs(u64::from(min_interval));
    let elapsed = now.saturating_duration_since(last_wait_next_request_instant);

    min_interval
        .checked_sub(elapsed)
        .filter(|delay| !delay.is_zero())
}

impl BitcoinCoreSv2TDP {
    /// Spawns a new task to monitor the IPC templates
    ///
    /// This task is responsible for:
    /// - Creating a dedicated blocking_thread_ipc_client for waitNext requests
    /// - Entering a loop to handle waitNext requests
    /// - Handling the response from the waitNext request
    /// - Updating the current template data
    /// - Sending the NewTemplate message
    pub fn monitor_ipc_templates(&self) {
        let mut self_clone = self.clone();

        let handle = tokio::task::spawn_local(async move {
            tracing::debug!("monitor_ipc_templates() task started");
            // a dedicated thread_ipc_client is used for waitNext requests
            // this is because waitNext requests are blocking, and we don't want to block the main
            // thread where other requests are handled
            //
            // as soon as this task is cancelled, the blocking_thread_ipc_client is dropped,
            // which cleans up the thread on the Bitcoin Core side
            tracing::debug!("Creating dedicated blocking_thread_ipc_client for waitNext requests");
            let blocking_thread_ipc_client = match self_clone.new_thread_ipc_client().await {
                Ok(blocking_thread_ipc_client) => blocking_thread_ipc_client,
                Err(e) => {
                    tracing::error!("Failed to create blocking thread IPC client: {:?}", e);
                    tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                    self_clone.global_cancellation_token.cancel();
                    return;
                }
            };

            let mut template_ipc_client = match self_clone.current_template_ipc_client() {
                Ok(template_ipc_client) => template_ipc_client,
                Err(e) => {
                    tracing::error!("Failed to get current template IPC client: {:?}", e);
                    tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                    self_clone.global_cancellation_token.cancel();
                    return;
                }
            };

            let mut last_wait_next_request_instant = None;

            tracing::debug!("monitor_ipc_templates() entering main loop");
            loop {
                tracing::debug!("monitor_ipc_templates() loop iteration start");

                let miner_activity = self_clone.template_runtime_status.miner_activity();
                let mut resumed_from_idle = false;
                while should_wait_for_active_miners(&miner_activity) {
                    tracing::debug!("Pausing waitNext while no downstream miners are connected");
                    tokio::select! {
                        _ = self_clone.global_cancellation_token.cancelled() => {
                            tracing::warn!("Exiting mempool change monitoring loop");
                            return;
                        }
                        _ = self_clone.template_ipc_client_cancellation_token.cancelled() => {
                            tracing::warn!("Exiting mempool change monitoring loop");
                            return;
                        }
                        _ = miner_activity.wait_for_active_downstream() => {
                            resumed_from_idle = true;
                        }
                    }
                }
                if resumed_from_idle {
                    last_wait_next_request_instant = None;
                }

                if let Some(delay) = remaining_wait_next_delay(
                    last_wait_next_request_instant,
                    self_clone.min_interval,
                    Instant::now(),
                ) {
                    tracing::debug!(
                        "Sleeping for {} milliseconds before next waitNext request",
                        delay.as_millis()
                    );

                    tokio::select! {
                        _ = self_clone.global_cancellation_token.cancelled() => {
                            tracing::warn!("Exiting mempool change monitoring loop");
                            break;
                        }
                        _ = self_clone.template_ipc_client_cancellation_token.cancelled() => {
                            tracing::warn!("Exiting mempool change monitoring loop");
                            break;
                        }
                        _ = tokio::time::sleep(delay) => {
                            continue;
                        }
                    }
                }

                last_wait_next_request_instant = Some(Instant::now());

                // Create a new request for each iteration
                let wait_next_request = match self_clone
                    .new_wait_next_request(&template_ipc_client, blocking_thread_ipc_client.clone())
                    .await
                {
                    Ok(wait_next_request) => wait_next_request,
                    Err(e) => {
                        tracing::error!("Failed to create waitNext request: {:?}", e);
                        tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                        self_clone.global_cancellation_token.cancel();
                        return;
                    }
                };

                tokio::select! {
                    _ = self_clone.global_cancellation_token.cancelled() => {
                        tracing::debug!("Interrupting waitNext request");
                        if let Err(e) = self_clone.interrupt_wait_request(&template_ipc_client).await {
                            tracing::error!("Failed to interrupt waitNext request during shutdown: {:?}", e);
                        }
                        tracing::warn!("Exiting mempool change monitoring loop");
                        break;
                    }
                    _ = self_clone.template_ipc_client_cancellation_token.cancelled() => {
                        tracing::debug!("Interrupting waitNext request");
                        if let Err(e) = self_clone.interrupt_wait_request(&template_ipc_client).await {
                            tracing::error!("Failed to interrupt waitNext request: {:?}", e);
                            tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                            self_clone.global_cancellation_token.cancel();
                            break;
                        }
                        tracing::warn!("Exiting mempool change monitoring loop");
                        break;
                    }
                    wait_next_request_response = wait_next_request.send().promise => {
                        match wait_next_request_response {
                            Ok(response) => {
                                let result = match response.get() {
                                    Ok(result) => result,
                                    Err(e) => {
                                        tracing::error!("Failed to get response: {}", e);
                                        tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                        self_clone.global_cancellation_token.cancel();
                                        break;
                                    }
                                };

                                let new_template_ipc_client = match result.get_result() {
                                    Ok(new_template_ipc_client) => {
                                        tracing::debug!("waitNext returned new template IPC client");
                                        new_template_ipc_client
                                    },
                                    Err(e) => {
                                        match e.kind {
                                            capnp::ErrorKind::MessageContainsNullCapabilityPointer => {
                                                tracing::debug!("waitNext timed out (no mempool changes)");
                                                tracing::debug!("Continuing to next throttled waitNext iteration");
                                                continue; // Go back to the start of the loop
                                            }
                                            _ => {
                                                tracing::error!("Failed to get new template IPC client: {}", e);
                                                tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                                self_clone.global_cancellation_token.cancel();
                                                break;
                                            }
                                        }
                                    }
                                };

                                tracing::debug!("Fetching new template data...");
                                let new_template_data = match self_clone.fetch_template_data(
                                    new_template_ipc_client.clone(),
                                    blocking_thread_ipc_client.clone(),
                                ).await {
                                    Ok(new_template_data) => new_template_data,
                                    Err(e) => {
                                        tracing::error!("Failed to fetch template data: {:?}", e);
                                        tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                        self_clone.global_cancellation_token.cancel();
                                        break;
                                    }
                                };

                                let new_prev_hash = new_template_data.get_prev_hash();
                                let current_prev_hash = match self_clone.current_prev_hash.borrow().clone() {
                                    Some(prev_hash) => prev_hash,
                                    None => {
                                        tracing::error!("current_prev_hash is not set");
                                        tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                        self_clone.global_cancellation_token.cancel();
                                        break;
                                    }
                                };

                                if new_prev_hash != current_prev_hash {
                                    info!("⛓️ Chain Tip changed! New prev_hash: {}", new_prev_hash);
                                    tracing::debug!("CHAIN TIP CHANGE DETECTED - old: {}, new: {}", current_prev_hash, new_prev_hash);

                                    let stale_template_ids = match self_clone.current_template_ids() {
                                        Ok(stale_template_ids) => stale_template_ids,
                                        Err(e) => {
                                            tracing::error!("Failed to collect stale template ids: {:?}", e);
                                            tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                            self_clone.global_cancellation_token.cancel();
                                            break;
                                        }
                                    };

                                    match self_clone.publish_template(
                                        new_template_data,
                                        TemplateStatusSource::IpcChainTip,
                                        true,
                                        true,
                                        false,
                                    ).await {
                                        Ok(()) => {
                                            self_clone.set_current_template_ipc_client(new_template_ipc_client.clone());
                                            template_ipc_client = new_template_ipc_client;
                                        }
                                        Err(e) => {
                                            tracing::error!("Failed to publish chain-tip template: {:?}", e);
                                            tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                            self_clone.global_cancellation_token.cancel();
                                            break;
                                        }
                                    }

                                    // process the stale template data after 10s
                                    self_clone.process_stale_template_data(stale_template_ids).await;
                                } else {
                                    // check if the minimum interval has been reached
                                    if let Some(last_sent_template_instant) = self_clone.last_sent_template_instant {
                                        let elapsed = last_sent_template_instant.elapsed().as_millis();
                                        let min_interval_millis = self_clone.min_interval as u128 * 1_000;

                                        // if the minimum interval has not been reached, sleep for the remaining time
                                        if elapsed < min_interval_millis {
                                            let sleep_duration = min_interval_millis - elapsed;
                                            // Safe cast: min_interval is u8 (max 255), so sleep_duration is at most 255,000 ms,
                                            // which fits comfortably in u64 (max: 18,446,744,073,709,551,615)
                                            tracing::debug!("Sleeping for {} milliseconds to reach the minimum interval", sleep_duration);
                                            tokio::time::sleep(std::time::Duration::from_millis(sleep_duration as u64)).await;
                                        }
                                    }

                                    info!("💹 Mempool fees increased! Sending NewTemplate message.");
                                    tracing::debug!("MEMPOOL FEE CHANGE DETECTED - sending non-future template");

                                    match self_clone.publish_template(
                                        new_template_data,
                                        TemplateStatusSource::IpcMempool,
                                        false,
                                        false,
                                        true,
                                    ).await {
                                        Ok(()) => {
                                            self_clone.set_current_template_ipc_client(new_template_ipc_client.clone());
                                            template_ipc_client = new_template_ipc_client;
                                        }
                                        Err(e) => {
                                            tracing::error!("Failed to publish fee-update template: {:?}", e);
                                            tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                            self_clone.global_cancellation_token.cancel();
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::debug!("waitNext request failed with error: {}", e);
                                tracing::error!("Failed to get response: {}", e);
                                tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                self_clone.global_cancellation_token.cancel();
                                break;
                            }
                        }
                    }
                }
            }

            tracing::debug!("monitor_ipc_templates() task exiting");
        });

        // Store the handle so we can wait for this task to finish before spawning a new one
        // when handle_coinbase_output_constraints is called
        *self.monitor_ipc_templates_handle.borrow_mut() = Some(handle);
    }

    /// Spawns a new task to monitor the incoming messages
    ///
    /// This task is responsible for:
    /// - Entering a loop to listen for incoming messages
    /// - Routing incoming messages to the appropriate handler
    pub fn monitor_incoming_messages(&self) {
        let mut self_clone = self.clone();

        tokio::task::spawn_local(async move {
            tracing::debug!("monitor_incoming_messages() task started");
            loop {
                tokio::select! {
                    _ = self_clone.global_cancellation_token.cancelled() => {
                        tracing::warn!("Exiting incoming messages loop");
                        tracing::debug!("monitor_incoming_messages() exiting due to cancellation");
                        break;
                    }
                    Ok(incoming_message) = self_clone.incoming_messages.recv() => {
                        tracing::info!("Received: {}", incoming_message);
                        tracing::debug!("monitor_incoming_messages() processing message");

                        match incoming_message {
                            TemplateDistribution::CoinbaseOutputConstraints(coinbase_output_constraints) => {
                                tracing::debug!("Received CoinbaseOutputConstraints - max_additional_size: {}, max_additional_sigops: {}",
                                    coinbase_output_constraints.coinbase_output_max_additional_size,
                                    coinbase_output_constraints.coinbase_output_max_additional_sigops);
                                if let Err(e) = self_clone.handle_coinbase_output_constraints(coinbase_output_constraints).await {
                                    tracing::error!("Failed to handle coinbase output constraints: {:?}", e);
                                    tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                    self_clone.global_cancellation_token.cancel();
                                    break;
                                }
                            }
                            TemplateDistribution::RequestTransactionData(request_transaction_data) => {
                                tracing::debug!("Received RequestTransactionData for template_id: {}", request_transaction_data.template_id);
                                if let Err(e) = self_clone.handle_request_transaction_data(request_transaction_data).await {
                                    tracing::error!("Failed to handle request transaction data: {:?}", e);
                                    tracing::warn!("Terminating Sv2 Bitcoin Core IPC Connection");
                                    self_clone.global_cancellation_token.cancel();
                                    break;
                                }
                            }
                            TemplateDistribution::SubmitSolution(submit_solution) => {
                                tracing::debug!("Received SubmitSolution for template_id: {}", submit_solution.template_id);
                                if let Err(e) = self_clone.handle_submit_solution(submit_solution).await {
                                    tracing::error!("Failed to handle submit solution: {:?}", e);
                                    // no need to activate the global cancellation token here
                                }
                            }
                            _ => {
                                tracing::error!("Received unexpected message: {}", incoming_message);
                                tracing::warn!("Ignoring message");
                                continue;
                            }
                        }
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wait_next_delay_is_none_before_first_request() {
        assert_eq!(remaining_wait_next_delay(None, 60, Instant::now()), None);
    }

    #[test]
    fn wait_next_delay_enforces_min_interval_between_requests() {
        let last = Instant::now();
        let now = last + Duration::from_secs(10);

        assert_eq!(
            remaining_wait_next_delay(Some(last), 60, now),
            Some(Duration::from_secs(50))
        );
    }

    #[test]
    fn wait_next_delay_is_none_after_min_interval_elapsed() {
        let last = Instant::now();
        let now = last + Duration::from_secs(60);

        assert_eq!(remaining_wait_next_delay(Some(last), 60, now), None);
    }
}
