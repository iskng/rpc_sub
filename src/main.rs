mod errors;
mod models;
mod constants;
mod signal_handler;
mod subscription;
mod processing;

use crate::errors::IndexerError;
use crate::constants::*;
use crate::signal_handler::setup_signal_handler;
use crate::subscription::start_log_subscription_task;
use crate::processing::start_processing_loop;
use log::{ error, info, warn };
use anchor_client::solana_client::{
    nonblocking::rpc_client::RpcClient as NonBlockingRpcClient,
    rpc_response::RpcLogsResponse,
};

use anchor_client::solana_sdk::{ commitment_config::CommitmentConfig, pubkey::Pubkey };

use std::{ str::FromStr, time::Duration };
use tokio::time::sleep;
use std::sync::atomic::{ AtomicUsize, Ordering, AtomicBool };
use std::sync::Arc;
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    env_logger::init();
    info!("Starting agent_feed_indexer...");

    let rpc_url = std::env::var("RPC_URL").unwrap_or_else(|_| {
        warn!("RPC_URL environment variable not set, using default: {}", DEFAULT_RPC_URL);
        DEFAULT_RPC_URL.to_string()
    });
    let program_id_str = std::env::var("PROGRAM_ID_STR").unwrap_or_else(|_| {
        warn!("PROGRAM_ID_STR environment variable not set, using default: {}", DEFAULT_PROGRAM_ID_STR);
        DEFAULT_PROGRAM_ID_STR.to_string()
    });

    let ws_url = rpc_url.replace("https", "wss");
    let program_id = Pubkey::from_str(&program_id_str)?;

    info!("Using RPC URL: {}", rpc_url);
    info!("Using Program ID: {}", program_id_str);
    info!("Subscribing to logs for program: {}", program_id);

    let rpc_client = Arc::new(
        NonBlockingRpcClient::new_with_commitment(rpc_url.clone(), CommitmentConfig::finalized())
    );

    match rpc_client.get_slot().await {
        Ok(slot) => info!("Successfully connected to RPC. Current slot: {}", slot),
        Err(e) => {
            error!("Initial RPC connection test failed: {}. Exiting.", e);
            return Err(IndexerError::RpcError(e.to_string()));
        }
    }

    let shutdown_signal = Arc::new(AtomicBool::new(false));
    setup_signal_handler(shutdown_signal.clone());

    let (tx_log_processor, rx_log_processor) = mpsc::channel::<RpcLogsResponse>(1000);

    // Start log subscription task
    start_log_subscription_task(
        ws_url.clone(),
        program_id.clone(),
        shutdown_signal.clone(),
        tx_log_processor
    ).await;

    // Track transaction stats with atomic counters for heartbeat
    let transactions_seen_for_heartbeat = Arc::new(AtomicUsize::new(0));
    let heartbeat_transactions_seen_clone = transactions_seen_for_heartbeat.clone();
    let heartbeat_shutdown_signal_clone = shutdown_signal.clone();

    let heartbeat_handle = tokio::spawn(async move {
        let mut last_check = std::time::Instant::now();
        loop {
            if heartbeat_shutdown_signal_clone.load(Ordering::SeqCst) {
                info!("Heartbeat task shutting down.");
                break;
            }
            sleep(Duration::from_secs(30)).await;
            if heartbeat_shutdown_signal_clone.load(Ordering::SeqCst) {
                // Check again after sleep, in case shutdown was signaled during sleep
                info!("Heartbeat task shutting down after sleep.");
                break;
            }
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(last_check).as_secs();
            let count = heartbeat_transactions_seen_clone.swap(0, Ordering::SeqCst);
            info!("Heartbeat: Indexed {} transactions in the last {} seconds", count, elapsed);
            last_check = now;
        }
    });

    // Start main processing loop
    let transactions_total = start_processing_loop(
        rpc_client.clone(),
        program_id.clone(),
        rx_log_processor,
        shutdown_signal.clone(),
        transactions_seen_for_heartbeat.clone() // Pass the counter for heartbeat
    ).await;

    info!("Waiting for heartbeat task to shut down...");
    if !heartbeat_handle.is_finished() {
        heartbeat_handle.await.unwrap_or_else(|e| {
            error!("Heartbeat task panicked or was cancelled: {:?}", e);
            // Consider if this should propagate as an error from main
        });
    } else {
        info!("Heartbeat task already finished.");
    }

    info!("Indexer stopped. Processed {} transactions total.", transactions_total);
    Ok(())
}

// TODO:
// 1. Consider adding database persistence for the deserialized data. (HIGHLY RECOMMENDED for durability)
// 2. Explore ways to handle chain reorgs or missed events if necessary for production use.
// 3. The current `spawn_blocking` with a new runtime per transaction is not optimal for high throughput.
//    Consider using a shared `tokio::runtime::Handle` or restructuring the async flow for RPC calls
//    to keep them on the main tokio executor, possibly with a `Semaphore` to limit concurrency.
// 4. Persist `processed_signatures` to disk to prevent reprocessing after restarts.
