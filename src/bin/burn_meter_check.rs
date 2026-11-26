use burn_meter::{Image, Outcome};
use lee_core::{
    account::{AccountId, Actor},
    program::{ReceiveInput, Transition},
    to_borsh_frame,
};
use redemption_actor::{ExitState, Message, TOKEN, sink};
use redemption_burn_methods::{REDEMPTION_BURN_GUEST_ELF, REDEMPTION_BURN_GUEST_ID};
use risc0_zkvm::{
    ExecutorEnv, default_executor,
    sha::{Impl, Sha256},
};

fn main() {
    assert_eq!(std::env::var("RISC0_DEV_MODE").as_deref(), Ok("0"));
    let image = Image::from_binary(REDEMPTION_BURN_GUEST_ELF).unwrap();
    let bytes = image.encode();
    let image = Image::decode(&bytes).unwrap();
    let owner = Actor::new(AccountId::new([0x71; 32]), TOKEN);
    let pairing = [0x10; 32];
    for amount in [1u128, 40, 100] {
        let input = ReceiveInput {
            receiver: owner,
            from: None,
            is_authorized: true,
            pre_state: borsh::to_vec(&100u128).unwrap().into(),
            message: borsh::to_vec(&Message::Burn {
                amount,
                pairing,
                binding: [0x20; 32],
            })
            .unwrap(),
        };
        let debit = check(&image, &input);
        let message = &debit.response.sync_messages[0];
        let mut tree = redemption_core::ExitTree::default();
        for _ in 0..amount % 3 {
            tree.append([0x33; 32]).unwrap();
        }
        check(
            &image,
            &ReceiveInput {
                receiver: sink(),
                from: Some(owner),
                is_authorized: false,
                pre_state: borsh::to_vec(&ExitState { pairing, tree }).unwrap().into(),
                message: message.message.clone(),
            },
        );
    }
    let overspend = ReceiveInput {
        receiver: owner,
        from: None,
        is_authorized: true,
        pre_state: borsh::to_vec(&100u128).unwrap().into(),
        message: borsh::to_vec(&Message::Burn {
            amount: 101,
            pairing,
            binding: [0x20; 32],
        })
        .unwrap(),
    };
    let frame = to_borsh_frame(&overspend);
    let env = ExecutorEnv::builder().write_slice(&frame).build().unwrap();
    assert!(
        default_executor()
            .execute(env, REDEMPTION_BURN_GUEST_ELF)
            .unwrap_err()
            .root_cause()
            .to_string()
            .contains("insufficient wrapped balance")
    );
    assert_eq!(image.execute(&frame, 5_000_000).unwrap(), Outcome::Panic);
    println!(
        "PASS exact native journals and user cycles for debit, append and session limit, plus native overspend panic"
    );
    println!(
        "image_id={:?} projection_bytes={} projection_sha256={}",
        REDEMPTION_BURN_GUEST_ID,
        bytes.len(),
        Impl::hash_bytes(&bytes)
    );
    if let Some(path) = std::env::args_os().nth(1) {
        std::fs::write(path, bytes).unwrap();
    }
}

fn check(image: &Image, input: &ReceiveInput) -> Transition {
    let frame = to_borsh_frame(input);
    let run = |limit| {
        default_executor().execute(
            ExecutorEnv::builder()
                .session_limit(Some(limit))
                .write_slice(&frame)
                .build()
                .unwrap(),
            REDEMPTION_BURN_GUEST_ELF,
        )
    };
    let native = run(5_000_000).unwrap();
    let replay = image.execute(&frame, 5_000_000).unwrap();
    assert_eq!(
        replay,
        Outcome::Halted {
            cycles: native.cycles(),
            journal: native.journal.bytes.clone()
        }
    );
    let short = native.cycles() - 3;
    assert!(
        run(short)
            .unwrap_err()
            .root_cause()
            .to_string()
            .starts_with("Session limit exceeded:")
    );
    assert_eq!(image.execute(&frame, short).unwrap(), Outcome::OutOfGas);
    borsh::from_slice(lee_core::from_frame(&native.journal.bytes).unwrap()).unwrap()
}
