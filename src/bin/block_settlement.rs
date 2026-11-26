#[path = "../../redemption/fixture.rs"]
mod fixture;
#[path = "../fixture_checkpoint.rs"]
mod fixture_checkpoint;
#[path = "../lez_blocks.rs"]
mod lez_blocks;
#[path = "../settlement_fixture.rs"]
mod settlement_fixture;
use settlement_fixture::{check_native_block, rehash, sign};

use fixture_checkpoint::composition::{
    ARCHIVED_SETTLEMENT_IMAGE, ChainedJournal, SETTLEMENT_IMAGE, join,
};
use lee::{PrivateKey, PublicKey};
use lee_core::{
    account::{AccountId, Actor},
    native_token,
};
use redemption_actor::{ExitState, TOKEN, read_exit_state, sink};
use reverse_settlement_prototype::block_settlement::{
    CLOCK_IMAGE, FEE_IMAGE, Input, Journal, apply,
};
use risc0_zkvm::{
    ExecutorEnv, InnerReceipt, ProverOpts, Receipt, default_executor, default_prover,
};
use settlement_methods::{
    BLOCK_SETTLEMENT_GUEST_ELF, BLOCK_SETTLEMENT_GUEST_ID, SETTLEMENT_CHAIN_GUEST_ELF,
    SETTLEMENT_CHAIN_GUEST_ID,
};
use std::{borrow::Cow, fs, path::PathBuf, time::Instant};

const ARCHIVED_CHAIN_IMAGE: [u32; 8] = [
    1373997337, 3472372327, 4173525209, 3298753985, 2558222010, 2884747113, 1079465393, 1759553085,
];

const GAS_LIMIT: u64 = 10_000_000;
const PRODUCER: AccountId = AccountId::new([0x79; 32]);

fn native_fixture() -> [(Input, Journal); 2] {
    assert_eq!(FEE_IMAGE, programs::fee().id());
    assert_eq!(CLOCK_IMAGE, programs::clock().id());
    let key = PrivateKey::try_new([0x0a; 32]).unwrap();
    let sender = AccountId::from(&PublicKey::new_from_private_key(&key));
    let owner = Actor::new(sender, TOKEN);
    let program = lee::program::Program::new(Cow::Borrowed(
        redemption_burn_methods::REDEMPTION_BURN_GUEST_ELF,
    ))
    .unwrap();
    let (instance, payout, secret) = fixture::note_for(TOKEN.as_ref().try_into().unwrap());
    let pairing = instance.id();
    let binding = redemption_core::burn_binding(pairing, &payout, secret).unwrap();
    let mut state = lez_blocks::seeded_state(
        TOKEN,
        program,
        sink().account_id,
        Some(
            borsh::to_vec(&ExitState {
                pairing,
                tree: Default::default(),
            })
            .unwrap(),
        ),
        sender,
        borsh::to_vec(&100u128).unwrap(),
    );
    let mut tip = None;
    let genesis = lez_blocks::build_block(&state, tip.as_ref(), vec![]);
    lez_blocks::accept(&mut state, &mut tip, &genesis).unwrap();
    let mut expected_tree = redemption_core::ExitTree::default();
    for (amount, binding) in [(payout.amount, binding), (30, [0x11; 32])] {
        let burn = sign(
            &key,
            sender,
            state.get_account_by_id(sender).nonce,
            owner,
            [owner, sink()],
            redemption_actor::Message::Burn {
                amount,
                pairing,
                binding,
            },
            GAS_LIMIT,
        );
        let block = lez_blocks::build_block(&state, tip.as_ref(), vec![burn]);
        lez_blocks::accept(&mut state, &mut tip, &block).unwrap();
        assert!(block.body.receipts[0].success);
        expected_tree
            .append(redemption_core::burn_leaf(
                TOKEN.as_ref().try_into().unwrap(),
                amount,
                binding,
            ))
            .unwrap();
    }
    let predecessor = tip.as_ref().unwrap().hash.0;
    let exit = read_exit_state(
        state
            .get_account_by_id(sink().account_id)
            .data
            .actor_state(TOKEN),
    );
    assert_eq!(exit.tree, expected_tree);
    assert_eq!(
        exit.tree.root(),
        include_bytes!("../../evidence/redemption-native-v2-journal.borsh")[32..64]
    );

    let mut nonce = state.get_account_by_id(sender).nonce;
    let sender_actor = Actor::native_balance(sender);
    let actors = [sender_actor, Actor::native_balance(PRODUCER)];
    let txs = [3_000, u128::MAX / 2].map(|amount| {
        let tx = sign(
            &key,
            sender,
            nonce,
            sender_actor,
            actors,
            native_token::Message::Transfer {
                to: PRODUCER,
                amount,
            },
            GAS_LIMIT,
        );
        nonce.public_account_nonce_increment();
        tx
    });
    let block = lez_blocks::build_block(&state, tip.as_ref(), txs.to_vec());
    let [success, reverted] = block.body.receipts.as_slice() else {
        panic!("expected two receipts")
    };
    assert!(success.success && !reverted.success);
    assert_eq!(success.fee.gas_exec, 0);
    assert_eq!(reverted.fee.gas_exec, GAS_LIMIT);
    assert!(success.fee.refund > 0);
    assert_eq!(reverted.fee.refund, 0);
    let (input, journal) =
        check_native_block(&mut state, &mut tip, &block, std::convert::identity, apply);
    assert_eq!(state.get_account_by_id(sender).nonce, nonce);
    assert_eq!(journal.exit_root, exit.tree.root());
    assert_eq!(journal.pairing, pairing);
    assert_eq!(journal.parent_hash, predecessor);
    assert_eq!(journal.block_hash, block.header.hash.0);

    let mut forged = block.clone();
    forged.body.receipts[1].success = true;
    rejects(&input, rehash(forged), "forged successful receipt");
    let mut reordered = block.clone();
    reordered.body.transactions.swap(0, 1);
    rejects(&input, rehash(reordered), "replayed or out-of-order nonce");
    let mut missing_distribute = block;
    missing_distribute.body.transactions.remove(2);
    rejects(
        &input,
        rehash(missing_distribute),
        "missing fee distribution",
    );
    println!(
        "PASS native apply_block parity, success and charged revert, forged receipt, wrong nonce, missing fee tail"
    );
    let followup = lez_blocks::build_block(&state, tip.as_ref(), vec![]);
    let (next_input, next_journal) = check_native_block(
        &mut state,
        &mut tip,
        &followup,
        std::convert::identity,
        apply,
    );
    assert_eq!(next_journal.before, journal.after);
    assert_eq!(next_journal.parent_hash, journal.block_hash);
    assert_eq!(next_journal.exit_root, journal.exit_root);
    [(input, journal), (next_input, next_journal)]
}

fn rejects(input: &Input, block: Vec<u8>, reason: &str) {
    let invalid = Input {
        before: input.before.clone(),
        concealed_accounts: input.concealed_accounts.clone(),
        block,
    };
    assert!(
        std::panic::catch_unwind(|| apply(&invalid)).is_err(),
        "accepted {reason}"
    );
}

fn main() {
    assert_eq!(std::env::var("RISC0_DEV_MODE").as_deref(), Ok("0"));
    assert_eq!(SETTLEMENT_IMAGE, BLOCK_SETTLEMENT_GUEST_ID);
    let mut args = std::env::args_os().skip(1);
    let mode = args.next().expect(
        "use check, payload, execute, prove, verify, chain-execute, chain-prove or bind-fixture",
    );
    let output = args.next().map(PathBuf::from);
    let trailing: Vec<_> = args.map(PathBuf::from).collect();
    let mode = mode.to_str().expect("UTF-8 mode");
    let [(first, first_journal), (next, next_journal)] = native_fixture();
    let (mode, input, expected) = match mode.strip_prefix("next-") {
        Some(mode) => (mode, &next, &next_journal),
        None => (mode, &first, &first_journal),
    };
    let executor_env = || {
        ExecutorEnv::builder()
            .write_slice(&lee_core::to_frame(&borsh::to_vec(input).unwrap()))
            .build()
            .unwrap()
    };
    let verify = |receipt: &Receipt, expected: &Journal| {
        assert!(!matches!(receipt.inner, InnerReceipt::Fake(_)));
        let image = [SETTLEMENT_IMAGE, ARCHIVED_SETTLEMENT_IMAGE]
            .into_iter()
            .find(|image| receipt.verify(*image).is_ok())
            .expect("settlement receipt for an approved image");
        let journal: Journal = borsh::from_slice(&receipt.journal.bytes).unwrap();
        assert_eq!(&journal, expected);
        let mut edited = receipt.clone();
        edited.journal.bytes[0] ^= 1;
        assert!(edited.verify(image).is_err());
        (image, journal)
    };
    if !matches!(mode, "bind-fixture" | "chain-execute" | "chain-prove") {
        assert!(trailing.is_empty());
    }
    match mode {
        "check" if output.is_none() => {}
        "payload" => fs::write(output.expect("provide a payload path"), &input.block).unwrap(),
        "execute" if output.is_none() => {
            let info = default_executor()
                .execute(executor_env(), BLOCK_SETTLEMENT_GUEST_ELF)
                .unwrap();
            assert_eq!(
                borsh::from_slice::<Journal>(&info.journal.bytes).unwrap(),
                *expected
            );
            println!("PASS guest execution user_cycles={}", info.cycles());
        }
        "prove" => {
            let path = output.expect("provide a receipt output path");
            let start = Instant::now();
            println!(
                "Proving settlement for {}",
                hex::encode(expected.block_hash)
            );
            let info = default_prover()
                .prove_with_opts(
                    executor_env(),
                    BLOCK_SETTLEMENT_GUEST_ELF,
                    &ProverOpts::composite(),
                )
                .unwrap();
            verify(&info.receipt, expected);
            let bytes = borsh::to_vec(&info.receipt).unwrap();
            fs::write(path, &bytes).unwrap();
            println!(
                "PASS settlement proof cycles={} receipt_bytes={} elapsed_ms={}",
                info.stats.total_cycles,
                bytes.len(),
                start.elapsed().as_millis()
            );
        }
        "verify" => {
            let path = output.expect("provide a receipt path");
            verify(
                &borsh::from_slice(&fs::read(path).unwrap()).unwrap(),
                expected,
            );
            println!("PASS settlement receipt reverified");
        }
        "chain-execute" | "chain-prove" => {
            let (next_path, chain_path) = match (mode, trailing.as_slice()) {
                ("chain-execute", [next]) => (next, None),
                ("chain-prove", [next, output]) => (next, Some(output)),
                _ => panic!("use chain-execute FIRST NEXT or chain-prove FIRST NEXT OUTPUT"),
            };
            let first_receipt = read_receipt(&output.expect("first settlement receipt"));
            let next_receipt = read_receipt(next_path);
            let (first_image, first) = verify(&first_receipt, &first_journal);
            let (next_image, next) = verify(&next_receipt, &next_journal);
            let expected = join(&first, &next).unwrap();
            assert!(join(&next, &first).is_none());
            let mut unrelated = next.clone();
            unrelated.before[0] ^= 1;
            assert!(join(&first, &unrelated).is_none());
            let env = ExecutorEnv::builder()
                .write_slice(&lee_core::to_frame(
                    &borsh::to_vec(&([first_image, next_image], [first, next])).unwrap(),
                ))
                .add_assumption(first_receipt)
                .add_assumption(next_receipt)
                .build()
                .unwrap();
            if let Some(path) = chain_path {
                let start = Instant::now();
                let info = default_prover()
                    .prove_with_opts(env, SETTLEMENT_CHAIN_GUEST_ELF, &ProverOpts::composite())
                    .unwrap();
                verify_chain(&info.receipt, &expected);
                let bytes = borsh::to_vec(&info.receipt).unwrap();
                fs::write(path, &bytes).unwrap();
                println!(
                    "PASS recursive settlement proof cycles={} receipt_bytes={} elapsed_ms={}",
                    info.stats.total_cycles,
                    bytes.len(),
                    start.elapsed().as_millis()
                );
            } else {
                let info = default_executor()
                    .execute(env, SETTLEMENT_CHAIN_GUEST_ELF)
                    .unwrap();
                assert_eq!(
                    borsh::from_slice::<ChainedJournal>(&info.journal.bytes).unwrap(),
                    expected
                );
                println!(
                    "PASS recursive guest execution user_cycles={}",
                    info.cycles()
                );
            }
        }
        "bind-fixture" => {
            let [replay, checkpoint] = trailing.as_slice() else {
                panic!("usage: block-settlement bind-fixture CHAIN_RECEIPT REPLAY OUTPUT");
            };
            let receipt = read_receipt(&output.expect("chained settlement receipt"));
            let expected = join(&first_journal, &next_journal).unwrap();
            let journal = verify_chain(&receipt, &expected);
            fixture_checkpoint::write(replay, checkpoint, &journal);
        }
        _ => panic!(
            "usage: block-settlement [next-]check | [next-]payload OUTPUT | [next-]execute | [next-]prove OUTPUT | [next-]verify RECEIPT | chain-execute FIRST NEXT | chain-prove FIRST NEXT OUTPUT | bind-fixture CHAIN_RECEIPT REPLAY OUTPUT"
        ),
    }
    println!(
        "block={} before={} after={} payload={} exits={}",
        hex::encode(expected.block_hash),
        hex::encode(expected.before),
        hex::encode(expected.after),
        hex::encode(expected.payload_digest),
        hex::encode(expected.exit_root)
    );
}

fn read_receipt(path: &std::path::Path) -> Receipt {
    borsh::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn verify_chain(receipt: &Receipt, expected: &ChainedJournal) -> ChainedJournal {
    assert!(!matches!(receipt.inner, InnerReceipt::Fake(_)));
    receipt
        .verify(SETTLEMENT_CHAIN_GUEST_ID)
        .or_else(|_| receipt.verify(ARCHIVED_CHAIN_IMAGE))
        .unwrap();
    assert!(receipt.verify(SETTLEMENT_IMAGE).is_err());
    let journal: ChainedJournal = borsh::from_slice(&receipt.journal.bytes).unwrap();
    assert_eq!(&journal, expected);
    journal
}
