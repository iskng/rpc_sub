use anchor_client::solana_client::rpc_response::{ Response, RpcLogsResponse };
use futures_util::stream::{ BoxStream, StreamExt };
use log::info;
use std::pin::Pin;

// Define a struct to hold all parts of an active subscription
pub struct ActiveSubscription {
    pub stream: BoxStream<'static, Response<RpcLogsResponse>>,
    pub unsubscribe: Box<
        dyn (FnOnce() -> Pin<Box<dyn futures_util::Future<Output = ()> + Send>>) + Send + 'static
    >,
}

impl ActiveSubscription {
    // Method to call unsubscribe, consuming the unsubscribe function part
    pub async fn close(self) {
        info!("Closing active subscription and unsubscribing...");
        let ActiveSubscription { unsubscribe, .. } = self;
        unsubscribe().await;
        info!("Active subscription closed.");
    }
}

pub async fn start_log_subscription_task(
    ws_url: String,
    program_id: anchor_client::solana_sdk::pubkey::Pubkey,
    shutdown_signal: std::sync::Arc<std::sync::atomic::AtomicBool>,
    tx_log_processor: tokio::sync::mpsc::Sender<RpcLogsResponse>
) {
    tokio::spawn(async move {
        let mut active_subscription_opt: Option<ActiveSubscription> = None;
        let mut reconnect_attempts = 0;
        const MAX_RECONNECT_ATTEMPTS: u32 = 5;
        const INITIAL_RECONNECT_DELAY_MS: u64 = 1000;
        let mut current_reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;

        loop {
            if shutdown_signal.load(std::sync::atomic::Ordering::SeqCst) {
                info!("Log subscriber task shutting down.");
                if let Some(active_sub) = active_subscription_opt.take() {
                    log::warn!(
                        "Log subscriber task exiting, closing any remaining active subscription."
                    );
                    active_sub.close().await;
                }
                break;
            }

            if active_subscription_opt.is_none() {
                info!(
                    "Attempting to connect to WebSocket: {} and subscribe to logs for program: {}",
                    ws_url,
                    program_id
                );

                match
                    anchor_client::solana_client::nonblocking::pubsub_client::PubsubClient::new(
                        &ws_url
                    ).await
                {
                    Ok(client) => {
                        let leaked_client: &'static mut anchor_client::solana_client::nonblocking::pubsub_client::PubsubClient = Box::leak(
                            Box::new(client)
                        );
                        let logs_config =
                            anchor_client::solana_client::rpc_config::RpcTransactionLogsConfig {
                                commitment: Some(
                                    anchor_client::solana_sdk::commitment_config::CommitmentConfig::finalized()
                                ),
                            };
                        let filter =
                            anchor_client::solana_client::rpc_config::RpcTransactionLogsFilter::Mentions(
                                vec![program_id.to_string()]
                            );

                        match leaked_client.logs_subscribe(filter, logs_config).await {
                            Ok((stream, unsubscribe)) => {
                                info!("Successfully subscribed to logs!");
                                active_subscription_opt = Some(ActiveSubscription {
                                    stream: Box::pin(stream),
                                    unsubscribe: Box::new(unsubscribe),
                                });
                                reconnect_attempts = 0;
                                current_reconnect_delay_ms = INITIAL_RECONNECT_DELAY_MS;
                            }
                            Err(e) => {
                                log::error!("Failed to subscribe to logs: {}. Retrying...", e);
                            }
                        }
                    }
                    Err(e) => {
                        log::error!("Failed to connect to WebSocket: {}. Retrying...", e);
                    }
                }
            }

            if let Some(mut active_sub) = active_subscription_opt.take() {
                match
                    tokio::time::timeout(
                        std::time::Duration::from_secs(60),
                        active_sub.stream.next()
                    ).await
                {
                    Ok(Some(response_wrapper)) => {
                        log::debug!("Raw WS Response: {:?}", response_wrapper);
                        let signature_for_log = response_wrapper.value.signature.clone();
                        if tx_log_processor.try_send(response_wrapper.value).is_err() {
                            log::warn!(
                                "Log processor channel is full or closed. Dropping log message for signature: {}. Processor issue may require restart.",
                                signature_for_log
                            );
                        }
                        active_subscription_opt = Some(active_sub);
                    }
                    Ok(None) => {
                        log::warn!("Logs stream ended. Closing subscription and reconnecting...");
                        active_sub.close().await;
                    }
                    Err(_timeout_err) => {
                        log::warn!(
                            "Timeout waiting for log message. Assuming stalled connection. Closing subscription and reconnecting..."
                        );
                        active_sub.close().await;
                    }
                }
            } else {
                reconnect_attempts += 1;
                log::warn!(
                    "Connection/Subscription attempt failed or no active subscription. Attempt #{}. Retrying in {} ms...",
                    reconnect_attempts,
                    current_reconnect_delay_ms
                );
                tokio::time::sleep(
                    std::time::Duration::from_millis(current_reconnect_delay_ms)
                ).await;
                if reconnect_attempts >= MAX_RECONNECT_ATTEMPTS {
                    current_reconnect_delay_ms = (current_reconnect_delay_ms * 2).min(60000);
                }
            }
        }
        info!("Log subscriber task finished.");
        if let Some(active_sub) = active_subscription_opt.take() {
            log::warn!("Log subscriber task exiting, closing any remaining active subscription.");
            active_sub.close().await;
        }
    });
}
