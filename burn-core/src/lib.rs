//! Experimental wrapped-asset equivalent. Synthetic genesis, no production mint or vault.
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{account::{AccountId, Actor}, program::{PdaSeed, ProgramEvent, ReceiveInput, Response}};
use sha2::{Digest, Sha256};
pub const TOKEN: AccountId = AccountId::new([0x42; 32]);
pub const EXIT_SELECTOR: [u8; 8] = *b"RFP022EX";
#[derive(Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Holding { pub balance: u128, pub exits: Vec<[u8; 32]> }
#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub enum Message {
    Transfer { to: AccountId, amount: u128 },
    Credit { amount: u128 },
    Burn { amount: u128, binding: [u8; 32] },
    Append { amount: u128, binding: [u8; 32] },
}
pub fn sink() -> Actor { Actor::new(AccountId::for_public_pda(&TOKEN, &PdaSeed::new([0x45; 32])), TOKEN) }
pub fn leaf(amount: u128, binding: [u8; 32]) -> [u8; 32] {
    let mut h = Sha256::new(); h.update(b"RFP022/exit/v1"); h.update(TOKEN.as_ref());
    h.update(amount.to_le_bytes()); h.update(binding); h.finalize().into()
}
/// A list accumulator for this small experiment; membership/privacy circuit is out of scope.
pub fn exit_root(exits: &[[u8; 32]]) -> [u8; 32] {
    let mut h = Sha256::new(); h.update(b"RFP022/exits/v1");
    h.update((exits.len() as u64).to_le_bytes()); for e in exits { h.update(e); } h.finalize().into()
}
pub fn read_state(bytes: &[u8]) -> Holding {
    if bytes.is_empty() { Holding::default() } else { borsh::from_slice(bytes).expect("canonical holding") }
}
pub fn handle(input: &ReceiveInput, message: Message) -> Response {
    assert_eq!(input.receiver.program_account_id, TOKEN);
    let mut state = read_state(&input.pre_state);
    let response = match message {
        Message::Transfer { to, amount } => {
            assert!(input.receiver != sink() && input.is_authorized && amount > 0);
            assert_ne!(to, sink().account_id);
            state.balance = state.balance.checked_sub(amount).expect("insufficient wrapped balance");
            Response::keep_state().send_sync(Actor::new(to, TOKEN), &Message::Credit { amount })
        }
        Message::Credit { amount } => {
            assert!(input.from_own_program() && input.receiver != sink());
            state.balance = state.balance.checked_add(amount).expect("overflow");
            Response::keep_state()
        }
        Message::Burn { amount, binding } => {
            assert!(input.receiver != sink() && input.is_authorized && amount > 0);
            state.balance = state.balance.checked_sub(amount).expect("insufficient wrapped balance");
            Response::keep_state().send_sync(sink(), &Message::Append { amount, binding })
        }
        Message::Append { amount, binding } => {
            assert!(state.exits.is_empty(), "experimental one-exit capacity forces stale settlement revert");
            assert!(input.receiver == sink() && input.from_own_program());
            let commitment = leaf(amount, binding); state.exits.push(commitment);
            Response::keep_state().event(ProgramEvent { selector: EXIT_SELECTOR, data: commitment.to_vec() })
        }
    };
    Response { post_state: Some(borsh::to_vec(&state).unwrap().into()), ..response }
}
