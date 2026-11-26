#[path = "../checkpoint-guest/src/block_link.rs"]
pub mod block_link;
#[cfg(feature = "settlement")]
#[path = "../settlement-guest/src/lib.rs"]
pub mod block_settlement;
pub mod exit_index;
