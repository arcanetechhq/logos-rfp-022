#[path = "../release_gate.rs"]
mod release_gate;
use acceptance_core::{Input, Journal};
use risc0_zkvm::{ExecutorEnv, InnerReceipt, Receipt, default_executor, default_prover};
use std::time::Instant;
fn ensure_real(receipt: &Receipt) {
    assert!(
        !matches!(receipt.inner, InnerReceipt::Fake(_)),
        "fake receipt forbidden"
    );
}
fn native_receipt(input: &Input) -> Receipt {
    let inner: InnerReceipt = borsh::from_slice(&input.tx.witness.proof).unwrap();
    let m = &input.tx.message;
    let receipt = Receipt::new(
        inner,
        lee_core::to_borsh_frame(&(&m.public_context, &m.execution)),
    );
    ensure_real(&receipt);
    receipt.verify(lee::PRIVACY_PRESERVING_CIRCUIT_ID).unwrap();
    receipt
}
fn main() {
    assert!(std::env::var("RISC0_DEV_MODE").map_or(true, |v| v == "0"));
    let start = Instant::now();
    let bytes = std::fs::read("evidence/acceptance-input.borsh").unwrap();
    let input: Input = borsh::from_slice(&bytes).unwrap();
    assert_eq!(input.token_image, reverse_methods::REVERSE_GUESTS_ID);
    let expected: Journal =
        borsh::from_slice(&std::fs::read("evidence/acceptance-journal.borsh").unwrap()).unwrap();
    std::fs::write(
        "evidence/checkpoint-program.bin",
        checkpoint_methods::CHECKPOINT_GUEST_ELF,
    )
    .unwrap();
    let cached = std::env::var_os("REUSE_CHECKPOINT").is_some();
    let (receipt, total_cycles, prove_ms) = if cached {
        native_receipt(&input);
        let receipt: Receipt =
            borsh::from_slice(&std::fs::read("evidence/checkpoint-receipt.borsh").unwrap())
                .unwrap();
        println!("Reverifying cached cryptographic checkpoint receipt; no new proving measurement");
        (receipt, None, None)
    } else {
        let env = ExecutorEnv::builder()
            .add_assumption(native_receipt(&input))
            .write_slice(&lee_core::to_frame(&bytes))
            .build()
            .unwrap();
        println!(
            "Proving real checkpoint: {} input bytes, native-framed guest",
            bytes.len()
        );
        let info = default_prover()
            .prove(env, checkpoint_methods::CHECKPOINT_GUEST_ELF)
            .unwrap();
        let elapsed = start.elapsed().as_millis();
        (info.receipt, Some(info.stats.total_cycles), Some(elapsed))
    };
    ensure_real(&receipt);
    let verify_start = Instant::now();
    receipt
        .verify(checkpoint_methods::CHECKPOINT_GUEST_ID)
        .unwrap();
    let verify_ms = verify_start.elapsed().as_millis();
    let journal: Journal = borsh::from_slice(&receipt.journal.bytes).unwrap();
    assert_eq!(journal, expected);
    let block: common::block::Block =
        borsh::from_slice(&std::fs::read("evidence/accepted-burn-block.borsh").unwrap()).unwrap();
    assert_eq!(block.recompute_hash(), block.header.hash);
    assert_eq!(
        (block.header.block_id, block.header.timestamp),
        (journal.block, journal.timestamp)
    );
    assert!(
        block
            .body
            .transactions
            .iter()
            .any(|t| t.hash().0 == journal.tx_hash),
        "checkpoint action must be the exact native transaction in the inscription payload"
    );
    println!(
        "PASS host binding: verified checkpoint action is the exact transaction in the accepted native block/inscription payload"
    );

    let seal_bytes = borsh::to_vec(&receipt).unwrap();
    std::fs::write("evidence/checkpoint-receipt.borsh", &seal_bytes).unwrap();
    println!(
        "PASS real checkpoint receipt: cycles={:?} receipt_bytes={} prove_ms={:?} verify_ms={}",
        total_cycles,
        seal_bytes.len(),
        prove_ms,
        verify_ms
    );
    println!(
        "PASS checkpoint before/after commitments and tx hash match independently executed native LEZ action"
    );
    let mut changed = receipt.clone();
    changed.journal.bytes[0] ^= 1;
    assert!(
        changed
            .verify(checkpoint_methods::CHECKPOINT_GUEST_ID)
            .is_err()
    );
    assert!(receipt.verify([0u32; 8]).is_err());
    println!("PASS modified journal and wrong image ID rejected");
    // The initial digest here comes from our synthetic native replay, never from a source-chain proof.
    // Prover-chosen predecessor roots cannot replace this anchor at the consumer boundary.
    let mut head = release_gate::FixtureActionHead::from_fixture(expected.before);
    head.advance(&journal).unwrap();
    assert!(
        head.advance(&journal).is_err(),
        "the same checkpoint cannot advance the head twice"
    );
    assert!(
        release_gate::FixtureActionHead::from_fixture([0x99; 32])
            .advance(&journal)
            .is_err()
    );
    println!(
        "PASS fixture checkpoint head rejects predecessor substitution and replay after advancing"
    );
    // Negative guest execution: native commitment-root history and spend set are enforced in-guest.
    let mut spent = input.clone();
    let mut s: acceptance_core::State = borsh::from_slice(&spent.before).unwrap();
    s.private
        .1
        .insert(spent.tx.message.execution.nullifiers()[0].0.to_byte_array());
    spent.before = borsh::to_vec(&s).unwrap();
    let env = ExecutorEnv::builder()
        .add_assumption(native_receipt(&spent))
        .write_slice(&lee_core::to_borsh_frame(&spent))
        .build()
        .unwrap();
    assert!(
        default_executor()
            .execute(env, checkpoint_methods::CHECKPOINT_GUEST_ELF)
            .is_err()
    );
    println!("PASS genuine PPE proof with spent nullifier aborts checkpoint guest");
    let mut unknown = input.clone();
    let mut s: acceptance_core::State = borsh::from_slice(&unknown.before).unwrap();
    s.private.0.roots.clear();
    unknown.before = borsh::to_vec(&s).unwrap();
    let env = ExecutorEnv::builder()
        .add_assumption(native_receipt(&unknown))
        .write_slice(&lee_core::to_borsh_frame(&unknown))
        .build()
        .unwrap();
    assert!(
        default_executor()
            .execute(env, checkpoint_methods::CHECKPOINT_GUEST_ELF)
            .is_err()
    );
    println!("PASS genuine PPE proof with unknown commitment root aborts checkpoint guest");
    let gate = release_gate::ReleaseGate::new(checkpoint_methods::CHECKPOINT_GUEST_ID);
    assert_eq!(
        gate.authorize(&receipt),
        Err(release_gate::Rejection::SourceHistoryVerifierUnavailable)
    );
    assert_eq!(
        gate.authorize(&changed),
        Err(release_gate::Rejection::InvalidActionProof)
    );
    println!("PASS RELEASE DENIED: no authenticated canonical Logos history/finality anchor");
    let receipt_kind = match &receipt.inner { InnerReceipt::Composite(_) => "Composite", InnerReceipt::Succinct(_) => "Succinct", InnerReceipt::Groth16(_) => "Groth16", _ => "Other" };
    let out = serde_json::json!({"receipt_kind":receipt_kind,"cached_receipt":cached,"checkpoint_image_id":hex::encode(risc0_zkvm::sha::Digest::from(checkpoint_methods::CHECKPOINT_GUEST_ID)),"cycles":total_cycles,"receipt_bytes":seal_bytes.len(),"prove_ms":prove_ms,"verify_ms":verify_ms,"before":hex::encode(journal.before),"after":hex::encode(journal.after),"exit_root":hex::encode(journal.exit_root),"native_action_differential":true,"host_l2_transaction_binding_checked":true,"full_block_proven":false,"source_finality_proven":false,"ethereum_verification_run":false,"release":false});
    std::fs::write(
        if cached {
            "evidence/checkpoint-verification.json"
        } else {
            "evidence/checkpoint.json"
        },
        serde_json::to_vec_pretty(&out).unwrap(),
    )
    .unwrap();
    println!("{out}");
}
