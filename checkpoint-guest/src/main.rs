use risc0_zkvm::guest::env;
include!("../../evidence/pinned-images.rs");
fn main() {
    let bytes=lee_core::program::read_input_frame();
    let input:acceptance_core::Input=borsh::from_slice(&bytes).unwrap();
    assert_eq!(input.token_image,TOKEN_IMAGE,"backend code is bound to the deployed native guest");
    let m=&input.tx.message;
    env::verify(PPE_IMAGE, &lee_core::to_borsh_frame(&(&m.public_context,&m.execution))).unwrap();
    let (_,journal)=acceptance_core::apply(input);
    env::commit_slice(&borsh::to_vec(&journal).unwrap());
}
