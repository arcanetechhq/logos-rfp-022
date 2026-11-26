use risc0_zkvm::guest::env;
use settlement_guest::burn::{Input, apply};

fn main() {
    let frame = lee_core::program::read_input_frame();
    let input: Input = borsh::from_slice(&frame).expect("burn block input");
    let (_, journal) = apply(&input);
    env::commit_slice(&borsh::to_vec(&journal).unwrap());
}
