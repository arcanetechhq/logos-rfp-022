use common::block::{BedrockStatus, Block};
use reverse_settlement_prototype::block_link::{BlockJournal, Input, InvalidBlock, bind};

const ACTION: &[u8] = include_bytes!("../evidence/acceptance-journal.borsh");
const BLOCK: &[u8] = include_bytes!("../evidence/accepted-burn-block.borsh");

fn native_block() -> Block {
    borsh::from_slice(BLOCK).unwrap()
}

fn bind_native(mut block: Block) -> Result<BlockJournal, InvalidBlock> {
    block.header.hash = block.recompute_hash();
    bind(Input {
        block: borsh::to_vec(&block).unwrap(),
        action: borsh::from_slice(ACTION).unwrap(),
    })
}

#[test]
fn proof_statement_matches_native_block_identity() {
    let block = native_block();
    let journal = bind_native(block.clone()).unwrap();
    assert_eq!(journal.block_hash, block.header.hash.0);
    assert_eq!(journal.parent_hash, block.header.prev_block_hash.0);
    assert_eq!(journal.action.tx_hash, block.body.transactions[0].hash().0);
    assert_eq!(journal.payload_digest, acceptance_core::hash(BLOCK));
}

#[test]
fn rejects_rehashed_reverted_or_unrelated_action() {
    let mut reverted = native_block();
    reverted.body.receipts[0].success = false;
    assert_eq!(bind_native(reverted), Err(InvalidBlock::RevertedAction));

    let mut other_time = native_block();
    other_time.header.timestamp += 1;
    assert_eq!(bind_native(other_time), Err(InvalidBlock::ActionMismatch));

    let mut other_action = native_block();
    let conflict: Block =
        borsh::from_slice(include_bytes!("../evidence/rejected-conflict-block.borsh")).unwrap();
    other_action.body.transactions[0] = conflict.body.transactions[0].clone();
    assert_eq!(bind_native(other_action), Err(InvalidBlock::ActionMismatch));
}

#[test]
fn rejects_malformed_or_unsupported_blocks() {
    let mut truncated = BLOCK.to_vec();
    truncated.pop();
    assert_eq!(
        bind(Input {
            block: truncated,
            action: borsh::from_slice(ACTION).unwrap(),
        }),
        Err(InvalidBlock::Encoding)
    );

    let mut bad_hash = native_block();
    bad_hash.header.hash.0[0] ^= 1;
    assert_eq!(
        bind(Input {
            block: borsh::to_vec(&bad_hash).unwrap(),
            action: borsh::from_slice(ACTION).unwrap(),
        }),
        Err(InvalidBlock::Hash)
    );

    let mut extra = native_block();
    extra
        .body
        .transactions
        .push(extra.body.transactions[0].clone());
    assert_eq!(bind_native(extra), Err(InvalidBlock::Shape));
}

#[test]
fn predecessor_and_exact_payload_remain_bound() {
    let base = bind_native(native_block()).unwrap();
    let mut branch = native_block();
    branch.header.prev_block_hash.0[0] ^= 1;
    let other = bind_native(branch).unwrap();
    assert_ne!(base.block_hash, other.block_hash);
    assert_ne!(base.parent_hash, other.parent_hash);
    assert_eq!(base.action, other.action);

    let mut relabelled = native_block();
    relabelled.bedrock_status = BedrockStatus::Finalized;
    let other = bind_native(relabelled).unwrap();
    assert_eq!(base.block_hash, other.block_hash);
    assert_ne!(base.payload_digest, other.payload_digest);
}
