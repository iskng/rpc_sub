mod errors;
mod models;

use anchor_lang::AccountDeserialize;
use crate::errors::IndexerError;
use crate::models::*;
use anchor_lang::{ AnchorDeserialize };
use log::{ debug, error, info, warn };
use solana_client::{
    nonblocking::pubsub_client::PubsubClient,
    rpc_client::RpcClient,
    rpc_config::{ RpcTransactionLogsFilter, RpcTransactionLogsConfig },
    rpc_response::RpcLogsResponse,
};
use solana_sdk::{ commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signature };
use solana_transaction_status::{
    option_serializer::OptionSerializer,
    EncodedConfirmedTransactionWithStatusMeta,
    UiTransactionEncoding,
};
use futures_util::stream::StreamExt;
use std::{ collections::HashSet, env, str::FromStr, time::Duration };
use tokio::time::sleep;
use std::sync::atomic::{ AtomicUsize, Ordering };
use std::sync::Arc;

const PROGRAM_ID_STR: &str = "Cw6VFzwbVFV9GHYLTVsK55jujskQAZF63xFFsL8oGFjr";

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

    let rpc_client = RpcClient::new_with_commitment(rpc_url.clone(), CommitmentConfig::confirmed());

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
        commitment: Some(CommitmentConfig::confirmed()),
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

    while let Some(response_wrapper) = logs_stream.next().await {
        transactions_total += 1;
        transactions_seen.fetch_add(1, Ordering::SeqCst);
        let RpcLogsResponse { signature, err, logs, .. } = response_wrapper.value;

        info!("Transaction #{}: {}", transactions_total, signature);

        if let Some(tx_err) = err {
            warn!("Transaction {} failed: {:?}. Logs: {:?}", signature, tx_err, logs);
            continue;
        }

        debug!("Received logs for signature: {}", signature);
        // log::trace!("Logs: {:?}", logs); // Very verbose

        // Heuristic: if logs contain "Program log: Instruction:", it's likely one of our program's instructions
        // and not just a token transfer or some other CPI from another program mentioning ours.
        let is_program_interaction = logs.iter().any(|log| {
            log.starts_with(&format!("Program {} invoke", program_id)) ||
                log.contains("Program log: Instruction:") // Anchor instruction logs
        });

        if !is_program_interaction {
            debug!("Skipping signature {} as it doesn't seem to be a direct program interaction.", signature);
            continue;
        }

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
                for (_idx, inst) in decoded_transaction.message.instructions().iter().enumerate() {
                    let prog_key_idx = inst.program_id_index as usize;
                    if
                        prog_key_idx < account_keys.len() &&
                        account_keys[prog_key_idx] == program_id
                    {
                        for acc_idx in &inst.accounts {
                            if (*acc_idx as usize) < account_keys.len() {
                                unique_accounts_to_fetch.insert(account_keys[*acc_idx as usize]);
                            }
                        }
                    }
                }

                if let Some(meta) = tx_detail.transaction.meta.as_ref() {
                    // meta.loaded_addresses is OptionSerializer<UiLoadedAddresses>
                    // Match on its variants to get the inner UiLoadedAddresses
                    match &meta.loaded_addresses {
                        // Match on a reference
                        OptionSerializer::Some(loaded_addresses_val) => {
                            // loaded_addresses_val is &UiLoadedAddresses here
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
                    match rpc_client.get_account_data(&account_pubkey) {
                        Ok(data) => {
                            if data.is_empty() {
                                debug!("Account {} has no data or was closed.", account_pubkey);
                                continue;
                            }
                            match
                                rpc_client.get_account_with_commitment(
                                    &account_pubkey,
                                    CommitmentConfig::confirmed()
                                )
                            {
                                Ok(owner_response) => {
                                    if let Some(account) = owner_response.value {
                                        if account.owner != program_id {
                                            debug!(
                                                "Account {} is not owned by program {}. Owner: {}",
                                                account_pubkey,
                                                program_id,
                                                account.owner
                                            );
                                            continue;
                                        }
                                        info!(
                                            "Account {} owned by program {}. Deserializing...",
                                            account_pubkey,
                                            program_id
                                        );
                                        deserialize_and_print_account_data(&account_pubkey, &data);
                                    } else {
                                        debug!("Account {} not found after initial data check.", account_pubkey);
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        "Failed to get account owner info for {}: {}",
                                        account_pubkey,
                                        e
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            warn!("Failed to fetch account data for {}: {}", account_pubkey, e);
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

fn deserialize_and_print_account_data(account_pubkey: &Pubkey, data: &[u8]) {
    if data.is_empty() {
        info!("Account {} data is empty, possibly closed or uninitialized.", account_pubkey);
        return;
    }

    if let Ok(post) = Post::try_deserialize(&mut &data[..]) {
        info!("Deserialized Post ({}): {:?}", account_pubkey, post);
        return;
    }

    if let Ok(profile) = AgentProfile::try_deserialize(&mut &data[..]) {
        info!("Deserialized AgentProfile ({}): {:?}", account_pubkey, profile);
        return;
    }

    if let Ok(like_record) = LikeRecord::try_deserialize(&mut &data[..]) {
        info!("Deserialized LikeRecord ({}): {:?}", account_pubkey, like_record);
        return;
    }

    if let Ok(repost_record) = RepostRecord::try_deserialize(&mut &data[..]) {
        info!("Deserialized RepostRecord ({}): {:?}", account_pubkey, repost_record);
        return;
    }

    if let Ok(follow_record) = FollowRecord::try_deserialize(&mut &data[..]) {
        info!("Deserialized FollowRecord ({}): {:?}", account_pubkey, follow_record);
        return;
    }

    warn!(
        "Could not deserialize account data for {} (len {}). First 8 bytes (discriminator?): {:?}. This may indicate an unknown account type or corrupted data.",
        account_pubkey,
        data.len(),
        &data[..std::cmp::min(8, data.len())]
    );
}

// TODO:
// 1. Implement robust discriminator checking in `deserialize_and_print_account_data`.
//    You can get discriminators by calling `MyAccount::discriminator()` if your `models.rs` structs
//    were actual Anchor `#[account]` structs. Since we replicated them, you might need to
//    pre-calculate them or fetch from IDL. For `#[derive(InitSpace)]` on plain structs,
//    there's no automatic discriminator like `#[account]`.
//
//    The `try_deserialize`
