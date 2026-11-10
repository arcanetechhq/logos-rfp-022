//! Preserve the already signed action transaction used by an in-flight checkpoint proof.
//! Useful when replaying receipts: Schnorr auxiliary randomness changes transaction hashes.
fn main() {
    let input: acceptance_core::Input =
        borsh::from_slice(&std::fs::read("evidence/acceptance-input.borsh").unwrap()).unwrap();
    let bytes = borsh::to_vec(&input.tx).unwrap();
    let tx: lee::PrivacyPreservingTransaction = borsh::from_slice(&bytes).unwrap();
    let inner: risc0_zkvm::InnerReceipt =
        borsh::from_slice(&tx.witness_set.proof().clone().into_inner()).unwrap();
    assert!(matches!(inner, risc0_zkvm::InnerReceipt::Succinct(_)));
    risc0_zkvm::Receipt::new(
        inner,
        lee_core::to_borsh_frame(&(&tx.message.public_context, &tx.message.execution)),
    )
    .verify(lee::PRIVACY_PRESERVING_CIRCUIT_ID)
    .unwrap();
    tx.check_stateless().unwrap();
    std::fs::write("evidence/burn.borsh", bytes).unwrap();
    println!(
        "Preserved verified native transaction {} used by checkpoint input",
        hex::encode(tx.hash())
    );
}
