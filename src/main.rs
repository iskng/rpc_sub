mod errors;
mod models;
use anchor_lang::AccountDeserialize;
use crate::errors::IndexerError;
use crate::models::*;
use log::{ debug, error, info, warn };
use anchor_client::solana_client::{
    nonblocking::pubsub_client::PubsubClient,
    rpc_client::RpcClient,
    rpc_config::{ RpcTransactionLogsFilter, RpcTransactionLogsConfig },
    rpc_response::RpcLogsResponse,
};
use anchor_client::solana_sdk;
use anchor_client::solana_client;
use anchor_client::solana_sdk::{
    commitment_config::CommitmentConfig,
    pubkey::Pubkey,
    signature::Signature,
};
use solana_transaction_status::{
    option_serializer::OptionSerializer,
    EncodedConfirmedTransactionWithStatusMeta,
    UiTransactionEncoding,
};
use futures_util::stream::StreamExt;
use std::{ collections::HashSet, str::FromStr, time::Duration };
use tokio::time::sleep;
use std::sync::atomic::{ AtomicUsize, Ordering };
use std::sync::Arc;

const PROGRAM_ID_STR: &str = "9AtCEeZCvLhNU4hypE69CmhiTzMejCz9vdpuRPH1SRjw";

// Instruction discriminators (Updated from IDL)
const CREATE_POST_DISCRIMINATOR: [u8; 8] = [123, 92, 184, 29, 231, 24, 15, 202];
const REPLY_TO_POST_DISCRIMINATOR: [u8; 8] = [47, 124, 83, 114, 55, 170, 176, 188];
const LIKE_POST_DISCRIMINATOR: [u8; 8] = [45, 242, 154, 71, 63, 133, 54, 186];
const UNLIKE_POST_DISCRIMINATOR: [u8; 8] = [236, 63, 6, 34, 128, 3, 114, 174];
const REPOST_POST_DISCRIMINATOR: [u8; 8] = [175, 134, 193, 12, 27, 197, 61, 189];
const UNREPOST_POST_DISCRIMINATOR: [u8; 8] = [222, 76, 33, 179, 167, 222, 5, 21];
const CREATE_OR_UPDATE_PROFILE_DISCRIMINATOR: [u8; 8] = [52, 169, 99, 129, 231, 122, 119, 207];
const FOLLOW_AGENT_DISCRIMINATOR: [u8; 8] = [64, 202, 196, 131, 84, 241, 248, 38];
const UNFOLLOW_AGENT_DISCRIMINATOR: [u8; 8] = [124, 101, 198, 225, 35, 138, 12, 36];

#[tokio::main]
async fn main() -> Result<(), IndexerError> {
    env_logger::init();
    info!("Starting agent_feed_indexer...");

    // let rpc_url = "https://rpc.testnet.x1.xyz"; // X1 testnet endpoint - may have WebSocket issues
    let rpc_url = "https://rpc.testnet.x1.xyz"; // Solana devnet endpoint for testing
    let ws_url = rpc_url.replace("https", "wss");

    let program_id = Pubkey::from_str(PROGRAM_ID_STR)?;

    info!("Connecting to RPC: {}", rpc_url);
    info!("Connecting to WebSocket: {}", ws_url);
    info!("Subscribing to logs for program: {}", program_id);

    let rpc_client = RpcClient::new_with_commitment(rpc_url.clone(), CommitmentConfig::finalized());

    // Try a simple RPC call to test connection
    match rpc_client.get_slot() {
        Ok(slot) => info!("Successfully connected to RPC. Current slot: {}", slot),
        Err(e) => warn!("RPC connection test failed: {}. Continuing anyway...", e),
    }

    // Non-blocking client for subscriptions
    info!("Connecting to WebSocket at {} ...", ws_url);
    let pubsub_client = PubsubClient::new(&ws_url).await.map_err(|e| {
        error!("Failed to connect to WebSocket: {}", e);
        IndexerError::WebSocketError(format!("Failed to connect to WebSocket: {}", e))
    })?;
    info!("WebSocket connection established.");

    // No separate slot subscription test for simplicity - Solana WS clients have complex ownership

    // Now proceed with the program logs subscription
    info!(
        "Creating logs subscription with filter: {:?}",
        RpcTransactionLogsFilter::Mentions(vec![program_id.to_string()])
    );
    let logs_config = RpcTransactionLogsConfig {
        commitment: Some(CommitmentConfig::finalized()),
    };

    let (mut logs_stream, logs_unsubscribe) = pubsub_client
        .logs_subscribe(
            RpcTransactionLogsFilter::Mentions(vec![program_id.to_string()]),
            logs_config
        ).await
        .map_err(|e| {
            error!("Failed to subscribe to logs: {}", e);
            IndexerError::WebSocketError(format!("Failed to subscribe to logs: {}", e))
        })?;

    info!("Successfully subscribed to logs. Waiting for notifications...");

    // Track transaction stats with atomic counters
    let transactions_seen = Arc::new(AtomicUsize::new(0));
    let transactions_seen_clone = transactions_seen.clone();

    // Spawn a heartbeat task to periodically check if we're receiving transactions
    let heartbeat_handle = tokio::spawn(async move {
        let mut last_check = std::time::Instant::now();

        loop {
            sleep(Duration::from_secs(30)).await;
            let now = std::time::Instant::now();
            let elapsed = now.duration_since(last_check).as_secs();

            let count = transactions_seen_clone.swap(0, Ordering::SeqCst);
            info!("Heartbeat: Indexed {} transactions in the last {} seconds", count, elapsed);

            last_check = now;
        }
    });

    // Add a counter for transactions
    let mut transactions_total = 0;
    let mut processed_signatures = HashSet::new();

    while let Some(response_wrapper) = logs_stream.next().await {
        transactions_total += 1;
        transactions_seen.fetch_add(1, Ordering::SeqCst);

        // Log raw response for debugging
        debug!("Raw WS Response: {:?}", response_wrapper);

        let RpcLogsResponse { signature, err, logs, .. } = response_wrapper.value;

        // Check if we've already processed this signature
        if !processed_signatures.insert(signature.clone()) {
            info!("Skipping already processed transaction: {}", signature);
            continue;
        }

        info!("Transaction #{}: {}", transactions_total, signature);

        if let Some(tx_err) = err {
            warn!("Transaction {} failed: {:?}. Logs: {:?}", signature, tx_err, logs);
            continue;
        }

        debug!("Received logs for signature: {}", signature);
        // log::trace!("Logs: {:?}", logs); // Very verbose

        // Heuristic: if logs contain "Program log: Instruction:", it's likely one of our program's instructions
        // and not just a token transfer or some other CPI from another program mentioning ours.
        /*
        let is_program_interaction = logs.iter().any(|log| {
            log.starts_with(&format!("Program {} invoke", program_id)) ||
                log.contains("Program log: Instruction:") // Anchor instruction logs
        });

        if !is_program_interaction {
            debug!("Skipping signature {} as it doesn't seem to be a direct program interaction.", signature);
            continue;
        }
        */

        info!("Processing transaction: {} ({} logs)", signature, logs.len());

        // Fetch transaction details to get involved accounts
        let signature = Signature::from_str(&signature).unwrap();
        let tx_detail_opt = fetch_transaction_with_retry(&rpc_client, &signature, 20, 1000).await?;
        if let Some(tx_detail) = tx_detail_opt {
            // tx_detail IS EncodedConfirmedTransactionWithStatusMeta here
            // Directly use tx_detail, no if let Some needed
            if let Some(decoded_transaction) = tx_detail.transaction.transaction.decode() {
                // Access account_keys by matching the VersionedMessage enum
                let account_keys_slice: &[Pubkey] = match &decoded_transaction.message {
                    solana_sdk::message::VersionedMessage::Legacy(m) => &m.account_keys,
                    solana_sdk::message::VersionedMessage::V0(m) => &m.account_keys,
                };
                let account_keys: Vec<Pubkey> = account_keys_slice.to_vec();

                let mut unique_accounts_to_fetch = HashSet::new();

                // Instructions are likely fine as VersionedMessage has an instructions() method
                for (idx, inst) in decoded_transaction.message.instructions().iter().enumerate() {
                    let prog_key_idx = inst.program_id_index as usize;
                    if
                        prog_key_idx < account_keys.len() &&
                        account_keys[prog_key_idx] == program_id
                    {
                        info!(
                            "  Instruction #{} targets program {}. Data len: {}",
                            idx,
                            program_id,
                            inst.data.len()
                        );

                        // Instruction discriminator check
                        if inst.data.len() >= 8 {
                            let discriminator = &inst.data[..8];
                            match discriminator {
                                x if x == CREATE_POST_DISCRIMINATOR =>
                                    info!("    Detected: create_post"),
                                x if x == REPLY_TO_POST_DISCRIMINATOR =>
                                    info!("    Detected: reply_to_post"),
                                x if x == LIKE_POST_DISCRIMINATOR =>
                                    info!("    Detected: like_post"),
                                x if x == UNLIKE_POST_DISCRIMINATOR =>
                                    info!("    Detected: unlike_post"),
                                x if x == REPOST_POST_DISCRIMINATOR =>
                                    info!("    Detected: repost_post"),
                                x if x == UNREPOST_POST_DISCRIMINATOR =>
                                    info!("    Detected: unrepost_post"),
                                x if x == CREATE_OR_UPDATE_PROFILE_DISCRIMINATOR =>
                                    info!("    Detected: create_or_update_profile"),
                                x if x == FOLLOW_AGENT_DISCRIMINATOR =>
                                    info!("    Detected: follow_agent"),
                                x if x == UNFOLLOW_AGENT_DISCRIMINATOR =>
                                    info!("    Detected: unfollow_agent"),
                                _ =>
                                    warn!(
                                        "    Unknown instruction discriminator: {:?}",
                                        discriminator
                                    ),
                            }
                            // TODO: Deserialize instruction args based on discriminator for full indexing
                        } else {
                            warn!("    Instruction data too short for discriminator.");
                        }

                        // Collect accounts involved in this instruction
                        for acc_idx in &inst.accounts {
                            if (*acc_idx as usize) < account_keys.len() {
                                unique_accounts_to_fetch.insert(account_keys[*acc_idx as usize]);
                            }
                        }
                    } else {
                        // Log instructions not targeting our program (e.g., CPIs *from* our program)
                        debug!("  Instruction #{} targets different program: {}", idx, if
                            prog_key_idx < account_keys.len()
                        {
                            account_keys[prog_key_idx].to_string()
                        } else {
                            "<Index out of bounds>".to_string()
                        });
                    }
                }

                // Include accounts from loaded addresses (important for ALT)
                if let Some(meta) = tx_detail.transaction.meta.as_ref() {
                    match &meta.loaded_addresses {
                        OptionSerializer::Some(loaded_addresses_val) => {
                            for key_str in loaded_addresses_val.writable
                                .iter()
                                .chain(&loaded_addresses_val.readonly) {
                                match Pubkey::from_str(key_str) {
                                    Ok(pubkey) => {
                                        unique_accounts_to_fetch.insert(pubkey);
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Failed to parse loaded address pubkey string '{}': {}",
                                            key_str,
                                            e
                                        );
                                    }
                                }
                            }
                        }
                        OptionSerializer::None | OptionSerializer::Skip => {
                            // Do nothing if loaded_addresses is None or Skip
                        }
                    }

                    // The loop for meta.account_keys still needs to be addressed.
                    // for key in &meta.account_keys { unique_accounts_to_fetch.insert(*key); }
                }

                info!(
                    "Identified {} unique accounts to potentially fetch for tx {}",
                    unique_accounts_to_fetch.len(),
                    signature
                );

                for account_pubkey in unique_accounts_to_fetch {
                    debug!("Attempting to fetch account: {}", account_pubkey);
                    match
                        rpc_client.get_account_with_commitment(
                            &account_pubkey,
                            CommitmentConfig::finalized()
                        )
                    {
                        Ok(response) => {
                            if let Some(account) = response.value {
                                debug!(
                                    "Processing fetched account {} for deserialization. Owner: {}, Data len: {}. Is program owned: {}",
                                    account_pubkey,
                                    account.owner,
                                    account.data.len(),
                                    account.owner == program_id
                                );
                                // Check owner *before* trying to deserialize
                                if account.owner != program_id {
                                    debug!(
                                        "Account {} is not owned by program {}. Owner: {}",
                                        account_pubkey,
                                        program_id,
                                        account.owner
                                    );
                                    continue; // Skip accounts not owned by our program
                                }

                                if account.data.is_empty() {
                                    debug!(
                                        "Account {} owned by program {} but has no data.",
                                        account_pubkey,
                                        program_id
                                    );
                                    continue;
                                }

                                // Now deserialize
                                info!(
                                    "Account {} owned by program {}. Deserializing...",
                                    account_pubkey,
                                    program_id
                                );
                                deserialize_and_print_account_data(&account_pubkey, &account.data);
                            } else {
                                // Account not found, could have been closed. This is often expected.
                                debug!("Account {} not found (likely closed).", account_pubkey);
                            }
                        }
                        Err(e) => {
                            // Log other RPC errors more seriously
                            if
                                let solana_client::client_error::ClientErrorKind::RpcError(
                                    solana_client::rpc_request::RpcError::ForUser(s),
                                ) = &e.kind
                            {
                                if s.contains("AccountNotFound") || s.contains("was not found") {
                                    debug!(
                                        "Account {} not found (likely closed): {}",
                                        account_pubkey,
                                        s
                                    );
                                    continue;
                                }
                            }
                            // Log other errors as warnings
                            warn!(
                                "Failed to fetch account info/owner for {}: {}",
                                account_pubkey,
                                e
                            );
                        }
                    }
                }
            } else {
                warn!("Failed to decode transaction: {}", signature);
            }
        } else {
            warn!("Transaction {} not found after retries", signature);
        }
        tokio::time::sleep(Duration::from_millis(100)).await; // Small delay
    }

    // Clean up heartbeat task
    heartbeat_handle.abort();
    logs_unsubscribe().await;
    info!("Indexer stopped. Processed {} transactions total.", transactions_total);
    Ok(())
}

async fn fetch_transaction_with_retry(
    rpc_client: &RpcClient,
    signature: &Signature,
    max_retries: usize,
    delay_ms: u64
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, IndexerError> {
    for attempt in 0..max_retries {
        match rpc_client.get_transaction(signature, UiTransactionEncoding::Base64) {
            Ok(tx_detail) => {
                return Ok(Some(tx_detail));
            }
            Err(e) => {
                let error_str = e.to_string();
                // Check if it's a "Transaction not found" error (indicated by null response)
                if error_str.contains("invalid type: null") || error_str.contains("not found") {
                    if attempt + 1 == max_retries {
                        return Ok(None); // Transaction not found after all retries
                    }
                    debug!(
                        "Transaction {} not found on attempt {}/{}, retrying after {}ms",
                        signature,
                        attempt + 1,
                        max_retries,
                        delay_ms
                    );
                } else {
                    // For other errors, retry a few times but eventually fail
                    if attempt + 1 == max_retries {
                        return Err(
                            IndexerError::Other(
                                format!("RPC error after {} retries: {}", max_retries, e)
                            )
                        );
                    }
                    warn!(
                        "RPC error on attempt {}/{}: {}, retrying after {}ms",
                        attempt + 1,
                        max_retries,
                        e,
                        delay_ms
                    );
                }
                sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    }
    Ok(None)
}

// Simplified function: Relies on try_deserialize's internal discriminator check.
fn deserialize_and_print_account_data(account_pubkey: &Pubkey, data: &[u8]) {
    debug!(
        "Enter deserialize_and_print_account_data for {}. Data len: {}. Data prefix (first 8 bytes): {:?}",
        account_pubkey,
        data.len(),
        &data[..std::cmp::min(8, data.len())]
    );
    if data.is_empty() {
        info!("Account {} data is empty, cannot deserialize.", account_pubkey);
        return;
    }

    // Try deserializing into each known type.
    // The `try_deserialize` methods in models.rs handle the discriminator check.
    if let Ok(post) = Post::try_deserialize(&mut &*data) {
        info!("Deserialized Post ({}): {:?}", account_pubkey, post);
        return;
    }

    if let Ok(profile) = AgentProfile::try_deserialize(&mut &*data) {
        info!("Deserialized AgentProfile ({}): {:?}", account_pubkey, profile);
        return;
    }

    if let Ok(like_record) = LikeRecord::try_deserialize(&mut &*data) {
        info!("Deserialized LikeRecord ({}): {:?}", account_pubkey, like_record);
        return;
    }

    if let Ok(repost_record) = RepostRecord::try_deserialize(&mut &*data) {
        info!("Deserialized RepostRecord ({}): {:?}", account_pubkey, repost_record);
        return;
    }

    if let Ok(follow_record) = FollowRecord::try_deserialize(&mut &*data) {
        info!("Deserialized FollowRecord ({}): {:?}", account_pubkey, follow_record);
        return;
    }

    // If none matched
    warn!(
        "Could not deserialize account data for {} (len {}). Data prefix (8 bytes): {:?}. This may indicate an unknown account type for this program, data corruption, or an account associated with a CPI from/to another program.",
        account_pubkey,
        data.len(),
        &data[..std::cmp::min(8, data.len())]
    );
}

// TODO:
// 1. Consider adding database persistence for the deserialized data.
// 2. Explore ways to handle chain reorgs or missed events if necessary for production use.
