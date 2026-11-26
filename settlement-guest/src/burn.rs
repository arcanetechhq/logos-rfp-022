use super::{Journal, State, apply_inner, hash, set_actor};
use borsh::{BorshDeserialize, BorshSerialize};
use burn_meter::{Image, Outcome};
use lee_core::{
    RootCall,
    account::Actor,
    from_frame,
    program::{
        PROGRAM_LOADER_ACCOUNT_ID, ProgramHeader, ReceiveInput, Transition, ValidityWindows,
    },
    to_borsh_frame,
};
use program_loader_core::LoaderActorState;
use redemption_actor::{EXIT_SELECTOR, TOKEN, sink};

pub const ACTOR_IMAGE: [u32; 8] = [
    487496126, 1571472143, 1319561619, 928122058, 1716399605, 3671107669, 2973367471, 3467887481,
];
// SHA-256 of Image::from_binary on the exact CombinedELF with the image ID above.
const MEMORY_DIGEST: [u8; 32] = [
    0x25, 0x84, 0xb4, 0x3d, 0xe9, 0x44, 0xdb, 0xca, 0x7b, 0xe1, 0x19, 0xec, 0xab, 0x17, 0x3b, 0x8f,
    0x49, 0xce, 0x98, 0x37, 0x55, 0xc6, 0xcf, 0x25, 0x87, 0x4c, 0xe1, 0x26, 0xa2, 0xca, 0x14, 0x2f,
];

#[derive(BorshDeserialize, BorshSerialize)]
pub struct Input {
    pub block: super::Input,
    pub actor_memory: Vec<u8>,
}

pub fn apply(input: &Input) -> (Vec<u8>, Journal) {
    assert_eq!(
        hash(&input.actor_memory),
        MEMORY_DIGEST,
        "native burn memory image"
    );
    let meter = Image::decode(&input.actor_memory).expect("pinned RV32IM image");
    apply_inner(&input.block, Some(&meter))
}

pub(super) fn check_program(state: &State) {
    let loader = state.accounts[&TOKEN]
        .data
        .actor_state(PROGRAM_LOADER_ACCOUNT_ID);
    let header = ProgramHeader::from_loader_actor_state(loader).expect("deployed burn program");
    assert!(header.immutable);
    assert_eq!(header.image_id, ACTOR_IMAGE);
}

pub(super) fn execute(
    state: &mut State,
    root: &RootCall,
    meter: &Image,
    gas_limit: u64,
) -> Option<u64> {
    let call = |receiver: Actor, from, is_authorized, message| ReceiveInput {
        receiver,
        from,
        is_authorized,
        pre_state: state.accounts[&receiver.account_id]
            .data
            .actor_state(TOKEN)
            .clone(),
        message,
    };
    let (debit, debit_cycles) = run(
        meter,
        call(root.to, None, true, root.message.clone()),
        gas_limit,
    )?;
    let [append] = debit.response.sync_messages.as_slice() else {
        panic!("one append required")
    };
    assert_eq!(append.to, sink());
    assert!(append.pda_seeds.is_empty() && debit.response.events.is_empty());
    let (exit, append_cycles) = run(
        meter,
        call(append.to, Some(root.to), false, append.message.clone()),
        gas_limit.saturating_sub(debit_cycles),
    )?;
    assert!(exit.response.sync_messages.is_empty());
    let [event] = exit.response.events.as_slice() else {
        panic!("one exit event required")
    };
    assert_eq!(event.selector, EXIT_SELECTOR);
    // The native executor commits only after the whole synchronous action succeeds.
    set_actor(state, root.to, debit.response.post_state.unwrap());
    set_actor(state, sink(), exit.response.post_state.unwrap());
    Some(debit_cycles.checked_add(append_cycles).unwrap())
}

fn run(meter: &Image, input: ReceiveInput, budget: u64) -> Option<(Transition, u64)> {
    let Outcome::Halted { cycles, journal } = meter
        .execute(&to_borsh_frame(&input), budget)
        .expect("supported native actor execution")
    else {
        // Full budget for program panic and session exhaustion (native charge rules).
        return None;
    };
    let transition: Transition = borsh::from_slice(from_frame(&journal).unwrap()).unwrap();
    assert_eq!(
        borsh::to_vec(&transition.input).unwrap(),
        borsh::to_vec(&input).unwrap()
    );
    assert!(transition.response.async_messages.is_empty());
    assert_eq!(
        borsh::to_vec(&transition.response.validity).unwrap(),
        borsh::to_vec(&ValidityWindows::default()).unwrap(),
    );
    Some((transition, cycles))
}
