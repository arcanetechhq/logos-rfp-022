mod lez_blocks;
#[path = "../redemption/fixture.rs"]
mod redemption_fixture;
use burn_core::{Holding, Message, TOKEN, sink};
use common::transaction::{LeeTransaction, clock_invocation};
use lee::{
    FeeDeclaration, PrivacyPreservingTransaction, PrivateKey, PublicKey, V03State,
    privacy_preserving_transaction::{
        Message as PrivateMessage, WitnessSet as PrivateWitnessSet,
        circuit::{ProgramCatalog, Proof, Simulation},
    },
};
use lee_core::{
    AuthorizationSecretKey, Commitment, NullifierPublicKey, NullifierWitness, PrivateWitness,
    ProvingInput, RegularKey, RootCall, SenderPresentation, WitnessKind,
    account::{Account, AccountId, Actor, Nonce},
    encryption::ViewingPublicKey,
    execution_state::{PublicExecutionContext, TransactionEntry},
    program::TransactionEvent,
};
use lez_blocks::{accept, build_block};
use risc0_zkvm::{InnerReceipt, Receipt};
use std::{
    borrow::Cow,
    collections::{BTreeMap, BTreeSet},
    time::Instant,
};

fn state_digest(state: &V03State) -> [u8; 32] {
    redemption_core::state_commitment(&borsh::to_vec(state).unwrap())
}
fn signed(
    output: lee_core::PrivacyPreservingCircuitOutput,
    proof: Proof,
    key: &PrivateKey,
    state: &V03State,
) -> PrivacyPreservingTransaction {
    let payer = AccountId::from(&PublicKey::new_from_private_key(key));
    let mut message = PrivateMessage::from_circuit_output(
        BTreeMap::from([(payer, state.get_account_by_id(payer).nonce)]),
        output,
    );
    message.fee = Some(FeeDeclaration::new(payer, 10_000_000, 0, 100_000_000));
    let witness_set = PrivateWitnessSet::for_message(&message, proof, &[key]);
    PrivacyPreservingTransaction::new(message, witness_set)
}
fn verify_real(tx: &PrivacyPreservingTransaction) {
    let inner: InnerReceipt =
        borsh::from_slice(&tx.witness_set.proof().clone().into_inner()).unwrap();
    assert!(
        matches!(inner, InnerReceipt::Succinct(_)),
        "only real succinct receipts; Fake is forbidden"
    );
    Receipt::new(
        inner,
        lee_core::to_borsh_frame(&(&tx.message.public_context, &tx.message.execution)),
    )
    .verify(lee::PRIVACY_PRESERVING_CIRCUIT_ID)
    .expect("real native PPE receipt verifies");
}
fn input(
    root: Actor,
    message: Message,
    public: PublicExecutionContext,
    witness: PrivateWitness,
) -> ProvingInput {
    ProvingInput {
        root: TransactionEntry::Sync(RootCall {
            to: root,
            message: borsh::to_vec(&message).unwrap(),
        }),
        claims: vec![],
        fee: None,
        public_context: public,
        private_witnesses: vec![witness],
        dummy_inputs: vec![],
        ciphertext_padding: None,
        recoveries: vec![],
        private_promotions: BTreeSet::new(),
    }
}
fn prove(
    name: &str,
    input: ProvingInput,
    simulation: &Simulation,
    catalog: &ProgramCatalog,
    key: &PrivateKey,
    state: &V03State,
) -> PrivacyPreservingTransaction {
    let start = Instant::now();
    if std::env::var_os("REUSE_PROOFS").is_some() {
        if let Ok(bytes) = std::fs::read(format!("evidence/{name}.borsh")) {
            let old: PrivacyPreservingTransaction = borsh::from_slice(&bytes).unwrap();
            verify_real(&old);
            let payer = AccountId::from(&PublicKey::new_from_private_key(key));
            if old.message.nonces.get(&payer) == Some(&state.get_account_by_id(payer).nonce) {
                old.check_stateless().unwrap();
                println!(
                    "PASS reverified cached real native PPE {name} (preserved signed transaction)"
                );
                return old;
            }
            let proof = old.witness_set.proof().clone();
            let output = lee_core::PrivacyPreservingCircuitOutput {
                public_context: old.message.public_context,
                execution: old.message.execution,
            };
            let tx = signed(output, proof, key, state);
            verify_real(&tx);
            println!("PASS reverified cached real native PPE {name}");
            return tx;
        }
    }
    println!("Proving real native PPE {name}; dev mode is forbidden");
    let (out, proof) = lee::execute_and_prove(
        input,
        simulation,
        catalog,
        |_| SenderPresentation::Canonical,
        |_, _| true,
        |messages| {
            assert!(messages.is_empty());
            Ok(vec![])
        },
    )
    .expect("real PPE proving");
    let tx = signed(out, proof, key, state);
    verify_real(&tx);
    std::fs::write(
        format!("evidence/{name}.borsh"),
        borsh::to_vec(&tx).unwrap(),
    )
    .unwrap();
    println!(
        "PASS real native PPE {name}: proof_bytes={} elapsed_ms={}",
        tx.witness_set.proof().size(),
        start.elapsed().as_millis()
    );
    tx
}
fn main() {
    assert!(
        std::env::var("RISC0_DEV_MODE").map_or(true, |v| v == "0"),
        "dev mode forbidden"
    );
    std::fs::create_dir_all("evidence").unwrap();
    let start = Instant::now();
    std::fs::write(
        "evidence/burn-program.bin",
        reverse_methods::REVERSE_GUESTS_ELF,
    )
    .unwrap();
    std::fs::write(
        "evidence/native-ppe-program.bin",
        lee::PRIVACY_PRESERVING_CIRCUIT_ELF,
    )
    .unwrap();
    let program =
        lee::program::Program::new(Cow::Borrowed(reverse_methods::REVERSE_GUESTS_ELF)).unwrap();
    let key = PrivateKey::try_new([1; 32]).unwrap();
    let owner = AccountId::from(&PublicKey::new_from_private_key(&key));
    let owner_actor = Actor::new(owner, TOKEN);
    let holding = Holding {
        balance: 100,
        exits: vec![],
    };
    let mut state = lez_blocks::seeded_state(
        TOKEN,
        program.clone(),
        sink().account_id,
        None,
        owner,
        borsh::to_vec(&holding).unwrap(),
    );
    let initial = state_digest(&state);
    let mut tip = None;
    let genesis = build_block(&state, None, vec![]);
    accept(&mut state, &mut tip, &genesis).unwrap();
    println!(
        "PASS synthetic genesis executed native fee+clock; full_state_digest={}",
        hex::encode(state_digest(&state))
    );
    let catalog: ProgramCatalog = [(TOKEN, program)].into();
    let ask = AuthorizationSecretKey([0x12; 32]);
    let nsk = (&ask).into();
    let npk = NullifierPublicKey::from(&nsk);
    let vpk = ViewingPublicKey::from_seed(&[0x13; 32], &[0x14; 32]);
    let private = AccountId::for_regular_private_account(&npk, &vpk);
    let private_actor = Actor::new(private, TOKEN);
    let witness = PrivateWitness {
        vpk: vpk.clone(),
        random_seed: [0x15; 32],
        kind: WitnessKind::Regular(RegularKey::Authorized(ask)),
        nullifier: NullifierWitness::Init {
            commitment_root: state.commitment_root(),
        },
        address_openings: BTreeSet::new(),
    };
    let shield = prove(
        "shield",
        input(
            owner_actor,
            Message::Transfer {
                to: private,
                amount: 100,
            },
            PublicExecutionContext::new([owner_actor], [owner]),
            witness,
        ),
        &Simulation {
            public_actor_states: [(owner_actor, borsh::to_vec(&holding).unwrap().into())].into(),
            admitted_accounts: None,
        },
        &catalog,
        &key,
        &state,
    );
    let block = build_block(
        &state,
        tip.as_ref(),
        vec![LeeTransaction::PrivacyPreserving(shield.clone())],
    );
    accept(&mut state, &mut tip, &block).unwrap();
    assert!(block.body.receipts[0].success);
    let mut private_account =
        Account::default().with_actor_state(TOKEN, borsh::to_vec(&holding).unwrap().into());
    private_account.nonce = Nonce(0).private_account_nonce_increment(&nsk);
    let membership = state
        .get_proof_for_commitment(&Commitment::new(&private, &private_account))
        .expect("shielded account really accepted");
    println!("PASS private funding accepted by native apply_block and membership witness obtained");
    let pre_burn_state = state.clone();
    let pre_burn_tip = tip.clone();
    let make_burn_input = |account: Account, membership, seed: u8, binding: [u8; 32], amount| {
        input(
            private_actor,
            Message::Burn { amount, binding },
            PublicExecutionContext::new([sink()], [owner]),
            PrivateWitness {
                vpk: vpk.clone(),
                random_seed: [seed; 32],
                kind: WitnessKind::Regular(RegularKey::Authorized(ask)),
                nullifier: NullifierWitness::Update {
                    account,
                    membership_proof: membership,
                },
                address_openings: BTreeSet::new(),
            },
        )
    };
    let empty_sim = Simulation::default();
    if std::env::var_os("NATIVE_REDEMPTION_FIXTURE").is_some() {
        let (pairing, payout, secret) = redemption_fixture::note();
        assert_eq!(TOKEN.as_ref(), &pairing.bridge_program);
        let binding = redemption_core::burn_binding(pairing.id(), &payout, secret).unwrap();
        let leaf = redemption_core::burn_leaf(pairing.bridge_program, payout.amount, binding);
        assert_eq!(leaf, burn_core::leaf(payout.amount, binding));
        let tx = prove(
            "redemption-native-burn",
            make_burn_input(private_account, membership, 0x19, binding, payout.amount),
            &empty_sim,
            &catalog,
            &key,
            &state,
        );
        let block = build_block(
            &state,
            tip.as_ref(),
            vec![LeeTransaction::PrivacyPreserving(tx)],
        );
        accept(&mut state, &mut tip, &block).unwrap();
        assert!(block.body.receipts[0].success);
        let exits = burn_core::read_state(
            state
                .get_account_by_id(sink().account_id)
                .data
                .actor_state(TOKEN),
        )
        .exits;
        assert_eq!(exits, vec![leaf]);
        let mut tree = redemption_core::ExitTree::default();
        for exit in &exits {
            tree.append(*exit).unwrap();
        }
        let snapshot = redemption_fixture::AppliedExitSnapshot {
            block_hash: block.header.hash.0,
            state_digest: state_digest(&state),
            leaves: exits,
            root: tree.root(),
        };
        std::fs::write(
            "evidence/redemption-native-checkpoint.borsh",
            borsh::to_vec(&snapshot).unwrap(),
        )
        .unwrap();
        std::fs::write(
            "evidence/redemption-native-block.borsh",
            borsh::to_vec(&block).unwrap(),
        )
        .unwrap();
        println!("PASS real burn accepted by native LEZ; exit root exported from applied state");
        return;
    }
    let burn = prove(
        "burn",
        make_burn_input(
            private_account.clone(),
            membership.clone(),
            0x16,
            [0x61; 32],
            40,
        ),
        &empty_sim,
        &catalog,
        &key,
        &state,
    );
    std::fs::write(
        "evidence/pinned-images.rs",
        format!(
            "pub const TOKEN_IMAGE: [u32;8] = {:?};\npub const PPE_IMAGE: [u32;8] = {:?};\n",
            reverse_methods::REVERSE_GUESTS_ID,
            lee::PRIVACY_PRESERVING_CIRCUIT_ID
        ),
    )
    .unwrap();
    let tx_wire: acceptance_core::Tx = borsh::from_slice(&borsh::to_vec(&burn).unwrap()).unwrap();
    assert_eq!(
        borsh::to_vec(&tx_wire).unwrap(),
        borsh::to_vec(&burn).unwrap()
    );
    let fragment = acceptance_core::Input {
        before: borsh::to_vec(&state).unwrap(),
        tx: tx_wire,
        token_image: reverse_methods::REVERSE_GUESTS_ID,
        block: tip.as_ref().unwrap().block_id + 1,
        timestamp: (tip.as_ref().unwrap().block_id + 1) * 10,
    };
    let (mirrored, journal) = acceptance_core::apply(fragment.clone());
    let mut native_action = state.clone();
    let diff = lee::ValidatedStateDiff::from_privacy_preserving_transaction(
        &burn,
        &native_action,
        fragment.block,
        fragment.timestamp,
    )
    .unwrap();
    native_action.apply_state_diff(diff).unwrap();
    assert_eq!(
        borsh::to_vec(&mirrored).unwrap(),
        borsh::to_vec(&native_action).unwrap(),
        "differential: complete native state bytes"
    );
    assert_eq!(journal.after, state_digest(&native_action));
    assert_eq!(journal.tx_hash, burn.hash());
    std::fs::write(
        "evidence/acceptance-input.borsh",
        borsh::to_vec(&fragment).unwrap(),
    )
    .unwrap();
    std::fs::write(
        "evidence/acceptance-journal.borsh",
        borsh::to_vec(&journal).unwrap(),
    )
    .unwrap();
    println!(
        "PASS restricted acceptance reference matches native validator + apply_state_diff byte-for-byte over full V03State"
    );
    let burn_block = build_block(
        &state,
        tip.as_ref(),
        vec![LeeTransaction::PrivacyPreserving(burn.clone())],
    );
    let events = accept(&mut state, &mut tip, &burn_block).unwrap();
    assert!(burn_block.body.receipts[0].success);
    assert_eq!(
        burn_core::read_state(
            state
                .get_account_by_id(sink().account_id)
                .data
                .actor_state(TOKEN)
        )
        .exits,
        vec![burn_core::leaf(40, [0x61; 32])]
    );
    println!(
        "PASS burn accepted by native apply_block; exit commitment present; emitted_event_groups={}",
        events.len()
    );
    std::fs::write(
        "evidence/accepted-burn-block.borsh",
        borsh::to_vec(&burn_block).unwrap(),
    )
    .unwrap();
    if std::env::var_os("EXPORT_ACTION_ONLY").is_some() {
        return;
    }
    let conflict = prove(
        "conflicting-burn",
        make_burn_input(private_account.clone(), membership, 0x17, [0x62; 32], 30),
        &empty_sim,
        &catalog,
        &key,
        &pre_burn_state,
    );
    // Resign against the current payer nonce: rejection must reach private double-spend checks.
    let mut conflict = conflict;
    conflict
        .message
        .nonces
        .insert(owner, state.get_account_by_id(owner).nonce);
    conflict.witness_set = PrivateWitnessSet::for_message(
        &conflict.message,
        conflict.witness_set.proof().clone(),
        &[&key],
    );
    verify_real(&conflict);
    let before = state_digest(&state);
    let mut invalid_block = burn_block.clone();
    invalid_block.header.block_id = tip.as_ref().unwrap().block_id + 1;
    invalid_block.header.prev_block_hash = tip.as_ref().unwrap().hash;
    invalid_block.header.timestamp = invalid_block.header.block_id * 10;
    invalid_block.body.transactions[0] = LeeTransaction::PrivacyPreserving(conflict);
    *invalid_block.body.transactions.last_mut().unwrap() =
        LeeTransaction::Public(clock_invocation(
            invalid_block.header.block_id,
            invalid_block.header.timestamp,
        ));
    invalid_block.header.hash = invalid_block.recompute_hash();
    let rejected = accept(&mut state, &mut tip, &invalid_block).unwrap_err();
    assert!(
        rejected.to_string().contains("Nullifier already seen"),
        "must reach the native spend check: {rejected}"
    );
    assert_eq!(before, state_digest(&state));
    std::fs::write(
        "evidence/rejected-conflict-block.borsh",
        borsh::to_vec(&invalid_block).unwrap(),
    )
    .unwrap();
    println!(
        "PASS conflicting genuine proof rejected by native apply_block after fresh payer signature, atomically: {rejected}"
    );
    // A valid proof generated against an empty simulated sink meets the now-full live sink.
    private_account.data.set_actor_state(
        TOKEN,
        borsh::to_vec(&Holding {
            balance: 60,
            exits: vec![],
        })
        .unwrap()
        .into(),
    );
    private_account.nonce = private_account.nonce.private_account_nonce_increment(&nsk);
    let membership = state
        .get_proof_for_commitment(&Commitment::new(&private, &private_account))
        .unwrap();
    let stale = prove(
        "stale-public-simulation",
        make_burn_input(private_account, membership, 0x18, [0x63; 32], 20),
        &empty_sim,
        &catalog,
        &key,
        &state,
    );
    let reverted_block = build_block(
        &state,
        tip.as_ref(),
        vec![LeeTransaction::PrivacyPreserving(stale)],
    );
    assert!(!reverted_block.body.receipts[0].success);
    let mut forged = reverted_block.clone();
    forged.bedrock_status = common::block::BedrockStatus::Finalized;
    forged.body.receipts[0].success = true;
    forged.header.hash = forged.recompute_hash();
    let old_digest = state_digest(&state);
    let error = accept(&mut state, &mut tip, &forged).unwrap_err();
    assert_eq!(old_digest, state_digest(&state));
    println!(
        "PASS forged success receipt with self-consistent block hash rejected atomically: {error}"
    );
    let previous_exit = burn_core::read_state(
        state
            .get_account_by_id(sink().account_id)
            .data
            .actor_state(TOKEN),
    )
    .exits;
    accept(&mut state, &mut tip, &reverted_block).unwrap();
    let exits = burn_core::read_state(
        state
            .get_account_by_id(sink().account_id)
            .data
            .actor_state(TOKEN),
    )
    .exits;
    assert_eq!(previous_exit, exits);
    println!("PASS valid execution proof + reverted settlement creates no additional exit");
    let mut changed_status = burn_block.clone();
    changed_status.bedrock_status = common::block::BedrockStatus::Finalized;
    assert_eq!(changed_status.recompute_hash(), burn_block.header.hash);
    let mut bad = burn_block.clone();
    bad.body.receipts[0].success = false;
    assert_ne!(bad.recompute_hash(), burn_block.header.hash);
    let mut scratch = pre_burn_state.clone();
    let mut scratch_tip = pre_burn_tip.clone();
    assert!(accept(&mut scratch, &mut scratch_tip, &bad).is_err());
    println!(
        "PASS receipts are hash-bound; BedrockStatus::Finalized is unbound and supplies no finality proof"
    );
    let result = serde_json::json!({"synthetic_genesis":true,"initial_full_state_digest":hex::encode(initial),"post_full_state_digest":hex::encode(state_digest(&state)),"accepted_exit_count":exits.len(),"exit_root":hex::encode(burn_core::exit_root(&exits)),"native_ppe_image_id":hex::encode(risc0_zkvm::sha::Digest::from(lee::PRIVACY_PRESERVING_CIRCUIT_ID)),"elapsed_ms":start.elapsed().as_millis(),"native_block_settlement":true,"cryptographic_ppe_proofs":true,"source_finality_verified":false,"release":false});
    std::fs::write(
        "evidence/settlement.json",
        serde_json::to_vec_pretty(&result).unwrap(),
    )
    .unwrap();
    println!("{result}");
    println!("PASS release gate CLOSED: source history authentication/finality proof unavailable");
}
