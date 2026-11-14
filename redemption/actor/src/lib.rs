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
}

pub fn sink() -> Actor {
    Actor::new(
        AccountId::for_public_pda(&TOKEN, &PdaSeed::new([0x45; 32])),
        TOKEN,
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
    if let Message::Append {
        amount,
        pairing,
        binding,
    } = message
    {
        assert_eq!(input.receiver, sink());
        assert!(input.from_own_program() && amount > 0 && binding != [0; 32]);
        let mut exits = read_exit_state(&input.pre_state);
        assert_eq!(exits.pairing, pairing, "burn pairing mismatch");
        let leaf = burn_leaf(TOKEN.as_ref().try_into().unwrap(), amount, binding);
        let position = exits.tree.append(leaf).expect("exit tree capacity");
        let event = ProgramEvent {
            selector: EXIT_SELECTOR,
            data: borsh::to_vec(&(position, leaf)).unwrap(),
        };
        return with_state(Response::keep_state().event(event), &exits);
    }
    assert_ne!(input.receiver, sink());
    let mut balance = read_balance(&input.pre_state);
    let response = match message {
        Message::Transfer { to, amount } => {
            assert!(input.is_authorized && amount > 0);
            assert_ne!(to, sink().account_id);
            balance = balance
                .checked_sub(amount)
                .expect("insufficient wrapped balance");
            Response::keep_state().send_sync(Actor::new(to, TOKEN), &Message::Credit { amount })
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
                sink(),
                &Message::Append {
                    amount,
                    pairing,
                    binding,
                },
            )
        }
        Message::Append { .. } => unreachable!(),
    };
    with_state(response, &balance)
}
