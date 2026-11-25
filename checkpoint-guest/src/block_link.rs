//! Wire-level binding to one native LEZ block containing the proved action.
//! Fee settlement, clock execution and Logos finality are separate statements.
use acceptance_core::{Evidence, Fee, Journal, Tx, hash};
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    RootCall,
    account::{AccountId, Nonce},
    execution_state::PublicExecutionContext,
};
use std::collections::BTreeMap;

// The historical action image that produced the archived native transition receipt.
pub const ACTION_IMAGE: [u32; 8] = [
    0x5db3b8ea, 0x1f83a3da, 0x543cfb61, 0x9441c75e, 0x3250cc78, 0x55654a75, 0xf6b8dd2c, 0xc7b10f4e,
];
const BLOCK_DOMAIN: &[u8; 32] = b"/LEE/v0.3/Message/Block/\x00\x00\x00\x00\x00\x00\x00\x00";

// Native Borsh layout at the LEZ pin in reproduce.sh. Using the wire types keeps the
// host program executor and its IPC dependencies out of the zkVM guest.
#[derive(BorshDeserialize, BorshSerialize)]
struct Block {
    block_id: u64,
    parent: [u8; 32],
    hash: [u8; 32],
    timestamp: u64,
    transactions: Vec<Transaction>,
    receipts: Vec<TransactionReceipt>,
    status: BedrockStatus,
}

#[derive(BorshDeserialize, BorshSerialize)]
enum BedrockStatus {
    Pending,
    Safe,
    Finalized,
}

#[derive(BorshDeserialize, BorshSerialize)]
enum Transaction {
    Public(PublicTransaction),
    PrivacyPreserving(Tx),
}

#[derive(BorshDeserialize, BorshSerialize)]
struct PublicTransaction {
    public_context: PublicExecutionContext,
    root: RootCall,
    nonces: BTreeMap<AccountId, Nonce>,
    fee: Option<Fee>,
    admission_evidence: Vec<Evidence>,
    signatures: Vec<([u8; 64], [u8; 32])>,
}

#[derive(BorshDeserialize, BorshSerialize)]
struct TransactionReceipt {
    success: bool,
    gas_exec: u64,
    gas_stor: u64,
    fee_base: u128,
    tip: u128,
    refund: u128,
}

#[derive(BorshDeserialize, BorshSerialize)]
pub struct Input {
    pub block: Vec<u8>,
    pub action: Journal,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
pub struct BlockJournal {
    pub block_hash: [u8; 32],
    pub parent_hash: [u8; 32],
    pub payload_digest: [u8; 32],
    pub action: Journal,
}

#[derive(Debug, PartialEq, Eq)]
pub enum InvalidBlock {
    Encoding,
    Hash,
    Shape,
    RevertedAction,
    ActionMismatch,
}

/// The caller authenticates `input.action` against ACTION_IMAGE before calling this function.
pub fn bind(input: Input) -> Result<BlockJournal, InvalidBlock> {
    let block: Block = borsh::from_slice(&input.block).map_err(|_| InvalidBlock::Encoding)?;
    if borsh::to_vec(&block).map_err(|_| InvalidBlock::Encoding)? != input.block {
        return Err(InvalidBlock::Encoding);
    }
    let [
        Transaction::PrivacyPreserving(tx),
        Transaction::Public(_),
        Transaction::Public(_),
    ] = block.transactions.as_slice()
    else {
        return Err(InvalidBlock::Shape);
    };
    let [receipt] = block.receipts.as_slice() else {
        return Err(InvalidBlock::Shape);
    };

    let mut preimage = BLOCK_DOMAIN.to_vec();
    (
        block.block_id,
        block.parent,
        block.timestamp,
        &block.transactions,
        &block.receipts,
    )
        .serialize(&mut preimage)
        .map_err(|_| InvalidBlock::Encoding)?;
    if hash(&preimage) != block.hash {
        return Err(InvalidBlock::Hash);
    }
    if !receipt.success {
        return Err(InvalidBlock::RevertedAction);
    }
    if (block.block_id, block.timestamp) != (input.action.block, input.action.timestamp)
        || hash(&borsh::to_vec(tx).map_err(|_| InvalidBlock::Encoding)?) != input.action.tx_hash
    {
        return Err(InvalidBlock::ActionMismatch);
    }

    Ok(BlockJournal {
        block_hash: block.hash,
        parent_hash: block.parent,
        payload_digest: hash(&input.block),
        action: input.action,
    })
}
