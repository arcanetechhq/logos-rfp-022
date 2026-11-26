use super::Journal;
use borsh::{BorshDeserialize, BorshSerialize};

// Independent verifier pins for current and archived transfer settlement.
pub const SETTLEMENT_IMAGE: [u32; 8] = [
    1986472089, 3097709940, 2462758874, 3356233220, 1182519034, 2384855035, 2319619896, 639065794,
];
pub const ARCHIVED_SETTLEMENT_IMAGE: [u32; 8] = [
    722521674, 3113861519, 4014836451, 668905204, 2062045237, 4057429489, 2149499369, 1914569824,
];

#[derive(Debug, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
pub struct ChainedJournal {
    pub before: [u8; 32],
    pub after: [u8; 32],
    pub parent_hash: [u8; 32],
    pub block_hash: [u8; 32],
    pub payload_digests: [[u8; 32]; 2],
    pub pairing: [u8; 32],
    pub exit_root: [u8; 32],
    pub height: u64,
}

pub fn join(first: &Journal, next: &Journal) -> Option<ChainedJournal> {
    (next.parent_hash == first.block_hash
        && next.before == first.after
        && first.height.checked_add(1) == Some(next.height)
        && next.pairing == first.pairing)
        .then_some(ChainedJournal {
            before: first.before,
            after: next.after,
            parent_hash: first.parent_hash,
            block_hash: next.block_hash,
            payload_digests: [first.payload_digest, next.payload_digest],
            pairing: next.pairing,
            exit_root: next.exit_root,
            height: next.height,
        })
}
