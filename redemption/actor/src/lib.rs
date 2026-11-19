//! Wrapped-balance burn and a public incremental exit tree in native LEZ state.
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, Actor},
    program::{PdaSeed, ProgramEvent, ReceiveInput, Response},
};
use redemption_core::{ExitTree, Hash, burn_leaf};

pub const TOKEN: AccountId = AccountId::new([0x44; 32]);
pub const EXIT_SELECTOR: [u8; 8] = *b"RFP022EX";

#[derive(BorshSerialize, BorshDeserialize)]
pub enum Message {
    Transfer {
        to: AccountId,
        amount: u128,
    },
    Credit {
        amount: u128,
    },
    Burn {
        amount: u128,
        pairing: Hash,
        binding: Hash,
    },
    Append {
        amount: u128,
        pairing: Hash,
        binding: Hash,
    },
    Claim(Vec<u8>),
    Consume([u8; 32]),
}

pub fn sink() -> Actor {
    sink_for(TOKEN)
}

pub fn sink_for(program: AccountId) -> Actor {
    Actor::new(
        AccountId::for_public_pda(&program, &PdaSeed::new([0x45; 32])),
        program,
    )
}

fn actor_state<T: Default + BorshDeserialize>(data: &[u8]) -> T {
    if data.is_empty() {
        T::default()
    } else {
        borsh::from_slice(data).expect("canonical actor state")
    }
}

pub fn read_balance(data: &[u8]) -> u128 {
    actor_state(data)
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ExitState {
    pub pairing: Hash,
    pub tree: ExitTree,
}

pub fn read_exit_state(data: &[u8]) -> ExitState {
    borsh::from_slice(data).expect("pairing initialized by native state")
}

fn with_state(response: Response, state: &impl BorshSerialize) -> Response {
    Response {
        post_state: Some(borsh::to_vec(state).unwrap().into()),
        ..response
    }
}

pub fn handle(input: &ReceiveInput, message: Message) -> Response {
    assert_eq!(input.receiver.program_account_id, TOKEN);
    handle_wrapped(input, message)
}

/// Deployed LEZ programs take their identity from the authenticated receiver.
pub fn handle_wrapped(input: &ReceiveInput, message: Message) -> Response {
    let program = input.receiver.program_account_id;
    let sink = sink_for(program);
    if let Message::Append {
        amount,
        pairing,
        binding,
    } = message
    {
        assert_eq!(input.receiver, sink);
        assert!(input.from_own_program() && amount > 0 && binding != [0; 32]);
        let mut exits = read_exit_state(&input.pre_state);
        assert_eq!(exits.pairing, pairing, "burn pairing mismatch");
        let leaf = burn_leaf(program.as_ref().try_into().unwrap(), amount, binding);
        let position = exits.tree.append(leaf).expect("exit tree capacity");
        let event = ProgramEvent {
            selector: EXIT_SELECTOR,
            data: borsh::to_vec(&(position, leaf)).unwrap(),
        };
        return with_state(Response::keep_state().event(event), &exits);
    }
    assert_ne!(input.receiver, sink);
    let mut balance = read_balance(&input.pre_state);
    let response = match message {
        Message::Transfer { to, amount } => {
            assert!(input.is_authorized && amount > 0);
            assert_ne!(to, sink.account_id);
            balance = balance
                .checked_sub(amount)
                .expect("insufficient wrapped balance");
            let credit = Message::Credit { amount };
            Response::keep_state().send_sync(Actor::new(to, program), &credit)
        }
        Message::Credit { amount } => {
            assert!(input.from_own_program() && amount > 0);
            balance = balance
                .checked_add(amount)
                .expect("wrapped balance overflow");
            Response::keep_state()
        }
        Message::Burn {
            amount,
            pairing,
            binding,
        } => {
            assert!(input.is_authorized && amount > 0 && binding != [0; 32]);
            balance = balance
                .checked_sub(amount)
                .expect("insufficient wrapped balance");
            Response::keep_state().send_sync(
                sink,
                &Message::Append {
                    amount,
                    pairing,
                    binding,
                },
            )
        }
        Message::Append { .. } => unreachable!(),
        Message::Claim(_) | Message::Consume(_) => panic!("inbound program required"),
    };
    with_state(response, &balance)
}
