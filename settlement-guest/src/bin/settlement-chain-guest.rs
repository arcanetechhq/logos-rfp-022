#[path = "../composition.rs"]
mod composition;

use composition::{SETTLEMENT_IMAGE, join};
use risc0_zkvm::guest::env;
use settlement_guest::Journal;

fn main() {
    let frame = lee_core::program::read_input_frame();
    let [first, next]: [Journal; 2] = borsh::from_slice(&frame).expect("two settlement journals");
    for journal in [&first, &next] {
        env::verify(SETTLEMENT_IMAGE, &borsh::to_vec(journal).unwrap())
            .expect("native LEZ settlement receipt");
    }
    let result = join(&first, &next).expect("consecutive LEZ blocks and committed states");
    env::commit_slice(&borsh::to_vec(&result).unwrap());
}
