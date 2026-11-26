mod attest;
#[cfg(test)]
#[path = "../../tests/support/mod.rs"]
mod support;

use alloy_primitives::{B256, keccak256};
use anyhow::{Context, Result, bail, ensure};
use arcane_attestation_actor::staged::history;
use lee::{
    AccountId, Actor, PrivacyPreservingTransaction, PrivateKey, PublicKey, V03State,
    ValidatedStateDiff,
    privacy_preserving_transaction::{
        Message as PrivateMessage, WitnessSet,
        circuit::{ProgramCatalog, Simulation, simulate_privacy_preserving_transaction},
    },
    program::Program,
};
use lee_core::{
    AuthorizationSecretKey, Commitment, NullifierWitness, PrivateWitness, ProvingInput, RegularKey,
    RootCall, SenderPresentation, WitnessKind,
    encryption::{EncryptionScheme, SharedSecretKey, ViewingPublicKey},
    execution_state::{PublicExecutionContext, TransactionEntry},
};
use redemption_inbound::{
    Claim, Message, Policy, claim_marker, commitment, deposit_slot, nullifier, read_balance,
};
use risc0_zkvm::InnerReceipt;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::json;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Deserialize, Serialize)]
struct Note {
    secret: B256,
    authorize: [u8; 32],
    viewing: [u8; 32],
    kem: [u8; 32],
}

impl Note {
    fn witness(&self, nullifier: NullifierWitness) -> PrivateWitness {
        PrivateWitness {
            vpk: ViewingPublicKey::from_seed(&self.viewing, &self.kem),
            kind: WitnessKind::Regular(RegularKey::Authorized(AuthorizationSecretKey(
                self.authorize,
            ))),
            random_seed: rand::random(),
            nullifier,
            address_openings: BTreeSet::new(),
        }
    }
}

fn program(path: &str) -> Result<Program> {
    let elf = fs::read(path).with_context(|| format!("reading guest {path}"))?;
    let binary = risc0_binfmt::ProgramBinary::new(&elf, risc0_zkos_v1compat::V1COMPAT_ELF).encode();
    Ok(Program::new(binary.into())?)
}

fn read<T: DeserializeOwned>(file: &str) -> Result<T> {
    serde_json::from_slice(&fs::read(file)?).with_context(|| format!("reading input {file}"))
}

fn actor_id(program: &Program) -> AccountId {
    AccountId::from_builtin_program(program.id())
}

fn now() -> Result<u64> {
    Ok(SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs())
}

fn owner() -> PrivateKey {
    // Signs only the tracker bootstrap in the isolated LEZ state created below.
    PrivateKey::try_new([0x30; 32]).unwrap()
}

fn proving_input(
    receiver: Actor,
    tracker: Actor,
    marker: Actor,
    claim: &Claim,
    witness: PrivateWitness,
) -> Result<ProvingInput> {
    Ok(ProvingInput {
        root: TransactionEntry::Sync(RootCall {
            to: receiver,
            message: borsh::to_vec(&Message::Claim(serde_json::to_vec(claim)?))?,
        }),
        claims: vec![],
        fee: None,
        public_context: PublicExecutionContext::new([tracker, marker], []),
        private_witnesses: vec![witness],
        dummy_inputs: vec![],
        ciphertext_padding: Some(256),
        recoveries: vec![],
        private_promotions: BTreeSet::new(),
    })
}

fn simulation(state: &V03State, tracker: Actor, marker: Actor) -> Simulation {
    Simulation {
        public_actor_states: [tracker, marker]
            .into_iter()
            .map(|actor| (actor, attest::actor_bytes(state, actor).into()))
            .collect(),
        admitted_accounts: Some(BTreeSet::from([tracker.account_id])),
    }
}

fn run(
    tracker_elf: &str,
    mint_elf: &str,
    policy: Policy,
    note: Note,
    inputs: attest::Inputs,
) -> Result<()> {
    ensure!(
        std::env::var("RISC0_DEV_MODE").as_deref() == Ok("0"),
        "RISC0_DEV_MODE=0 is required"
    );
    let tracker_program = program(tracker_elf)?;
    let mint_program = program(mint_elf)?;
    ensure!(
        actor_id(&tracker_program) == policy.verifier_program,
        "tracker image mismatch"
    );
    let mint_id = actor_id(&mint_program);
    let catalog: ProgramCatalog = [
        (policy.verifier_program, tracker_program.clone()),
        (mint_id, mint_program.clone()),
    ]
    .into();
    let mut state = V03State::new().with_programs([tracker_program, mint_program]);
    let now = now()?;
    let [first, second] = attest::apply(&mut state, &policy, inputs.finality, &owner(), now)?;
    let witness = note.witness(NullifierWitness::Init {
        commitment_root: state.commitment_root(),
    });
    let recipient = Actor::new(witness.account_id(), mint_id);
    let deposit = commitment(&policy, recipient, note.secret);
    let [proof_first, proof_second] = inputs.proofs;
    let request_first = first.request(proof_first, deposit_slot(deposit))?;
    let request_second = second.request(proof_second, deposit_slot(deposit))?;
    let fact_first = first.context.verify(&request_first)?;
    let fact_second = second.context.verify(&request_second)?;
    ensure!(fact_first.address == policy.escrow && fact_second.address == policy.escrow);
    ensure!(fact_first.value == fact_second.value && fact_first.fact_id != fact_second.fact_id);
    let amount = u128::try_from(fact_second.value)?;
    let claim_second = Claim {
        secret: note.secret,
        siblings: history::second_leaf_path(&first.context),
        context: second.context,
        request: request_second,
        index: 1,
    };
    let claim_first = Claim {
        secret: note.secret,
        siblings: history::second_leaf_path(&claim_second.context),
        context: first.context,
        request: request_first,
        index: 0,
    };
    println!(
        "Sepolia finality and deposit verified at {} and {}",
        first.execution_hash, second.execution_hash
    );
    prove_mint(
        &mut state,
        &catalog,
        &policy,
        recipient,
        &note,
        [&claim_first, &claim_second],
        amount,
        now,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "explicit native state and witness inputs"
)]
fn prove_mint(
    state: &mut V03State,
    catalog: &ProgramCatalog,
    policy: &Policy,
    recipient: Actor,
    note: &Note,
    [claim_first, claim_second]: [&Claim; 2],
    amount: u128,
    now: u64,
) -> Result<()> {
    let mint_id = recipient.program_account_id;
    let witness = note.witness(NullifierWitness::Init {
        commitment_root: state.commitment_root(),
    });
    let tracker = Actor::new(
        AccountId::new(policy.instance_id.0),
        policy.verifier_program,
    );
    let marker = claim_marker(mint_id, nullifier(policy, recipient, note.secret));
    let deposit = commitment(policy, recipient, note.secret);
    let present = |_| SenderPresentation::Blinded(rand::random());
    let input = proving_input(recipient, tracker, marker, claim_first, witness)?;
    let (output, proof) = lee::execute_and_prove(
        input,
        &simulation(state, tracker, marker),
        catalog,
        present,
        |_, _| false,
        |_| unreachable!("no asynchronous messages in an inbound claim"),
    )?;
    let raw_proof: InnerReceipt = borsh::from_slice(&proof.clone().into_inner())?;
    ensure!(
        matches!(raw_proof, InnerReceipt::Succinct(_)),
        "expected a genuine succinct receipt"
    );
    ensure!(
        output.execution.public_root.is_none(),
        "claim root must be private"
    );
    let tx_message = PrivateMessage::from_circuit_output(BTreeMap::new(), output);
    let signatures = WitnessSet::for_message(&tx_message, proof, &[]);
    let tx = PrivacyPreservingTransaction::new(tx_message, signatures);
    let bytes = borsh::to_vec(&tx)?;
    for private in [
        recipient.account_id.into_value(),
        note.secret.0,
        deposit.0,
        deposit_slot(deposit).0,
    ] {
        ensure!(
            !bytes.windows(32).any(|window| window == private),
            "claim witness leaked on wire"
        );
    }
    let (diff, charge) = ValidatedStateDiff::from_privacy_preserving_transaction_metered(
        &tx,
        state,
        2,
        now * 1000,
        attest::CAP,
    )?;
    ensure!(
        charge.cycles <= attest::CAP,
        "private settlement cycle cap exceeded"
    );
    state.apply_state_diff(diff)?;
    ensure!(
        attest::actor_bytes(state, marker) == [1],
        "claim was not consumed"
    );
    ensure!(
        attest::actor_bytes(state, recipient).is_empty(),
        "wrapped balance is public"
    );
    let [action] = tx.message.execution.private_actions.as_slice() else {
        bail!("expected one private account update");
    };
    let shared =
        SharedSecretKey::decapsulate(&action.encrypted_post_state.epk, &note.viewing, &note.kem)
            .context("private wallet could not recover shared secret")?;
    let (_, post) = EncryptionScheme::decrypt(
        &action.encrypted_post_state.ciphertext,
        &shared,
        &action.nullifier,
    )
    .context("private wallet could not read the accepted mint")?;
    ensure!(Commitment::new(&recipient.account_id, &post) == action.commitment);
    let membership_proof = state
        .get_proof_for_commitment(&action.commitment)
        .context("minted balance commitment missing from accepted state")?;
    ensure!(read_balance(post.data.actor_state(mint_id)) == amount);
    let before = keccak256(borsh::to_vec(state)?);
    ensure!(
        state
            .transition_from_privacy_preserving_transaction(&tx, 3, now * 1000)
            .is_err(),
        "replay of accepted private transaction succeeded"
    );

    // A fresh wallet witness from the minted account can use the second authenticated block.
    // The same candidate passes simulation with only the consume marker cleared.
    let replay = proving_input(
        recipient,
        tracker,
        marker,
        claim_second,
        note.witness(NullifierWitness::Update {
            account: post,
            membership_proof,
        }),
    )?;
    let current = simulation(state, tracker, marker);
    let mut unspent = current.clone();
    unspent.public_actor_states.remove(&marker);
    simulate_privacy_preserving_transaction(&replay, &unspent, catalog, present, |_, _| false)?;
    ensure!(
        simulate_privacy_preserving_transaction(&replay, &current, catalog, present, |_, _| false)
            .is_err(),
        "re-attested claim passed with an already consumed note"
    );
    ensure!(
        keccak256(borsh::to_vec(state)?) == before,
        "replay changed LEZ state"
    );
    if let Some(path) = std::env::var_os("RFP022_MINT_TX_OUT") {
        use std::io::Write;
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)?
            .write_all(&bytes)?;
    }
    println!(
        "PASS private_mint={} wrapped_wei={amount} cycles={} same_tx_rejected=true fresh_claim_rejected=true state_unchanged=true",
        B256::from(tx.hash()),
        charge.cycles
    );
    Ok(())
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [command, transaction] if command == "verify-mint-proof" => {
            let tx = PrivacyPreservingTransaction::from_bytes(&fs::read(transaction)?)?;
            tx.check_stateless()?;
            let inner: InnerReceipt =
                borsh::from_slice(&tx.witness_set().proof().clone().into_inner())?;
            ensure!(
                matches!(inner, InnerReceipt::Succinct(_)),
                "expected a succinct receipt"
            );
            ensure!(
                tx.message.execution.public_root.is_none(),
                "expected private root"
            );
            let journal =
                lee_core::to_borsh_frame(&(&tx.message.public_context, &tx.message.execution));
            risc0_zkvm::Receipt::new(inner, journal).verify(lee::PRIVACY_PRESERVING_CIRCUIT_ID)?;
            println!("PASS private transaction proof {}", B256::from(tx.hash()));
        }
        [command, tracker, escrow] if command == "policy" => {
            let policy = Policy {
                escrow: escrow.parse()?,
                instance_id: B256::from_slice(
                    AccountId::from(&PublicKey::new_from_private_key(&owner())).as_ref(),
                ),
                verifier_program: actor_id(&program(tracker)?),
            };
            println!("{}", serde_json::to_string_pretty(&policy)?);
        }
        [command, mint, policy, note_file] if command == "note" => {
            let policy: Policy = read(policy)?;
            let note = Note {
                secret: rand::random::<[u8; 32]>().into(),
                authorize: rand::random(),
                viewing: rand::random(),
                kem: rand::random(),
            };
            let file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(note_file)?;
            serde_json::to_writer(file, &note)?;
            let proof_key = note.witness(NullifierWitness::Init {
                commitment_root: [0; 32],
            });
            let receiver = Actor::new(proof_key.account_id(), actor_id(&program(mint)?));
            let deposit = commitment(&policy, receiver, note.secret);
            println!(
                "{}",
                json!({"commitment": deposit, "slot": deposit_slot(deposit)})
            );
        }
        [command, tracker, policy, finality] if command == "attest" => {
            let tracker = program(tracker)?;
            let policy: Policy = read(policy)?;
            ensure!(
                actor_id(&tracker) == policy.verifier_program,
                "tracker image mismatch"
            );
            let mut state = V03State::new().with_programs([tracker]);
            let finalized = attest::apply(&mut state, &policy, read(finality)?, &owner(), now()?)?;
            println!(
                "PASS native LEZ tracker finalized {} and {}",
                finalized[0].execution_hash, finalized[1].execution_hash
            );
        }
        [command, tracker, mint, policy, note, inputs] if command == "run" => {
            run(tracker, mint, read(policy)?, read(note)?, read(inputs)?)?;
        }
        _ => bail!(
            "usage: inbound-run policy tracker.elf escrow | note mint.elf policy.json private-note.json | attest tracker.elf policy.json finality.json | run tracker.elf mint.elf policy.json private-note.json inputs.json | verify-mint-proof transaction.borsh"
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::U256;

    #[test]
    #[ignore = "requires built actor images and the pinned rc3 executor"]
    fn native_simulation_checks_anchor_and_rejects_fresh_replay() -> Result<()> {
        let tracker_program = program(&std::env::var("RFP022_TRACKER_ELF")?)?;
        let mint_program = program(&std::env::var("RFP022_MINT_ELF")?)?;
        let policy: Policy = read(&std::env::var("RFP022_INBOUND_POLICY")?)?;
        let mint = actor_id(&mint_program);
        let tracker = Actor::new(
            AccountId::new(policy.instance_id.0),
            policy.verifier_program,
        );
        let programs = [
            (policy.verifier_program, tracker_program.clone()),
            (mint, mint_program.clone()),
        ]
        .into();
        let note = Note {
            secret: B256::repeat_byte(9),
            authorize: [7; 32],
            viewing: [8; 32],
            kem: [6; 32],
        };
        let witness = || {
            note.witness(NullifierWitness::Init {
                commitment_root: [0; 32],
            })
        };
        let receiver = Actor::new(witness().account_id(), mint);
        let marker = claim_marker(mint, nullifier(&policy, receiver, note.secret));
        let mut first = support::proof(&policy, receiver, note.secret, 21, U256::from(40));
        let mut second = support::proof(&policy, receiver, note.secret, 22, U256::from(40));
        first.siblings = history::second_leaf_path(&second.context);
        second.index = 1;
        second.siblings = history::second_leaf_path(&first.context);
        let root =
            history::proof_root(history::leaf(&first.context), first.index, &first.siblings)?;
        // Synthetic authenticated state for the runtime test. The live path builds it from
        // signed Ethereum consensus inputs with attest::apply.
        let history = borsh::to_vec(&(2u32, [[0u8; 32]; history::DEPTH], vec![root.0]))?;
        let history = borsh::from_slice::<history::HeaderHistory>(&history)?;
        let state = arcane_attestation_actor::staged::StagedState::Active {
            revision: 1,
            snapshot: vec![],
            history: Box::new(history),
        };
        let mut tracker_state = b"ASS4".to_vec();
        tracker_state.extend(borsh::to_vec(&state)?);
        let initial = Simulation {
            public_actor_states: [(tracker, tracker_state.clone().into())].into(),
            admitted_accounts: Some(BTreeSet::from([tracker.account_id])),
        };
        let check = |claim: &Claim, simulation: &Simulation| {
            let input = proving_input(receiver, tracker, marker, claim, witness()).unwrap();
            simulate_privacy_preserving_transaction(
                &input,
                simulation,
                &programs,
                |_| SenderPresentation::Blinded([11; 32]),
                |_, _| false,
            )
        };
        check(&first, &initial)?;
        check(&second, &initial)?;
        let mut spent = initial.clone();
        spent.public_actor_states.insert(marker, vec![1].into());
        ensure!(
            check(&second, &spent).is_err(),
            "runtime accepted a consumed deposit"
        );
        if std::env::var("RFP022_PROVE_FIXTURE").as_deref() == Ok("1") {
            ensure!(std::env::var("RISC0_DEV_MODE").as_deref() == Ok("0"));
            let tracker_account = lee::Account::default()
                .with_actor_state(policy.verifier_program, tracker_state.into());
            let mut state = V03State::new()
                .with_programs([mint_program, tracker_program])
                .with_public_accounts([(tracker.account_id, tracker_account)]);
            println!("Proving a native private mint against synthetic Ethereum fixture state");
            prove_mint(
                &mut state,
                &programs,
                &policy,
                receiver,
                &note,
                [&first, &second],
                40,
                now()?,
            )?;
        }
        second.siblings[0] = B256::ZERO;
        ensure!(
            check(&second, &initial).is_err(),
            "runtime accepted an unauthenticated header"
        );
        Ok(())
    }
}
