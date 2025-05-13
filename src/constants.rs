pub const DEFAULT_PROGRAM_ID_STR: &str = "9AtCEeZCvLhNU4hypE69CmhiTzMejCz9vdpuRPH1SRjw";
pub const DEFAULT_RPC_URL: &str = "https://rpc.testnet.x1.xyz";

// Instruction discriminators (Updated from IDL)
pub const CREATE_POST_DISCRIMINATOR: [u8; 8] = [123, 92, 184, 29, 231, 24, 15, 202];
pub const REPLY_TO_POST_DISCRIMINATOR: [u8; 8] = [47, 124, 83, 114, 55, 170, 176, 188];
pub const LIKE_POST_DISCRIMINATOR: [u8; 8] = [45, 242, 154, 71, 63, 133, 54, 186];
pub const UNLIKE_POST_DISCRIMINATOR: [u8; 8] = [236, 63, 6, 34, 128, 3, 114, 174];
pub const REPOST_POST_DISCRIMINATOR: [u8; 8] = [175, 134, 193, 12, 27, 197, 61, 189];
pub const UNREPOST_POST_DISCRIMINATOR: [u8; 8] = [222, 76, 33, 179, 167, 222, 5, 21];
pub const CREATE_OR_UPDATE_PROFILE_DISCRIMINATOR: [u8; 8] = [52, 169, 99, 129, 231, 122, 119, 207];
pub const FOLLOW_AGENT_DISCRIMINATOR: [u8; 8] = [64, 202, 196, 131, 84, 241, 248, 38];
pub const UNFOLLOW_AGENT_DISCRIMINATOR: [u8; 8] = [124, 101, 198, 225, 35, 138, 12, 36];
