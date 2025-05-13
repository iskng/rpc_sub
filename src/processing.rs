use crate::errors::IndexerError;
use crate::models::*;
use crate::constants::*;
use anchor_client::{
    solana_client::{
        nonblocking::rpc_client::RpcClient as NonBlockingRpcClient,
        client_error::ClientErrorKind,
        rpc_request::RpcError,
    },
    solana_sdk::{ commitment_config::CommitmentConfig, pubkey::Pubkey, signature::Signature },
};
use anchor_lang::AccountDeserialize;
use log::{ debug, error, info, warn };
use solana_transaction_status::{
    option_serializer::OptionSerializer,
    EncodedConfirmedTransactionWithStatusMeta,
    UiTransactionEncoding,
};
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::time::sleep;
use anchor_client::solana_client::rpc_response::RpcLogsResponse;
use anchor_client::solana_sdk; // For solana_sdk::message::VersionedMessage
use std::sync::atomic::{ AtomicUsize, Ordering };
use std::sync::atomic::AtomicBool;

pub async fn fetch_transaction_with_retry(
    rpc_client: &Arc<NonBlockingRpcClient>,
    signature: &Signature,
    max_retries: usize,
    delay_ms: u64
) -> Result<Option<EncodedConfirmedTransactionWithStatusMeta>, IndexerError> {
    for attempt in 0..max_retries {
        match rpc_client.get_transaction(signature, UiTransactionEncoding::Base64).await {
            Ok(tx_detail) => {
                return Ok(Some(tx_detail));
            }
            Err(e) => {
                let error_str = e.to_string();
                if error_str.contains("invalid type: null") || error_str.contains("not found") {
                    if attempt + 1 == max_retries {
                        return Ok(None);
                    }
                    debug!(
                        "Transaction {} not found on attempt {}/{}, retrying after {}ms",
                        signature,
                        attempt + 1,
                        max_retries,
                        delay_ms
                    );
                } else {
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

pub fn deserialize_and_print_account_data(
    account_pubkey: &Pubkey,
    data: &[u8],
    signature_for_log: &str
) {
    debug!(
        "Enter deserialize_and_print_account_data for {}. Data len: {}. Data prefix (first 8 bytes): {:?} (Tx: {})",
        account_pubkey,
        data.len(),
        &data[..std::cmp::min(8, data.len())],
        signature_for_log
    );
    if data.is_empty() {
        info!(
            "Account {} data is empty, cannot deserialize. (Tx: {})",
            account_pubkey,
            signature_for_log
        );
        return;
    }

    if let Ok(post) = Post::try_deserialize(&mut &*data) {
        info!("Deserialized Post ({}): {:?} (Tx: {})", account_pubkey, post, signature_for_log);
        return;
    }

    if let Ok(profile) = AgentProfile::try_deserialize(&mut &*data) {
        info!(
            "Deserialized AgentProfile ({}): {:?} (Tx: {})",
            account_pubkey,
            profile,
            signature_for_log
        );
        return;
    }

    if let Ok(like_record) = LikeRecord::try_deserialize(&mut &*data) {
        info!(
            "Deserialized LikeRecord ({}): {:?} (Tx: {})",
            account_pubkey,
            like_record,
            signature_for_log
        );
        return;
    }

    if let Ok(repost_record) = RepostRecord::try_deserialize(&mut &*data) {
        info!(
            "Deserialized RepostRecord ({}): {:?} (Tx: {})",
            account_pubkey,
            repost_record,
            signature_for_log
        );
        return;
    }

    if let Ok(follow_record) = FollowRecord::try_deserialize(&mut &*data) {
        info!(
            "Deserialized FollowRecord ({}): {:?} (Tx: {})",
            account_pubkey,
            follow_record,
            signature_for_log
        );
        return;
    }

    warn!(
        "Could not deserialize account data for {} (len {}). Data prefix (8 bytes): {:?}. (Tx: {}). This may indicate an unknown account type for this program, data corruption, or an account associated with a CPI from/to another program.",
        account_pubkey,
        data.len(),
        &data[..std::cmp::min(8, data.len())],
        signature_for_log
    );
}

pub async fn start_processing_loop(
    rpc_client: Arc<NonBlockingRpcClient>,
    program_id: Pubkey, // Assuming this is the intended program_id for matching instructions
    mut rx_log_processor: tokio::sync::mpsc::Receiver<RpcLogsResponse>,
    shutdown_signal: Arc<AtomicBool>,
    transactions_seen_counter: Arc<AtomicUsize> // For heartbeat
) -> u64 {
    // Return total transactions processed
    let mut transactions_total = 0;
    let mut processed_signatures = HashSet::new();

    info!("Starting main processing loop...");
    loop {
        tokio::select! {
            biased;
            _ = async {
                loop {
                    if shutdown_signal.load(Ordering::SeqCst) { break; }
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            } => {
                info!("Main processing loop shutting down due to signal (select).");
                break;
            }
            maybe_log_response = rx_log_processor.recv() => {
                if shutdown_signal.load(Ordering::SeqCst) {
                    info!("Main processing loop shutting down after recv due to signal.");
                    break;
                }

                if let Some(log_response) = maybe_log_response {
                    transactions_total += 1;
                    transactions_seen_counter.fetch_add(1, Ordering::SeqCst);

                    let RpcLogsResponse { signature, err, logs, .. } = log_response;

                    if !processed_signatures.insert(signature.clone()) {
                        info!("Skipping already processed transaction: {}", signature);
                        continue;
                    }

                    info!("Transaction #{}: {} (Logs: {})", transactions_total, signature, logs.len());

                    if let Some(tx_err) = err {
                        warn!("Transaction {} failed: {:?}. Logs: {:?}", signature, tx_err, logs);
                        continue;
                    }

                    debug!("Received logs for signature: {}", signature);
                    info!("Processing transaction: {} ({} logs)", signature, logs.len());

                    let signature_obj = match Signature::from_str(&signature) {
                        Ok(s) => s,
                        Err(e) => {
                            error!("Failed to parse signature string '{}': {}", signature, e);
                            continue;
                        }
                    };

                    let rpc_client_clone = rpc_client.clone();
                    let current_signature_for_task = signature.clone();
                    // program_id is already in scope from function arguments

                    tokio::spawn(async move {
                        match fetch_transaction_with_retry(&rpc_client_clone, &signature_obj, 20, 1000).await {
                            Ok(Some(tx_detail)) => {
                                if let Some(decoded_transaction) = tx_detail.transaction.transaction.decode() {
                                    let account_keys_slice: &[Pubkey] = match &decoded_transaction.message {
                                        solana_sdk::message::VersionedMessage::Legacy(m) => &m.account_keys,
                                        solana_sdk::message::VersionedMessage::V0(m) => &m.account_keys,
                                    };
                                    let account_keys: Vec<Pubkey> = account_keys_slice.to_vec();
                                    let mut unique_accounts_to_fetch = HashSet::new();

                                    for (idx, inst) in decoded_transaction.message.instructions().iter().enumerate() {
                                        let prog_key_idx = inst.program_id_index as usize;
                                        if prog_key_idx < account_keys.len() && account_keys[prog_key_idx] == program_id {
                                            info!(
                                                "  Instruction #{} targets program {}. Data len: {} (Tx: {})",
                                                idx,
                                                program_id, // Use program_id from args
                                                inst.data.len(),
                                                current_signature_for_task
                                            );
                                            if inst.data.len() >= 8 {
                                                let discriminator = &inst.data[..8];
                                                match discriminator {
                                                    x if x == CREATE_POST_DISCRIMINATOR => info!("    Detected: create_post (Tx: {})", current_signature_for_task),
                                                    x if x == REPLY_TO_POST_DISCRIMINATOR => info!("    Detected: reply_to_post (Tx: {})", current_signature_for_task),
                                                    x if x == LIKE_POST_DISCRIMINATOR => info!("    Detected: like_post (Tx: {})", current_signature_for_task),
                                                    x if x == UNLIKE_POST_DISCRIMINATOR => info!("    Detected: unlike_post (Tx: {})", current_signature_for_task),
                                                    x if x == REPOST_POST_DISCRIMINATOR => info!("    Detected: repost_post (Tx: {})", current_signature_for_task),
                                                    x if x == UNREPOST_POST_DISCRIMINATOR => info!("    Detected: unrepost_post (Tx: {})", current_signature_for_task),
                                                    x if x == CREATE_OR_UPDATE_PROFILE_DISCRIMINATOR => info!("    Detected: create_or_update_profile (Tx: {})", current_signature_for_task),
                                                    x if x == FOLLOW_AGENT_DISCRIMINATOR => info!("    Detected: follow_agent (Tx: {})", current_signature_for_task),
                                                    x if x == UNFOLLOW_AGENT_DISCRIMINATOR => info!("    Detected: unfollow_agent (Tx: {})", current_signature_for_task),
                                                    _ => warn!("    Unknown instruction discriminator: {:?} (Tx: {})", discriminator, current_signature_for_task),
                                                }
                                            } else {
                                                warn!("    Instruction data too short for discriminator. (Tx: {})", current_signature_for_task);
                                            }
                                            for acc_idx in &inst.accounts {
                                                if (*acc_idx as usize) < account_keys.len() {
                                                    unique_accounts_to_fetch.insert(account_keys[*acc_idx as usize]);
                                                }
                                            }
                                        } else {
                                            debug!("  Instruction #{} targets different program: {} (Tx: {})", idx, if prog_key_idx < account_keys.len() { account_keys[prog_key_idx].to_string() } else { "<Index out of bounds>".to_string() }, current_signature_for_task);
                                        }
                                    }

                                    if let Some(meta) = tx_detail.transaction.meta.as_ref() {
                                        match &meta.loaded_addresses {
                                            OptionSerializer::Some(loaded_addresses_val) => {
                                                for key_str in loaded_addresses_val.writable.iter().chain(&loaded_addresses_val.readonly) {
                                                    match Pubkey::from_str(key_str) {
                                                        Ok(pubkey) => { unique_accounts_to_fetch.insert(pubkey); }
                                                        Err(e) => { warn!("Failed to parse loaded address pubkey string '{}': {} (Tx: {})", key_str, e, current_signature_for_task); }
                                                    }
                                                }
                                            }
                                            OptionSerializer::None | OptionSerializer::Skip => {}
                                        }
                                    }

                                    info!("Identified {} unique accounts to potentially fetch for tx {}", unique_accounts_to_fetch.len(), current_signature_for_task);

                                    for account_pubkey in unique_accounts_to_fetch {
                                        debug!("Attempting to fetch account: {} (Tx: {})", account_pubkey, current_signature_for_task);
                                        match rpc_client_clone.get_account_with_commitment(&account_pubkey, CommitmentConfig::finalized()).await {
                                            Ok(response) => {
                                                if let Some(account) = response.value {
                                                    debug!("Processing fetched account {} for deserialization. Owner: {}, Data len: {}. Is program owned: {} (Tx: {})", account_pubkey, account.owner, account.data.len(), account.owner == program_id, current_signature_for_task);
                                                    if account.owner != program_id {
                                                        debug!("Account {} is not owned by program {}. Owner: {} (Tx: {})", account_pubkey, program_id, account.owner, current_signature_for_task);
                                                        continue;
                                                    }
                                                    if account.data.is_empty() {
                                                        debug!("Account {} owned by program {} but has no data. (Tx: {})", account_pubkey, program_id, current_signature_for_task);
                                                        continue;
                                                    }
                                                    info!("Account {} owned by program {}. Deserializing... (Tx: {})", account_pubkey, program_id, current_signature_for_task);
                                                    deserialize_and_print_account_data(&account_pubkey, &account.data, &current_signature_for_task);
                                                } else {
                                                    debug!("Account {} not found (likely closed). (Tx: {})", account_pubkey, current_signature_for_task);
                                                }
                                            }
                                            Err(e) => {
                                                if let ClientErrorKind::RpcError(RpcError::ForUser(s)) = &e.kind {
                                                    if s.contains("AccountNotFound") || s.contains("was not found") {
                                                        debug!("Account {} not found (likely closed): {} (Tx: {})", account_pubkey, s, current_signature_for_task);
                                                        continue;
                                                    }
                                                }
                                                warn!("Failed to fetch account info/owner for {}: {} (Tx: {})", account_pubkey, e, current_signature_for_task);
                                            }
                                        }
                                    }
                                } else {
                                    warn!("Failed to decode transaction: {} (Tx: {})", signature_obj, current_signature_for_task);
                                }
                            }
                            Ok(None) => {
                                warn!("Transaction {} not found after retries", signature_obj);
                            }
                            Err(e) => {
                                error!("Error fetching transaction {}: {:?}", signature_obj, e);
                            }
                        }
                    });
                } else {
                    info!("Log processor channel closed. Main processing loop shutting down.");
                    break;
                }
            }
        }
    }
    transactions_total
}
