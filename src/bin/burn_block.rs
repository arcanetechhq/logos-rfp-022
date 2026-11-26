#[path = "../../redemption/fixture.rs"]
mod fixture;
#[path = "../lez_blocks.rs"]
mod lez_blocks;
#[path = "../settlement_fixture.rs"]
mod settlement_fixture;

use acceptance_core::hash;
use burn_block_methods::{BURN_BLOCK_GUEST_ELF, BURN_BLOCK_GUEST_ID};
use burn_meter::Image;
use lee::{PrivateKey, PublicKey, program::Program};
use lee_core::account::{AccountId, Actor};
use redemption_actor::{ExitState, Message, TOKEN, read_balance, read_exit_state, sink};
use redemption_burn_methods::{REDEMPTION_BURN_GUEST_ELF, REDEMPTION_BURN_GUEST_ID};
use redemption_core::{ExitTree, Hash, burn_binding, burn_leaf};
use reverse_settlement_prototype::block_settlement::{
    Journal,
    burn::{ACTOR_IMAGE, Input, apply},
};
use risc0_zkvm::{
    ExecutorEnv, InnerReceipt, ProverOpts, Receipt, default_executor, default_prover,
};
use settlement_fixture::{check_native_block, rehash, sign};
use std::{
    borrow::Cow,
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

const GAS_LIMIT: u64 = 1_000_000;

fn native_fixture(check_rollback: bool) -> (Input, Journal, Hash) {
    assert_eq!(ACTOR_IMAGE, REDEMPTION_BURN_GUEST_ID);
    let key = PrivateKey::try_new([0x0a; 32]).unwrap();
    let owner = AccountId::from(&PublicKey::new_from_private_key(&key));
    let owner_actor = Actor::new(owner, TOKEN);
    let (instance, payout, secret) = fixture::note_for(TOKEN.as_ref().try_into().unwrap());
    let pairing = instance.id();
    let binding = burn_binding(pairing, &payout, secret).unwrap();
    let initial_tree = ExitTree::default();
    let program = Program::new(Cow::Borrowed(REDEMPTION_BURN_GUEST_ELF)).unwrap();
    let mut state = lez_blocks::seeded_state(
        TOKEN,
        program,
        sink().account_id,
        Some(
            borsh::to_vec(&ExitState {
                pairing,
                tree: initial_tree.clone(),
            })
            .unwrap(),
        ),
        owner,
        borsh::to_vec(&100u128).unwrap(),
    );
    let mut tip = None;
    let genesis = lez_blocks::build_block(&state, None, vec![]);
    lez_blocks::accept(&mut state, &mut tip, &genesis).unwrap();
    let mut nonce = state.get_account_by_id(owner).nonce;
    let actor_memory = Image::from_binary(REDEMPTION_BURN_GUEST_ELF)
        .unwrap()
        .encode();
    if check_rollback {
        // The debit handler stages a balance write. The mispaired append then
        // panics in the second delivery, so neither actor write may survive.
        let wrong_pair = sign(
            &key,
            owner,
            nonce,
            owner_actor,
            [owner_actor, sink()],
            Message::Burn {
                amount: payout.amount,
                pairing: [0xa1; 32],
                binding,
            },
            GAS_LIMIT,
        );
        let mut aborted_state = state.clone();
        let mut aborted_tip = tip.clone();
        let blocked =
            lez_blocks::build_block(&aborted_state, aborted_tip.as_ref(), vec![wrong_pair]);
        let receipt = &blocked.body.receipts[0];
        assert!(!receipt.success);
        assert_eq!(receipt.fee.gas_exec, GAS_LIMIT);
        assert_eq!(receipt.fee.refund, 0);
        let (_, aborted) = check_native_block(
            &mut aborted_state,
            &mut aborted_tip,
            &blocked,
            |block| Input {
                block,
                actor_memory: actor_memory.clone(),
            },
            apply,
        );
        assert_eq!(aborted.exit_root, initial_tree.root());
        assert_eq!(
            read_balance(
                aborted_state
                    .get_account_by_id(owner)
                    .data
                    .actor_state(TOKEN)
            ),
            100
        );
        println!("PASS rejected sink append rolls back the staged debit and keeps the fee");
    }
    let transactions = [(payout.amount, binding), (61, [0x61; 32])].map(|(amount, binding)| {
        let tx = sign(
            &key,
            owner,
            nonce,
            owner_actor,
            [owner_actor, sink()],
            Message::Burn {
                amount,
                pairing,
                binding,
            },
            GAS_LIMIT,
        );
        nonce.public_account_nonce_increment();
        tx
    });
    let block = lez_blocks::build_block(&state, tip.as_ref(), transactions.to_vec());
    let [success, reverted] = block.body.receipts.as_slice() else {
        panic!("two burn receipts")
    };
    assert!(success.success && !reverted.success);
    assert!((1..GAS_LIMIT).contains(&success.fee.gas_exec));
    assert_eq!(reverted.fee.gas_exec, GAS_LIMIT);
    assert!(success.fee.refund > 0 && reverted.fee.refund == 0);
    let (input, journal) = check_native_block(
        &mut state,
        &mut tip,
        &block,
        |block| Input {
            block,
            actor_memory,
        },
        apply,
    );
    let leaf = burn_leaf(instance.bridge_program, payout.amount, binding);
    let mut expected_tree = initial_tree.clone();
    expected_tree.append(leaf).unwrap();
    assert_eq!(journal.exit_root, expected_tree.root());
    assert_ne!(journal.exit_root, initial_tree.root());
    assert_eq!(journal.pairing, pairing);
    assert_eq!(journal.parent_hash, genesis.header.hash.0);
    assert_eq!(state.get_account_by_id(owner).nonce, nonce);
    assert_eq!(
        read_balance(state.get_account_by_id(owner).data.actor_state(TOKEN)),
        60
    );
    assert_eq!(
        read_exit_state(
            state
                .get_account_by_id(sink().account_id)
                .data
                .actor_state(TOKEN)
        )
        .tree,
        expected_tree
    );
    println!("PASS complete native apply_block parity for Burn and overspend");
    println!("success={:?} reverted={:?}", success.fee, reverted.fee);
    (input, journal, leaf)
}

fn check_invalid(mut input: Input) {
    let mut forged: common::block::Block = borsh::from_slice(&input.block.block).unwrap();
    forged.body.receipts[0].fee.gas_exec += 1;
    let original = std::mem::replace(&mut input.block.block, rehash(forged));
    assert!(std::panic::catch_unwind(|| apply(&input)).is_err());
    input.block.block = original;
    input.actor_memory[4] ^= 1;
    let rejected = std::panic::catch_unwind(|| apply(&input)).unwrap_err();
    assert!(
        rejected
            .downcast_ref::<String>()
            .unwrap()
            .contains("native burn memory image")
    );
    println!("PASS rejected rehashed false gas receipt and substituted actor memory");
}

fn verify(receipt: &Receipt, expected: &Journal) {
    assert!(!matches!(receipt.inner, InnerReceipt::Fake(_)));
    receipt.verify(BURN_BLOCK_GUEST_ID).unwrap();
    assert_eq!(
        borsh::from_slice::<Journal>(&receipt.journal.bytes).unwrap(),
        *expected
    );
    let mut changed = receipt.clone();
    changed.journal.bytes[160] ^= 1; // pairing
    assert!(changed.verify(BURN_BLOCK_GUEST_ID).is_err());
}

fn snapshot(directory: &Path, journal: &Journal, leaf: Hash) {
    assert_eq!(
        redemption_core::membership(&[leaf], 0).unwrap().0,
        journal.exit_root
    );
    let snapshot = fixture::AppliedExitSnapshot {
        block_hash: journal.block_hash,
        state_digest: journal.after,
        leaves: vec![leaf],
        root: journal.exit_root,
    };
    fs::create_dir_all(directory.join("evidence")).unwrap();
    fs::write(
        directory.join("evidence/redemption-native-v2-checkpoint.borsh"),
        borsh::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
}

fn checkpoint(directory: &Path, journal: &Journal) {
    let hex_bytes = |bytes: Hash| format!("0x{}", hex::encode(bytes));
    let data = serde_json::json!({
        "pairing": hex_bytes(journal.pairing),
        "previous": hex_bytes(hash(&borsh::to_vec(&(b"RFP022/local-burn-parent/v1", journal.parent_hash, journal.before)).unwrap())),
        "next": hex_bytes(hash(&borsh::to_vec(&(b"RFP022/local-burn-checkpoint/v1", journal)).unwrap())),
        "exitRoot": hex_bytes(journal.exit_root),
        "displacedRoot": hex_bytes(ExitTree::default().root()),
        "displacedCheckpoint": hex_bytes(hash(b"RFP022/local-empty-exit-checkpoint/v1")),
        "displacedSlot": 1,
        "slot": journal.height,
    });
    fs::write(
        directory.join("evm-checkpoint.json"),
        serde_json::to_vec_pretty(&data).unwrap(),
    )
    .unwrap();
}

fn main() {
    assert_eq!(std::env::var("RISC0_DEV_MODE").as_deref(), Ok("0"));
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .expect("use check, execute, prove, verify, native-snapshot or fixture");
    let paths: Vec<_> = args.map(PathBuf::from).collect();
    let (input, expected, leaf) = native_fixture(mode == "check");
    let executor_env = || {
        ExecutorEnv::builder()
            .write_slice(&lee_core::to_borsh_frame(&input))
            .build()
            .unwrap()
    };
    match (mode.to_str(), paths.as_slice()) {
        (Some("check"), []) => check_invalid(input),
        (Some("execute"), []) => {
            let result = default_executor()
                .execute(executor_env(), BURN_BLOCK_GUEST_ELF)
                .unwrap();
            assert_eq!(
                borsh::from_slice::<Journal>(&result.journal.bytes).unwrap(),
                expected
            );
            println!(
                "PASS guest Burn block execution user_cycles={}",
                result.cycles()
            );
        }
        (Some("prove"), [path]) => {
            println!("Proving Burn block {}", hex::encode(expected.block_hash));
            let started = Instant::now();
            let proof = default_prover()
                .prove_with_opts(
                    executor_env(),
                    BURN_BLOCK_GUEST_ELF,
                    &ProverOpts::composite(),
                )
                .unwrap();
            verify(&proof.receipt, &expected);
            let bytes = borsh::to_vec(&proof.receipt).unwrap();
            fs::write(path, &bytes).unwrap();
            println!(
                "PASS Burn block proof total_cycles={} receipt_bytes={} elapsed_ms={}",
                proof.stats.total_cycles,
                bytes.len(),
                started.elapsed().as_millis()
            );
        }
        (Some("verify"), [path]) => {
            let receipt = borsh::from_slice(&fs::read(path).unwrap()).unwrap();
            verify(&receipt, &expected);
            println!("PASS independently verified Burn block receipt");
        }
        (Some("native-snapshot"), [directory]) => snapshot(directory, &expected, leaf),
        (Some("fixture"), [path, directory]) => {
            let receipt: Receipt = borsh::from_slice(&fs::read(path).unwrap()).unwrap();
            verify(&receipt, &expected);
            let journal = borsh::from_slice(&receipt.journal.bytes).unwrap();
            snapshot(directory, &journal, leaf);
            checkpoint(directory, &journal);
            println!("PASS EVM fixture root read from the verified Burn block journal");
        }
        _ => panic!(
            "usage: burn-block check | execute | prove RECEIPT | verify RECEIPT | native-snapshot DIR | fixture RECEIPT DIR"
        ),
    }
    println!(
        "block={} after={} exit_root={}",
        hex::encode(expected.block_hash),
        hex::encode(expected.after),
        hex::encode(expected.exit_root)
    );
}
