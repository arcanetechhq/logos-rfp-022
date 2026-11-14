//! Deterministic test inputs for the private burn and the local recipient.
//! These fixture parameters are public. Never use them for spendable assets.
use borsh::{BorshDeserialize, BorshSerialize};
use redemption_core::{Hash, Pairing, Payout};

pub fn note() -> (Pairing, Payout, Hash) {
    let mut evm_chain_id = [0; 32];
    evm_chain_id[30..].copy_from_slice(&31337u16.to_be_bytes());
    let pairing = Pairing {
        logos_genesis: [1; 32],
        zone: [2; 32],
        bridge_program: [0x42; 32],
        evm_chain_id,
        vault: [4; 20],
        token: [0; 20],
    };
    let payout = Payout {
        token: pairing.token,
        amount: 40,
        recipient: [6; 20],
        relayer: [7; 20],
        fee: 1,
    };
    (pairing, payout, [0x51; 32])
}

#[derive(BorshSerialize, BorshDeserialize)]
pub struct AppliedExitSnapshot {
    pub block_hash: Hash,
    pub state_digest: Hash,
    pub leaves: Vec<Hash>,
    pub root: Hash,
}
