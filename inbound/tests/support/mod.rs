use alloy_primitives::{B256, Bytes, U256, keccak256};
use alloy_trie::{HashBuilder, Nibbles, TrieAccount, proof::ProofRetainer};
use arcane_attestation_actor::staged::history;
use arcane_attestation_core::{Predicate, StorageRequest};
use lee_core::account::Actor;
use redemption_inbound::{
    Claim, Policy, SEPOLIA_CHAIN_ID, SEPOLIA_GENESIS, commitment, deposit_slot,
};
use serde_json::json;

fn leaf(key: B256, value: &[u8]) -> (B256, Vec<Bytes>) {
    let path = Nibbles::unpack(key);
    let mut trie = HashBuilder::default().with_proof_retainer(ProofRetainer::from_iter([path]));
    trie.add_leaf(path, value);
    let root = trie.root();
    let proof = trie.take_proof_nodes().into_nodes_sorted();
    (root, proof.into_iter().map(|(_, node)| node).collect())
}

pub fn proof(policy: &Policy, receiver: Actor, secret: B256, block: u8, value: U256) -> Claim {
    let slot = deposit_slot(commitment(policy, receiver, secret));
    let (storage_root, storage_proof) = leaf(keccak256(slot), &alloy_rlp::encode(value));
    let account = TrieAccount {
        storage_root,
        ..TrieAccount::default()
    };
    let (state_root, account_proof) = leaf(keccak256(policy.escrow), &alloy_rlp::encode(account));
    let beacon_root = B256::repeat_byte(block);
    let context = serde_json::from_value(json!({
        "chain_id": SEPOLIA_CHAIN_ID,
        "instance_id": policy.instance_id,
        "genesis_validators_root": SEPOLIA_GENESIS,
        "execution": {
            "beacon_root": beacon_root,
            "slot": u64::from(block),
            "block_hash": beacon_root,
            "state_root": state_root,
            "receipts_root": B256::ZERO
        }
    }))
    .unwrap();
    Claim {
        secret,
        context,
        request: StorageRequest {
            instance_id: policy.instance_id,
            chain_id: SEPOLIA_CHAIN_ID,
            beacon_root,
            execution_header: None,
            address: policy.escrow,
            account,
            account_proof,
            slot,
            value,
            storage_proof,
            predicate: Predicate::Eq(value),
        },
        index: 0,
        siblings: [B256::ZERO; history::DEPTH],
    }
}
