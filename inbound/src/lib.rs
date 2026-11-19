//! The wrapped asset's inbound LEZ actor, backed by the RFP-021 finalized header history.
use alloy_primitives::{Address, B256, U256, keccak256};
use arcane_attestation_actor::{Command, staged::history};
use arcane_attestation_core::{Error, Predicate, StorageContext, StorageRequest};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ReceiveInput, Response, SyncMessage},
};
use serde::{Deserialize, Serialize};

#[path = "../../redemption/actor/src/lib.rs"]
#[allow(
    dead_code,
    reason = "also built independently as the outbound prototype"
)]
mod wrapped;
pub use wrapped::{Message, read_balance};

pub const SEPOLIA_CHAIN_ID: u64 = 11155111;
pub const SEPOLIA_GENESIS: B256 =
    alloy_primitives::b256!("d8ea171f3c94aea21ebc42a1ed61052acf3f9209c00e4efbaaddac09ed9b8078");
const CLAIMED: &[u8] = &[1];

/// Compiled into the mint program. The claimant cannot choose its tracker or escrow.
#[derive(Deserialize, Serialize)]
pub struct Policy {
    pub escrow: Address,
    pub instance_id: B256,
    pub verifier_program: AccountId,
}

/// Passed as private transaction data by the wallet.
#[derive(Deserialize, Serialize)]
pub struct Claim {
    pub secret: B256,
    pub context: StorageContext,
    pub request: StorageRequest,
    pub index: u32,
    pub siblings: [B256; history::DEPTH],
}

fn note_hash(domain: &[u8], policy: &Policy, receiver: Actor, secret: B256) -> B256 {
    keccak256(
        [
            domain,
            &SEPOLIA_CHAIN_ID.to_be_bytes(),
            SEPOLIA_GENESIS.as_slice(),
            policy.escrow.as_slice(),
            receiver.program_account_id.as_ref(),
            receiver.account_id.as_ref(),
            secret.as_slice(),
        ]
        .concat(),
    )
}

pub fn commitment(policy: &Policy, receiver: Actor, secret: B256) -> B256 {
    note_hash(b"RFP022/inbound/deposit/v1", policy, receiver, secret)
}

pub fn nullifier(policy: &Policy, receiver: Actor, secret: B256) -> B256 {
    note_hash(b"RFP022/inbound/spend/v1", policy, receiver, secret)
}

pub fn deposit_slot(commitment: B256) -> B256 {
    keccak256([commitment, keccak256(b"RFP022.deposits.v1")].concat())
}

pub fn claim_marker(program: AccountId, nullifier: B256) -> Actor {
    Actor::new(
        AccountId::for_public_pda(&program, &PdaSeed::new(nullifier.0)),
        program,
    )
}

fn claim(input: &ReceiveInput, policy: &Policy, mut claim: Claim) -> Result<Response, Error> {
    let receiver = input.receiver;
    let program = receiver.program_account_id;
    if input.from.is_some()
        || !input.is_authorized
        || receiver == wrapped::sink_for(program)
        || input.pre_state.as_ref() == CLAIMED
        || claim.secret == B256::ZERO
        || policy.escrow == Address::ZERO
        || policy.instance_id == B256::ZERO
        || policy.verifier_program == program
        || claim.request.instance_id != policy.instance_id
        || claim.request.chain_id != SEPOLIA_CHAIN_ID
        || claim.request.address != policy.escrow
        || claim.request.slot != deposit_slot(commitment(policy, receiver, claim.secret))
        || claim.request.execution_header.is_some()
    {
        return Err(Error::Binding);
    }
    claim.request.predicate = Predicate::Gt(U256::ZERO);
    let statement = claim.context.verify(&claim.request)?;
    if statement.genesis_validators_root != SEPOLIA_GENESIS {
        return Err(Error::Binding);
    }
    let amount = u128::try_from(statement.value).map_err(|_| Error::Malformed)?;
    let balance = read_balance(&input.pre_state)
        .checked_add(amount)
        .ok_or(Error::Malformed)?;
    let root = history::proof_root(history::leaf(&claim.context), claim.index, &claim.siblings)?;
    let tracker = Actor::new(
        AccountId::new(policy.instance_id.0),
        policy.verifier_program,
    );
    let require_root =
        serde_json::to_vec(&Command::RequireHeaderRoot(root)).map_err(|_| Error::Malformed)?;
    let id = nullifier(policy, receiver, claim.secret);
    let marker = claim_marker(program, id);
    if receiver == marker || marker == wrapped::sink_for(program) {
        return Err(Error::Binding);
    }
    // LEZ applies the private balance and public consume together, after both sync calls succeed.
    let consume =
        SyncMessage::new(marker, &Message::Consume(id.0)).with_pda_seeds([PdaSeed::new(id.0)]);
    Ok(
        Response::set_state(borsh::to_vec(&balance).map_err(|_| Error::Malformed)?)
            .send_sync(tracker, &require_root)
            .send(consume),
    )
}

pub fn handle(input: &ReceiveInput, policy: &Policy, message: Message) -> Result<Response, Error> {
    match message {
        Message::Claim(bytes) => claim(
            input,
            policy,
            serde_json::from_slice(&bytes).map_err(|_| Error::Malformed)?,
        ),
        Message::Consume(id) => {
            if !input.is_authorized
                || !input.from_own_program()
                || input.from == Some(input.receiver)
                || input.receiver != claim_marker(input.receiver.program_account_id, id.into())
            {
                return Err(Error::Binding);
            }
            if !input.pre_state.is_empty() {
                return Err(Error::Duplicate);
            }
            Ok(Response::set_state(CLAIMED.to_vec()))
        }
        message => {
            if input.pre_state.as_ref() == CLAIMED {
                return Err(Error::Binding);
            }
            Ok(wrapped::handle_wrapped(input, message))
        }
    }
}
