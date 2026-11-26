#[path = "../checkpoint-guest/src/block_link.rs"]
pub mod block_link;
pub mod exit_index;
#[cfg(feature = "settlement")]
#[path = "../settlement-guest/src/lib.rs"]
pub mod block_settlement;
