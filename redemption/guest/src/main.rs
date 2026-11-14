use redemption_core::{WITNESS_BYTES, Witness, redeem};
use risc0_zkvm::guest::env;

fn main() {
    let mut bytes = [0; WITNESS_BYTES];
    env::read_slice(&mut bytes);
    let witness: Witness = borsh::from_slice(&bytes).expect("canonical private witness");
    let journal = redeem(&witness).expect("valid private redemption");
    env::commit_slice(&borsh::to_vec(&journal).unwrap());
}
