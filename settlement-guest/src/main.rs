use risc0_zkvm::guest::env;
use settlement_guest::{Input, apply};

fn main() {
    let frame = lee_core::program::read_input_frame();
    let input: Input = borsh::from_slice(&frame).expect("settlement input");
    let (_, journal) = apply(&input);
    env::commit_slice(&borsh::to_vec(&journal).unwrap());
}
