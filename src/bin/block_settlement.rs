#[path = "../../redemption/fixture.rs"]
mod fixture;
#[path = "../lez_blocks.rs"]
mod lez_blocks;

use acceptance_core::State;
use common::{
    block::{Block, HashableBlockData},
    transaction::LeeTransaction,
};
use lee::{FeeDeclaration, PrivateKey, PublicKey, PublicTransaction, public_transaction};
use lee_core::{
    account::{AccountId, Actor, Nonce},
    native_token,
    program::PROGRAM_LOADER_ACCOUNT_ID,
};
use program_loader_core::{LoaderActorState, ProgramSegment};
use redemption_actor::{ExitState, TOKEN, read_exit_state, sink};
use reverse_settlement_prototype::block_settlement::{
    CLOCK_IMAGE, FEE_IMAGE, Input, Journal, account_digest, apply, snapshot_digest,
};
use risc0_zkvm::{
    ExecutorEnv, InnerReceipt, ProverOpts, Receipt, default_executor, default_prover,
};
use settlement_methods::{BLOCK_SETTLEMENT_GUEST_ELF, BLOCK_SETTLEMENT_GUEST_ID};
use std::{borrow::Cow, collections::BTreeMap, fs, path::PathBuf, time::Instant};

const GAS_LIMIT: u64 = 10_000_000;
const PRODUCER: AccountId = AccountId::new([0x79; 32]);

fn sign(
    key: &PrivateKey,
    sender: AccountId,
    nonce: Nonce,
    to: Actor,
    actors: [Actor; 2],
    call: impl borsh::BorshSerialize,
) -> LeeTransaction {
    let message = public_transaction::Message::try_new_with_fees(
        to,
        actors,
        BTreeMap::from([(sender, nonce)]),
        call,
        FeeDeclaration::new(sender, GAS_LIMIT, 17, 1_000_000_000),
    )
    .unwrap();
    // A stable local fixture lets a separate invocation reproduce the proof input.
    let signature = k256::schnorr::SigningKey::from_bytes(key.value())
        .unwrap()
        .sign_prehash_with_aux_rand(&message.hash(), &[0; 32])
        .unwrap();
    let witnesses = public_transaction::WitnessSet::from_raw_parts(vec![(
        lee::Signature {
            value: signature.to_bytes(),
        },
        PublicKey::new_from_private_key(key),
    )]);
    LeeTransaction::Public(PublicTransaction::new(message, witnesses))
}

fn rehash(block: Block) -> Vec<u8> {
    borsh::to_vec(
        &HashableBlockData {
            block_id: block.header.block_id,
            prev_block_hash: block.header.prev_block_hash,
            timestamp: block.header.timestamp,
            transactions: block.body.transactions,
            receipts: block.body.receipts,
        }
        .into_pending_block(),
    )
    .unwrap()
}

fn native_fixture() -> (Input, Journal) {
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
    let mut visible: State = borsh::from_slice(&borsh::to_vec(&state).unwrap()).unwrap();
    let expected_before = snapshot_digest(&visible, &BTreeMap::new());
    let mut concealed = BTreeMap::new();
    visible.accounts.retain(|id, account| {
        if ProgramSegment::from_loader_actor_state(
            account.data.actor_state(PROGRAM_LOADER_ACCOUNT_ID),
        )
        .is_some()
        {
            concealed.insert(*id, account.clone());
            false
        } else {
            true
        }
    });
    assert!(!concealed.is_empty());
    let input = Input {
        before: borsh::to_vec(&visible).unwrap(),
        concealed_accounts: concealed
            .iter()
            .map(|(id, account)| (*id, account_digest(account)))
            .collect(),
        block: borsh::to_vec(&block).unwrap(),
    };
    lez_blocks::accept(&mut state, &mut tip, &block).unwrap();
    let (after, journal) = apply(&input);
    let mut after: State = borsh::from_slice(&after).unwrap();
    for (id, account) in concealed {
        assert!(after.accounts.insert(id, account).is_none());
    }
    assert!(
        borsh::to_vec(&after).unwrap() == borsh::to_vec(&state).unwrap(),
        "complete native state differs"
    );
    assert_eq!(journal.before, expected_before);
    assert_eq!(journal.after, snapshot_digest(&after, &BTreeMap::new()));
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
    (input, journal)
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
    let mut args = std::env::args_os().skip(1);
    let mode = args
        .next()
        .expect("use check, payload, execute, prove or verify");
    let output = args.next().map(PathBuf::from);
    assert!(args.next().is_none());
    let (input, expected) = native_fixture();
    let executor_env = || {
        ExecutorEnv::builder()
            .write_slice(&lee_core::to_frame(&borsh::to_vec(&input).unwrap()))
            .build()
            .unwrap()
    };
    let verify = |receipt: &Receipt| {
        assert!(!matches!(receipt.inner, InnerReceipt::Fake(_)));
        receipt.verify(BLOCK_SETTLEMENT_GUEST_ID).unwrap();
        let journal: Journal = borsh::from_slice(&receipt.journal.bytes).unwrap();
        assert_eq!(journal, expected);
        let mut edited = receipt.clone();
        edited.journal.bytes[0] ^= 1;
        assert!(edited.verify(BLOCK_SETTLEMENT_GUEST_ID).is_err());
    };
    match mode.to_str() {
        Some("check") if output.is_none() => {}
        Some("payload") => {
            fs::write(output.expect("provide a payload path"), &input.block).unwrap()
        }
        Some("execute") if output.is_none() => {
            let info = default_executor()
                .execute(executor_env(), BLOCK_SETTLEMENT_GUEST_ELF)
                .unwrap();
            assert_eq!(
                borsh::from_slice::<Journal>(&info.journal.bytes).unwrap(),
                expected
            );
            println!("PASS guest execution user_cycles={}", info.cycles());
        }
        Some("prove") => {
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
            verify(&info.receipt);
            let bytes = borsh::to_vec(&info.receipt).unwrap();
            fs::write(path, &bytes).unwrap();
            println!(
                "PASS settlement proof cycles={} receipt_bytes={} elapsed_ms={}",
                info.stats.total_cycles,
                bytes.len(),
                start.elapsed().as_millis()
            );
        }
        Some("verify") => {
            let path = output.expect("provide a receipt path");
            verify(&borsh::from_slice(&fs::read(path).unwrap()).unwrap());
            println!("PASS settlement receipt reverified");
        }
        _ => panic!(
            "usage: block-settlement check | payload OUTPUT | execute | prove OUTPUT | verify RECEIPT"
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
