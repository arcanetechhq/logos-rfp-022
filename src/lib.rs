#[path = "../checkpoint-guest/src/block_link.rs"]
pub mod block_link;
#[cfg(feature = "settlement")]
pub use settlement_guest as block_settlement;
pub mod exit_index;
