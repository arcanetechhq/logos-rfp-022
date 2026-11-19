use alloy_consensus::Header;
use alloy_primitives::{Address, B256, Bytes, U64, U256};
use alloy_trie::TrieAccount;
use anyhow::{Result, bail, ensure};
use arcane_attestation_actor::{
    projected::record_actor,
    staged::{
        StagedCommand, StagedState, bootstrap_wire, decode_state, encode,
        update::{job_actor, job_digest, wire},
    },
};
use arcane_attestation_core::{Config, Instance, Predicate, Spec, StorageContext, StorageRequest};
use helios_consensus_core::types::{Bootstrap, FinalityUpdate, GenericUpdate};
use lee::{
    Account, AccountId, Actor, PrivateKey, PublicTransaction, V03State, ValidatedStateDiff,
    public_transaction::{Message, WitnessSet},
};
use redemption_inbound::{Policy, SEPOLIA_CHAIN_ID, SEPOLIA_GENESIS};
use serde::Deserialize;
use std::collections::BTreeMap;

pub const CAP: u64 = 10_000_000;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetProof {
    address: Address,
    nonce: U64,
    balance: U256,
    code_hash: B256,
    storage_hash: B256,
    account_proof: Vec<Bytes>,
    storage_proof: [SlotProof; 1],
}

#[derive(Deserialize)]
struct SlotProof {
    key: U256,
    value: U256,
    proof: Vec<Bytes>,
}

#[derive(Deserialize)]
pub struct FinalityInputs {
    pub config: Config,
    pub bootstrap: Bootstrap<Spec>,
    pub bootstrap_execution: Header,
    pub update: FinalityUpdate<Spec>,
    pub update_execution: Header,
}

#[derive(Deserialize)]
pub struct Inputs {
    pub finality: FinalityInputs,
    pub proofs: [GetProof; 2],
}

pub struct Finalized {
    pub context: StorageContext,
    beacon_root: B256,
    pub execution_hash: B256,
}

impl Finalized {
    pub fn request(&self, proof: GetProof, expected_slot: B256) -> Result<StorageRequest> {
        let [slot] = proof.storage_proof;
        ensure!(
            slot.key == U256::from_be_bytes(expected_slot.0),
            "proof is for a different storage slot"
        );
        Ok(StorageRequest {
            instance_id: self.context.instance_id(),
            chain_id: SEPOLIA_CHAIN_ID,
            beacon_root: self.beacon_root,
            execution_header: None,
            address: proof.address,
            account: TrieAccount {
                nonce: proof.nonce.to(),
                balance: proof.balance,
                code_hash: proof.code_hash,
                storage_root: proof.storage_hash,
            },
            account_proof: proof.account_proof,
            slot: expected_slot,
            value: slot.value,
            storage_proof: slot.proof,
            predicate: Predicate::Gt(U256::ZERO),
        })
    }
}

pub fn actor_bytes(state: &V03State, actor: Actor) -> Vec<u8> {
    state
        .get_account_by_id(actor.account_id)
        .data
        .actor_state(actor.program_account_id)
        .to_vec()
}

fn admit(state: &mut V03State, actor: Actor) {
    if state.get_account_by_id_ref(actor.account_id).is_none() {
        *state =
            std::mem::take(state).with_public_accounts([(actor.account_id, Account::default())]);
    }
}

fn step(
    state: &mut V03State,
    root: Actor,
    actors: Vec<Actor>,
    command: StagedCommand,
    signer: Option<&PrivateKey>,
    now: u64,
) -> Result<()> {
    let nonces = signer.map_or_else(BTreeMap::new, |_| {
        BTreeMap::from([(
            root.account_id,
            state.get_account_by_id(root.account_id).nonce,
        )])
    });
    let message = Message::try_new(root, actors, nonces, encode(&command))?;
    let witnesses = WitnessSet::for_message(&message, &signer.into_iter().collect::<Vec<_>>());
    let tx = PublicTransaction::new(message, witnesses);
    let (diff, charge) = ValidatedStateDiff::from_public_transaction_with_cycle_budget(
        &tx,
        state,
        1,
        now * 1000,
        CAP,
    )?;
    state.apply_state_diff(diff)?;
    ensure!(charge.cycles <= CAP, "tracker cycle cap exceeded");
    Ok(())
}

fn finalized(instance: &Instance, header: &Header) -> Result<Finalized> {
    let beacon_root = instance.finalized_root();
    let execution = instance.execution(beacon_root, Some(header))?;
    Ok(Finalized {
        context: instance.storage_context(beacon_root, Some(header))?,
        beacon_root,
        execution_hash: execution.block_hash,
    })
}

pub fn apply(
    state: &mut V03State,
    policy: &Policy,
    inputs: FinalityInputs,
    owner: &PrivateKey,
    now: u64,
) -> Result<[Finalized; 2]> {
    let FinalityInputs {
        config,
        bootstrap,
        bootstrap_execution,
        update,
        update_execution,
    } = inputs;
    ensure!(
        config.chain_id == SEPOLIA_CHAIN_ID
            && config.genesis_validators_root == SEPOLIA_GENESIS
            && config.instance_id == policy.instance_id,
        "expected pinned Sepolia tracker"
    );
    let tracker = Actor::new(
        AccountId::new(policy.instance_id.0),
        policy.verifier_program,
    );
    let mut reference = Instance::initialize(config.clone(), &bootstrap, now)?;
    let first = finalized(&reference, &bootstrap_execution)?;
    reference.advance_finality(&update, now)?;
    let second = finalized(&reference, &update_execution)?;
    ensure!(
        first.execution_hash != second.execution_hash,
        "need two distinct headers"
    );
    let first_record = record_actor(policy.verifier_program, &first.context);
    let second_record = record_actor(policy.verifier_program, &second.context);

    for actor in [tracker, first_record, second_record] {
        admit(state, actor);
    }
    let (kind, bytes) = bootstrap_wire(&bootstrap);
    step(
        state,
        tracker,
        vec![tracker],
        StagedCommand::Bootstrap {
            config: serde_json::to_vec(&config)?,
            kind,
            bootstrap: bytes,
            execution_header: Some(serde_json::to_vec(&bootstrap_execution)?),
            now,
        },
        Some(owner),
        now,
    )?;
    for sequence in 0..2 {
        step(
            state,
            tracker,
            vec![tracker, first_record],
            StagedCommand::Continue { sequence, now },
            None,
            now,
        )?;
    }

    let update = wire(&GenericUpdate::from(&update), Some(&update_execution));
    let job = job_actor(
        policy.verifier_program,
        policy.instance_id.0,
        job_digest(&actor_bytes(state, tracker)),
        job_digest(&update),
    );
    admit(state, job);
    step(
        state,
        tracker,
        vec![tracker, job],
        StagedCommand::BeginUpdate { input: update, now },
        None,
        now,
    )?;
    for _ in 0..80 {
        let sequence = match decode_state(&actor_bytes(state, job))? {
            StagedState::Job(job) => job.sequence,
            StagedState::PendingCommit { sequence, .. } => sequence,
            StagedState::Done => {
                ensure!(actor_bytes(state, first_record) == serde_json::to_vec(&first.context)?);
                ensure!(actor_bytes(state, second_record) == serde_json::to_vec(&second.context)?);
                return Ok([first, second]);
            }
            _ => bail!("unexpected tracker job state"),
        };
        step(
            state,
            job,
            vec![job, tracker, second_record],
            StagedCommand::Continue { sequence, now },
            None,
            now,
        )?;
    }
    bail!("tracker update exceeded the staged phase bound")
}
