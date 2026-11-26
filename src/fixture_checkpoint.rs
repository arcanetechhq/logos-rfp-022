//! Host-side composition against the trusted output of the local Logos replay test.
//! The emitted tuple is only expected data for the EVM fixture.

use acceptance_core::{Journal as ActionJournal, hash};
use reverse_settlement_prototype::block_settlement::Journal;
#[path = "../settlement-guest/src/composition.rs"]
pub mod composition;
use composition::ChainedJournal;
use serde::Deserialize;
use std::{fs, path::Path};

#[derive(Deserialize)]
struct Replay {
    genesis: String,
    channel: String,
    selected_tip: String,
    slot: u64,
    selected_message: String,
    selected_payloads: [String; 2],
    displaced_payload: String,
    displaced_tip: String,
    displaced_message: String,
    displaced_slot: u64,
}

fn decode_hash(bytes: &str) -> [u8; 32] {
    hex::decode(bytes).unwrap().try_into().unwrap()
}

pub fn write(replay: &Path, output: &Path, chain: &ChainedJournal) {
    let replay: Replay = serde_json::from_slice(&fs::read(replay).unwrap()).unwrap();
    let selected = replay
        .selected_payloads
        .map(|bytes| hash(&hex::decode(bytes).unwrap()));
    assert_eq!(
        selected, chain.payload_digests,
        "ordered selected inscriptions"
    );
    let displaced = hex::decode(replay.displaced_payload).unwrap();
    assert!(!selected.contains(&hash(&displaced)));
    assert_eq!(
        displaced,
        include_bytes!("../evidence/accepted-burn-block.borsh")
    );
    let archived_action: ActionJournal =
        borsh::from_slice(include_bytes!("../evidence/acceptance-journal.borsh")).unwrap();
    assert_ne!(archived_action.exit_root, chain.exit_root);
    let domain = b"RFP022/local-replay-fixture/v1";
    let genesis = decode_hash(&replay.genesis);
    let channel = decode_hash(&replay.channel);
    let selected_tip = decode_hash(&replay.selected_tip);
    let message = decode_hash(&replay.selected_message);
    assert!(replay.slot > 0);
    let previous =
        hash(&borsh::to_vec(&(domain, genesis, channel, chain.parent_hash, chain.before)).unwrap());
    let checkpoint = hash(
        &borsh::to_vec(&(
            domain,
            previous,
            selected_tip,
            message,
            replay.slot,
            chain.block_hash,
            chain.after,
            selected,
            chain.pairing,
            chain.exit_root,
        ))
        .unwrap(),
    );
    assert!(replay.displaced_slot > 0 && replay.displaced_slot < replay.slot);
    let displaced_checkpoint = hash(
        &borsh::to_vec(&(
            domain,
            previous,
            decode_hash(&replay.displaced_tip),
            decode_hash(&replay.displaced_message),
            replay.displaced_slot,
            hash(&displaced),
            archived_action.exit_root,
        ))
        .unwrap(),
    );
    let hex_data = |bytes| format!("0x{}", hex::encode(bytes));
    let expected = serde_json::json!({
        "pairing": hex_data(chain.pairing),
        "previous": hex_data(previous),
        "next": hex_data(checkpoint),
        "exitRoot": hex_data(chain.exit_root),
        "displacedRoot": hex_data(archived_action.exit_root),
        "displacedCheckpoint": hex_data(displaced_checkpoint),
        "displacedSlot": replay.displaced_slot,
        "slot": replay.slot,
    });
    fs::write(output, serde_json::to_vec_pretty(&expected).unwrap()).unwrap();
    println!("PASS genuine chained settlement proof binds both selected inscriptions, in order");
}
