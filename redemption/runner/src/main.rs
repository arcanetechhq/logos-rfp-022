#[path = "../../fixture.rs"]
mod fixture;

use redemption_core::{ExitTree, Journal, Witness, burn_binding, burn_leaf, membership, redeem};
use redemption_methods::{REDEMPTION_GUEST_ELF, REDEMPTION_GUEST_ID};
use risc0_zkvm::{ExecutorEnv, InnerReceipt, default_executor, default_prover};

fn main() {
    assert!(std::env::var("RISC0_DEV_MODE").is_ok_and(|mode| mode == "0"));
    let (pairing, payout, secret) = fixture::note();
    let leaf = burn_leaf(
        pairing.bridge_program,
        payout.amount,
        burn_binding(pairing.id(), &payout, secret).unwrap(),
    );
    let from_settlement = std::env::var_os("NATIVE_REDEMPTION_FIXTURE").is_some();
    let (leaves, position) = if from_settlement {
        let snapshot: fixture::AppliedExitSnapshot = borsh::from_slice(
            &std::fs::read("evidence/redemption-native-checkpoint.borsh").unwrap(),
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
    let result = default_prover().prove(env, REDEMPTION_GUEST_ELF).unwrap();
    let proof_ms = start.elapsed().as_millis();
    assert!(!matches!(result.receipt.inner, InnerReceipt::Fake(_)));
    result.receipt.verify(REDEMPTION_GUEST_ID).unwrap();
    let journal: Journal = borsh::from_slice(&result.receipt.journal.bytes).unwrap();
    assert_eq!(journal, expected);
    let mut changed = result.receipt.clone();
    changed.journal.bytes[64] ^= 1;
    assert!(changed.verify(REDEMPTION_GUEST_ID).is_err());
    let bytes = borsh::to_vec(&result.receipt).unwrap();
    let prefix = if from_settlement {
        "redemption-native"
    } else {
        "redemption-synthetic"
    };
    println!(
        "PASS real redemption proof: prove_ms={proof_ms} total_cycles={} receipt_bytes={}",
        result.stats.total_cycles,
        bytes.len()
    );
    std::fs::write(format!("evidence/{prefix}-receipt.borsh"), bytes).unwrap();
    std::fs::write(
        format!("evidence/{prefix}-journal.borsh"),
        &result.receipt.journal.bytes,
    )
    .unwrap();
    std::fs::write("evidence/redemption-program.bin", REDEMPTION_GUEST_ELF).unwrap();
}
