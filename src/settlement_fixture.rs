use acceptance_core::State;
use common::{
    block::{Block, HashableBlockData},
    transaction::LeeTransaction,
};
use lee::{FeeDeclaration, PrivateKey, PublicKey, PublicTransaction, V03State, public_transaction};
use lee_core::{
    account::{AccountId, Actor, Nonce},
    program::PROGRAM_LOADER_ACCOUNT_ID,
};
use program_loader_core::{LoaderActorState, ProgramSegment};
use reverse_settlement_prototype::block_settlement::{
    Input, Journal, account_digest, snapshot_digest,
};
use std::collections::BTreeMap;

pub fn sign(
    key: &PrivateKey,
    sender: AccountId,
    nonce: Nonce,
    to: Actor,
    actors: [Actor; 2],
    call: impl borsh::BorshSerialize,
    gas_limit: u64,
) -> LeeTransaction {
    let message = public_transaction::Message::try_new_with_fees(
        to,
        actors,
        BTreeMap::from([(sender, nonce)]),
        call,
        FeeDeclaration::new(sender, gas_limit, 17, 1_000_000_000),
    )
    .unwrap();
    // A stable local fixture lets a separate invocation reproduce the proof input.
    let signature = k256::schnorr::SigningKey::from_bytes(key.value())
        .unwrap()
        .sign_prehash_with_aux_rand(&message.hash(), &[0; 32])
        .unwrap();
    let witnesses = public_transaction::WitnessSet::from_raw_parts(vec![(
        lee::Signature {
            value: signature.to_bytes(),
        },
        PublicKey::new_from_private_key(key),
    )]);
    LeeTransaction::Public(PublicTransaction::new(message, witnesses))
}

pub fn rehash(block: Block) -> Vec<u8> {
    borsh::to_vec(
        &HashableBlockData {
            block_id: block.header.block_id,
            prev_block_hash: block.header.prev_block_hash,
            timestamp: block.header.timestamp,
            transactions: block.body.transactions,
            receipts: block.body.receipts,
        }
        .into_pending_block(),
    )
    .unwrap()
}

pub fn check_native_block<W>(
    state: &mut V03State,
    tip: &mut Option<chain_state::Tip>,
    block: &Block,
    wrap: impl FnOnce(Input) -> W,
    apply: impl FnOnce(&W) -> (Vec<u8>, Journal),
) -> (W, Journal) {
    let mut visible: State = borsh::from_slice(&borsh::to_vec(state).unwrap()).unwrap();
    let expected_before = snapshot_digest(&visible, &BTreeMap::new());
    let mut concealed = BTreeMap::new();
    visible.accounts.retain(|id, account| {
        if ProgramSegment::from_loader_actor_state(
            account.data.actor_state(PROGRAM_LOADER_ACCOUNT_ID),
        )
        .is_some()
        {
            concealed.insert(*id, account.clone());
            false
        } else {
            true
        }
    });
    assert!(!concealed.is_empty());
    let input = wrap(Input {
        before: borsh::to_vec(&visible).unwrap(),
        concealed_accounts: concealed
            .iter()
            .map(|(id, account)| (*id, account_digest(account)))
            .collect(),
        block: borsh::to_vec(block).unwrap(),
    });
    super::lez_blocks::accept(state, tip, block).unwrap();
    let (after, journal) = apply(&input);
    let mut after: State = borsh::from_slice(&after).unwrap();
    for (id, account) in concealed {
        assert!(after.accounts.insert(id, account).is_none());
    }
    assert!(
        borsh::to_vec(&after).unwrap() == borsh::to_vec(state).unwrap(),
        "complete native state differs"
    );
    assert_eq!(journal.before, expected_before);
    assert_eq!(journal.after, snapshot_digest(&after, &BTreeMap::new()));
    (input, journal)
}
