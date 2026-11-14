#[path = "../../fixture.rs"]
mod fixture;

use redemption_core::{ExitTree, Journal, Witness, burn_binding, burn_leaf, membership, redeem};
use redemption_methods::{REDEMPTION_GUEST_ELF, REDEMPTION_GUEST_ID};
use risc0_zkvm::{
    ExecutorEnv, InnerReceipt, ProverOpts, Receipt, default_executor, default_prover, sha::Digest,
};

fn main() {
    assert!(std::env::var("RISC0_DEV_MODE").is_ok_and(|mode| mode == "0"));
    let incremental_actor = std::env::var_os("NATIVE_V2_REDEMPTION").is_some();
    let (pairing, payout, secret) = if incremental_actor {
        fixture::note_for([0x44; 32])
    } else {
        fixture::note()
    };
    let leaf = burn_leaf(
        pairing.bridge_program,
        payout.amount,
        burn_binding(pairing.id(), &payout, secret).unwrap(),
    );
    let from_settlement =
        incremental_actor || std::env::var_os("NATIVE_REDEMPTION_FIXTURE").is_some();
    let prefix = match (incremental_actor, from_settlement) {
        (true, _) => "redemption-native-v2",
        (false, true) => "redemption-native",
        (false, false) => "redemption-synthetic",
    };
    let (leaves, position) = if from_settlement {
        let snapshot: fixture::AppliedExitSnapshot = borsh::from_slice(
            &std::fs::read(format!("evidence/{prefix}-checkpoint.borsh")).unwrap(),
        )
        .unwrap();
        let position = snapshot
            .leaves
            .iter()
            .position(|value| value == &leaf)
            .unwrap();
        let position = u32::try_from(position).unwrap();
        assert_eq!(
            membership(&snapshot.leaves, position).unwrap().0,
            snapshot.root
        );
        (snapshot.leaves, position)
    } else {
        (vec![[0x41; 32], leaf, [0x43; 32]], 1)
    };
    let mut tree = ExitTree::default();
    for leaf in &leaves {
        tree.append(*leaf).unwrap();
    }
    let (root, siblings) = membership(&leaves, position).unwrap();
    assert_eq!(tree.root(), root);
    let witness = Witness {
        pairing,
        root,
        payout,
        secret,
        position,
        siblings,
    };
    let expected = redeem(&witness).unwrap();
    let bytes = borsh::to_vec(&witness).unwrap();
    let env = ExecutorEnv::builder().write_slice(&bytes).build().unwrap();
    let start = std::time::Instant::now();
    if std::env::var_os("EXECUTE_ONLY").is_some() {
        let result = default_executor()
            .execute(env, REDEMPTION_GUEST_ELF)
            .unwrap();
        assert_eq!(result.journal.bytes, borsh::to_vec(&expected).unwrap());
        if !from_settlement {
            std::fs::write(
                "evidence/redemption-encoding-vector.borsh",
                &result.journal.bytes,
            )
            .unwrap();
        }
        println!("PASS private redemption guest execution");
        return;
    }
    let proof_path = format!("evidence/{prefix}-receipt.borsh");
    let reuse = std::env::var_os("REUSE_REDEMPTION").is_some();
    let receipt: Receipt = if reuse {
        borsh::from_slice(&std::fs::read(&proof_path).unwrap()).unwrap()
    } else {
        let result = default_prover().prove(env, REDEMPTION_GUEST_ELF).unwrap();
        println!(
            "Generated redemption receipt: prove_ms={} total_cycles={}",
            start.elapsed().as_millis(),
            result.stats.total_cycles,
        );
        result.receipt
    };
    assert!(!matches!(receipt.inner, InnerReceipt::Fake(_)));
    receipt.verify(REDEMPTION_GUEST_ID).unwrap();
    let journal: Journal = borsh::from_slice(&receipt.journal.bytes).unwrap();
    assert_eq!(journal, expected);
    let mut changed = receipt.clone();
    changed.journal.bytes[64] ^= 1;
    assert!(changed.verify(REDEMPTION_GUEST_ID).is_err());
    if !reuse {
        let bytes = borsh::to_vec(&receipt).unwrap();
        println!("PASS real redemption proof: receipt_bytes={}", bytes.len());
        std::fs::write(proof_path, bytes).unwrap();
        std::fs::write(
            format!("evidence/{prefix}-journal.borsh"),
            &receipt.journal.bytes,
        )
        .unwrap();
        std::fs::write("evidence/redemption-program.bin", REDEMPTION_GUEST_ELF).unwrap();
    } else {
        println!("PASS cached redemption receipt cryptographically reverified");
    }

    if std::env::var_os("WRAP_GROTH16").is_some() {
        let wrapped = default_prover()
            .compress(&ProverOpts::groth16(), &receipt)
            .unwrap();
        wrapped.verify(REDEMPTION_GUEST_ID).unwrap();
        assert_eq!(wrapped.journal.bytes, receipt.journal.bytes);
        let InnerReceipt::Groth16(proof) = wrapped.inner else {
            panic!("expected a real Groth16 receipt");
        };
        // The RISC Zero EVM seal prefixes the proof with the verifier parameter selector.
        let mut seal = proof.verifier_parameters.as_bytes()[..4].to_vec();
        seal.extend_from_slice(&proof.seal);
        std::fs::write(format!("evidence/{prefix}-evm-seal.bin"), &seal).unwrap();
        std::fs::write(
            format!("evidence/{prefix}-image-id.bin"),
            Digest::from(REDEMPTION_GUEST_ID).as_bytes(),
        )
        .unwrap();
        println!("PASS wrapped Groth16 receipt: seal_bytes={}", seal.len());
    }
}
