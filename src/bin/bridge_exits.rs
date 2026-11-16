#[path = "../../redemption/fixture.rs"]
mod fixture;
#[path = "../lez_blocks.rs"]
mod lez_blocks;

use common::transaction::LeeTransaction;
use lee::{FeeDeclaration, PrivateKey, PublicKey, PublicTransaction, V03State, public_transaction};
use lee_core::account::{AccountId, Actor};
use redemption_actor::{
    EXIT_SELECTOR, ExitState, Message, TOKEN, read_balance, read_exit_state, sink,
};
use redemption_core::{ExitTree, burn_leaf};
use reverse_settlement_prototype::exit_index::ExitIndex;
use std::{borrow::Cow, collections::BTreeMap};

fn signed_call(
    state: &V03State,
    key: &PrivateKey,
    payer: AccountId,
    target: Actor,
    call: Message,
) -> LeeTransaction {
    let allowed = if target == sink() {
        vec![target]
    } else {
        vec![target, sink()]
    };
    let message = public_transaction::Message::try_new_with_fees(
        target,
        allowed,
        BTreeMap::from([(payer, state.get_account_by_id(payer).nonce)]),
        call,
        FeeDeclaration::new(payer, 10_000_000, 0, 1_000_000_000),
    )
    .unwrap();
    let witnesses = public_transaction::WitnessSet::for_message(&message, &[key]);
    LeeTransaction::Public(PublicTransaction::new(message, witnesses))
}

fn main() {
    assert_eq!(std::env::var("RISC0_DEV_MODE").as_deref(), Ok("0"));
    let key = PrivateKey::try_new([0x0a; 32]).unwrap();
    let owner = AccountId::from(&PublicKey::new_from_private_key(&key));
    let owner_actor = Actor::new(owner, TOKEN);
    let program = lee::program::Program::new(Cow::Borrowed(
        redemption_burn_methods::REDEMPTION_BURN_GUEST_ELF,
    ))
    .unwrap();
    let (instance, payout, secret) = fixture::note_for(TOKEN.as_ref().try_into().unwrap());
    let pairing = instance.id();
    let binding = redemption_core::burn_binding(pairing, &payout, secret).unwrap();
    let mut expected = ExitState {
        pairing,
        tree: ExitTree::default(),
    };
    let mut state = lez_blocks::seeded_state(
        TOKEN,
        program,
        sink().account_id,
        Some(borsh::to_vec(&expected).unwrap()),
        owner,
        borsh::to_vec(&100u128).unwrap(),
    );
    let mut tip = None;
    let block = lez_blocks::build_block(&state, tip.as_ref(), vec![]);
    lez_blocks::accept(&mut state, &mut tip, &block).unwrap();

    let read_tree = |state: &V03State| {
        read_exit_state(
            state
                .get_account_by_id(sink().account_id)
                .data
                .actor_state(TOKEN),
        )
    };
    let get_balance =
        |state: &V03State| read_balance(state.get_account_by_id(owner).data.actor_state(TOKEN));
    assert_eq!(read_tree(&state), expected);
    let mut exit_index = ExitIndex::default();
    for (index, (amount, binding)) in [(payout.amount, binding), (30, [0x11; 32])]
        .into_iter()
        .enumerate()
    {
        let tx = signed_call(
            &state,
            &key,
            owner,
            owner_actor,
            Message::Burn {
                amount,
                pairing,
                binding,
            },
        );
        let block = lez_blocks::build_block(&state, tip.as_ref(), vec![tx]);
        let events = lez_blocks::accept(&mut state, &mut tip, &block).unwrap();
        assert!(block.body.receipts[0].success);
        let exits: Vec<_> = events
            .iter()
            .filter(|group| group.tx_index == 0)
            .flat_map(|group| &group.events)
            .filter(|entry| entry.account_id == TOKEN && entry.event.selector == EXIT_SELECTOR)
            .map(|entry| {
                borsh::from_slice::<(u32, redemption_core::Hash)>(&entry.event.data).unwrap()
            })
            .collect();
        let [(position, leaf)] = exits.as_slice() else {
            panic!("each accepted burn emits one sequenced exit");
        };
        assert_eq!(
            *leaf,
            burn_leaf(TOKEN.as_ref().try_into().unwrap(), amount, binding)
        );
        assert_eq!(expected.tree.append(*leaf).unwrap(), *position);
        assert_eq!(*position, index as u32);
        let committed = read_tree(&state);
        assert_eq!(committed, expected);
        exit_index.apply(&exits, committed.tree.root()).unwrap();
    }
    assert_eq!(exit_index.membership(0).unwrap().0, expected.tree.root());
    assert_eq!(get_balance(&state), 30);
    let committed = read_tree(&state);
    let snapshot = fixture::AppliedExitSnapshot {
        block_hash: tip.as_ref().unwrap().hash.0,
        state_digest: redemption_core::state_commitment(&borsh::to_vec(&state).unwrap()),
        leaves: exit_index.leaves().to_vec(),
        root: committed.tree.root(),
    };
    std::fs::write(
        "evidence/redemption-native-v2-checkpoint.borsh",
        borsh::to_vec(&snapshot).unwrap(),
    )
    .unwrap();
    println!(
        "PASS replay of two accepted LEZ burn events matches the persistent exit tree and balance debits"
    );

    for (actor, call) in [
        (
            owner_actor,
            Message::Burn {
                amount: 31,
                pairing,
                binding: [0x12; 32],
            },
        ),
        (
            owner_actor,
            Message::Burn {
                amount: 1,
                pairing: [0x31; 32],
                binding: [0x12; 32],
            },
        ),
        (
            sink(),
            Message::Append {
                amount: 1,
                pairing,
                binding: [0x13; 32],
            },
        ),
    ] {
        let tx = signed_call(&state, &key, owner, actor, call);
        let block = lez_blocks::build_block(&state, tip.as_ref(), vec![tx]);
        let events = lez_blocks::accept(&mut state, &mut tip, &block).unwrap();
        assert!(!block.body.receipts[0].success);
        assert!(
            !events.iter().flat_map(|group| &group.events).any(|entry| {
                entry.account_id == TOKEN && entry.event.selector == EXIT_SELECTOR
            })
        );
        assert_eq!(get_balance(&state), 30);
        assert_eq!(read_tree(&state), expected);
    }
    println!(
        "PASS overspend, foreign pairing and unauthorized append leave both wrapped balance and exit root intact"
    );
}
