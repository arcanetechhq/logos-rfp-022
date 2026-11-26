#[path = "../composition.rs"]
mod composition;

use composition::{ARCHIVED_SETTLEMENT_IMAGE, SETTLEMENT_IMAGE, join};
use risc0_zkvm::guest::env;
use settlement_guest::Journal;

fn main() {
    let frame = lee_core::program::read_input_frame();
    let (images, [first, next]): ([[u32; 8]; 2], [Journal; 2]) =
        borsh::from_slice(&frame).expect("two settlement journals and their images");
    for (image, journal) in images.into_iter().zip([&first, &next]) {
        assert!([SETTLEMENT_IMAGE, ARCHIVED_SETTLEMENT_IMAGE].contains(&image));
        env::verify(image, &borsh::to_vec(journal).unwrap())
            .expect("native LEZ settlement receipt");
    }
    let result = join(&first, &next).expect("consecutive LEZ blocks and committed states");
    env::commit_slice(&borsh::to_vec(&result).unwrap());
}
