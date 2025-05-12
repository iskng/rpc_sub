use anchor_lang::prelude::*;
use borsh::{ BorshDeserialize, BorshSerialize };
use serde::{ Deserialize, Serialize };

// Client-side representations of on-chain accounts

#[derive(AnchorSerialize, AnchorDeserialize, Debug, Clone)]
pub struct Post {
    pub author: Pubkey,
    pub author_post_index: u64,
    pub timestamp: i64,
    pub content: String,
    pub parent_post: Option<Pubkey>,
    pub likes: u64,
    pub reposts: u64,
    pub replies_count: u64,
    pub bump: u8,
}

impl anchor_lang::Discriminator for Post {
    const DISCRIMINATOR: &'static [u8] = &[8, 147, 90, 186, 185, 56, 192, 150];
}

impl anchor_lang::AccountDeserialize for Post {
    fn try_deserialize(buf: &mut &[u8]) -> Result<Self> {
        if buf.len() < 8 {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorNotFound.into());
        }
        let given_disc = &buf[..8];
        if given_disc != Self::DISCRIMINATOR {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorMismatch.into());
        }
        *buf = &buf[8..];
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
    fn try_deserialize_unchecked(buf: &mut &[u8]) -> Result<Self> {
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Debug, Clone)]
pub struct AgentProfile {
    pub authority: Pubkey,
    pub display_name: String,
    pub avatar_url: String,
    pub bio: String,
    pub follower_count: u64,
    pub following_count: u64,
    pub post_count: u64,
    pub bump: u8,
}

impl anchor_lang::Discriminator for AgentProfile {
    const DISCRIMINATOR: &'static [u8] = &[60, 227, 42, 24, 0, 87, 86, 205];
}

impl anchor_lang::AccountDeserialize for AgentProfile {
    fn try_deserialize(buf: &mut &[u8]) -> Result<Self> {
        if buf.len() < 8 {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorNotFound.into());
        }
        let given_disc = &buf[..8];
        if given_disc != Self::DISCRIMINATOR {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorMismatch.into());
        }
        *buf = &buf[8..];
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
    fn try_deserialize_unchecked(buf: &mut &[u8]) -> Result<Self> {
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Debug, Clone)]
pub struct LikeRecord {
    pub liker: Pubkey,
    pub original_post: Pubkey,
    pub timestamp: i64,
    pub bump: u8,
}

impl anchor_lang::Discriminator for LikeRecord {
    const DISCRIMINATOR: &'static [u8] = &[179, 237, 53, 5, 91, 236, 161, 50];
}

impl anchor_lang::AccountDeserialize for LikeRecord {
    fn try_deserialize(buf: &mut &[u8]) -> Result<Self> {
        if buf.len() < 8 {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorNotFound.into());
        }
        let given_disc = &buf[..8];
        if given_disc != Self::DISCRIMINATOR {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorMismatch.into());
        }
        *buf = &buf[8..];
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
    fn try_deserialize_unchecked(buf: &mut &[u8]) -> Result<Self> {
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Debug, Clone)]
pub struct RepostRecord {
    pub reposter: Pubkey,
    pub original_post: Pubkey,
    pub timestamp: i64,
    pub bump: u8,
}

impl anchor_lang::Discriminator for RepostRecord {
    const DISCRIMINATOR: &'static [u8] = &[134, 201, 23, 191, 183, 203, 59, 13];
}

impl anchor_lang::AccountDeserialize for RepostRecord {
    fn try_deserialize(buf: &mut &[u8]) -> Result<Self> {
        if buf.len() < 8 {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorNotFound.into());
        }
        let given_disc = &buf[..8];
        if given_disc != Self::DISCRIMINATOR {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorMismatch.into());
        }
        *buf = &buf[8..];
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
    fn try_deserialize_unchecked(buf: &mut &[u8]) -> Result<Self> {
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
}

#[derive(AnchorSerialize, AnchorDeserialize, Debug, Clone)]
pub struct FollowRecord {
    pub follower: Pubkey,
    pub followed: Pubkey,
    pub timestamp: i64,
    pub bump: u8,
}

impl anchor_lang::Discriminator for FollowRecord {
    const DISCRIMINATOR: &'static [u8] = &[184, 58, 38, 54, 57, 20, 186, 26];
}

impl anchor_lang::AccountDeserialize for FollowRecord {
    fn try_deserialize(buf: &mut &[u8]) -> Result<Self> {
        if buf.len() < 8 {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorNotFound.into());
        }
        let given_disc = &buf[..8];
        if given_disc != Self::DISCRIMINATOR {
            return Err(anchor_lang::error::ErrorCode::AccountDiscriminatorMismatch.into());
        }
        *buf = &buf[8..];
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
    fn try_deserialize_unchecked(buf: &mut &[u8]) -> Result<Self> {
        Self::deserialize(buf).map_err(anchor_lang::error::Error::from)
    }
}
