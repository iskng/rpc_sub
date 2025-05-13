use anchor_client::solana_client::client_error::ClientError;
use anchor_client::solana_sdk::pubkey::ParsePubkeyError;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum IndexerError {
    #[error("Configuration Error: {0}")] ConfigError(String),

    #[error("Solana Client Error: {0}")] SolanaClientError(#[from] ClientError),

    #[error("RPC Error: {0}")] RpcError(String),

    #[error("WebSocket Error: {0}")] WebSocketError(String),

    #[error("Failed to parse Pubkey: {0}")] ParsePubkeyError(#[from] ParsePubkeyError),

    #[error("Deserialization Error: {0}")] DeserializationError(String),

    #[error("Serialization Error: {0}")] SerializationError(String),

    #[error("Account data not found for {0}")] AccountDataNotFound(String),

    #[error("Log processing error: {0}")] LogProcessingError(String),

    #[error("Environment variable not found: {0}")] EnvVarError(#[from] std::env::VarError),

    #[error("An unexpected error occurred: {0}")] Other(String),
}

// Helper for converting borsh deserialize errors
impl From<std::io::Error> for IndexerError {
    fn from(e: std::io::Error) -> Self {
        IndexerError::DeserializationError(format!("Borsh IO Error: {}", e))
    }
}
