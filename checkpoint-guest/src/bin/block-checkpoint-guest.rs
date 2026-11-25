#[path = "../block_link.rs"]
mod block_link;

use block_link::{ACTION_IMAGE, Input, bind};
use risc0_zkvm::guest::env;

fn main() {
    let frame = lee_core::program::read_input_frame();
    let input: Input = borsh::from_slice(&frame).expect("block and action journal");
    let action_journal = borsh::to_vec(&input.action).unwrap();
    env::verify(ACTION_IMAGE, &action_journal).expect("native action receipt");
    let journal = bind(input).expect("one proved action in the native LEZ block");
    env::commit_slice(&borsh::to_vec(&journal).unwrap());
}
