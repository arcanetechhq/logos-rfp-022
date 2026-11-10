use risc0_zkvm::{ExecutorEnv, InnerReceipt, Receipt, default_executor};
fn main() {
    let bytes = std::fs::read("evidence/acceptance-input.borsh").unwrap();
    let input: acceptance_core::Input = borsh::from_slice(&bytes).unwrap();
    let inner: InnerReceipt = borsh::from_slice(&input.tx.witness.proof).unwrap();
    assert!(matches!(inner, InnerReceipt::Succinct(_)));
    let receipt = Receipt::new(
        inner,
        lee_core::to_borsh_frame(&(
            &input.tx.message.public_context,
            &input.tx.message.execution,
        )),
    );
    receipt.verify(lee::PRIVACY_PRESERVING_CIRCUIT_ID).unwrap();
    let start = std::time::Instant::now();
    let env = ExecutorEnv::builder()
        .add_assumption(receipt)
        .write_slice(&lee_core::to_frame(&bytes))
        .build()
        .unwrap();
    let info = default_executor()
        .execute(env, checkpoint_methods::CHECKPOINT_GUEST_ELF)
        .unwrap();
    println!(
        "actual_checkpoint_user_cycles={} segments={} execution_ms={} input_bytes={}",
        info.cycles(),
        info.segments.len(),
        start.elapsed().as_millis(),
        bytes.len()
    );
}
