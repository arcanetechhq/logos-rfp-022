use checkpoint_methods::{BLOCK_CHECKPOINT_GUEST_ELF, BLOCK_CHECKPOINT_GUEST_ID};
use reverse_settlement_prototype::block_link::{ACTION_IMAGE, BlockJournal, Input, bind};
use risc0_zkvm::{
    ExecutorEnv, InnerReceipt, ProverOpts, Receipt, default_executor, default_prover,
};
use std::{fs, path::PathBuf, time::Instant};

fn main() {
    assert!(std::env::var("RISC0_DEV_MODE").map_or(true, |v| v == "0"));
    let mut args = std::env::args_os().skip(1);
    let mode = args.next().expect("use execute, prove, or verify");
    let receipt_path = args.next().map(PathBuf::from);
    assert!(args.next().is_none(), "too many arguments");
    let action_receipt: Receipt =
        borsh::from_slice(&fs::read("evidence/checkpoint-receipt.borsh").unwrap()).unwrap();
    assert!(!matches!(action_receipt.inner, InnerReceipt::Fake(_)));
    action_receipt.verify(ACTION_IMAGE).unwrap();
    let input = Input {
        block: fs::read("evidence/accepted-burn-block.borsh").unwrap(),
        action: borsh::from_slice(&action_receipt.journal.bytes).unwrap(),
    };
    let bytes = borsh::to_vec(&input).unwrap();
    let expected = bind(input).unwrap();
    let start = Instant::now();
    let env = || {
        ExecutorEnv::builder()
            .add_assumption(action_receipt)
            .write_slice(&lee_core::to_frame(&bytes))
            .build()
            .unwrap()
    };
    let verify = |receipt: &Receipt| {
        assert!(!matches!(receipt.inner, InnerReceipt::Fake(_)));
        receipt.verify(BLOCK_CHECKPOINT_GUEST_ID).unwrap();
        assert!(receipt.verify(ACTION_IMAGE).is_err());
        let journal: BlockJournal = borsh::from_slice(&receipt.journal.bytes).unwrap();
        assert_eq!(journal, expected);
        let mut tampered = receipt.clone();
        tampered.journal.bytes[0] ^= 1;
        assert!(tampered.verify(BLOCK_CHECKPOINT_GUEST_ID).is_err());
    };
    match mode.to_str() {
        Some("execute") if receipt_path.is_none() => {
            let info = default_executor()
                .execute(env(), BLOCK_CHECKPOINT_GUEST_ELF)
                .unwrap();
            assert_eq!(
                borsh::from_slice::<BlockJournal>(&info.journal.bytes).unwrap(),
                expected
            );
            println!("PASS guest execution user_cycles={}", info.cycles());
        }
        Some("prove") => {
            let path = receipt_path.expect("provide a receipt output path");
            println!(
                "Proving block link for {}",
                hex::encode(expected.block_hash)
            );
            let info = default_prover()
                .prove_with_opts(env(), BLOCK_CHECKPOINT_GUEST_ELF, &ProverOpts::composite())
                .unwrap();
            verify(&info.receipt);
            let bytes = borsh::to_vec(&info.receipt).unwrap();
            fs::write(path, &bytes).unwrap();
            println!(
                "PASS block link proof cycles={} receipt_bytes={} elapsed_ms={}",
                info.stats.total_cycles,
                bytes.len(),
                start.elapsed().as_millis()
            );
        }
        Some("verify") => {
            let path = receipt_path.expect("provide the receipt path");
            verify(&borsh::from_slice(&fs::read(path).unwrap()).unwrap());
            println!("PASS block link receipt reverified");
        }
        _ => panic!("usage: block-checkpoint execute | prove OUTPUT | verify RECEIPT"),
    }
    println!(
        "block={} parent={} payload_digest={} exit_root={}",
        hex::encode(expected.block_hash),
        hex::encode(expected.parent_hash),
        hex::encode(expected.payload_digest),
        hex::encode(expected.action.exit_root),
    );
}
