use alloy_primitives::{Address, B256, U256};
use arcane_attestation_actor::{Command, staged::history};
use arcane_attestation_core::Error;
use lee_core::{
    account::{AccountId, Actor},
    program::ReceiveInput,
};
use redemption_inbound::{Claim, Message, Policy, claim_marker, handle, nullifier, read_balance};

mod support;
use support::proof;

fn input(receiver: Actor) -> ReceiveInput {
    ReceiveInput {
        receiver,
        from: None,
        is_authorized: true,
        pre_state: Vec::new().into(),
        message: Vec::new(),
    }
}

fn policy_and_recipient() -> (Policy, Actor, B256) {
    (
        Policy {
            escrow: Address::repeat_byte(2),
            instance_id: B256::repeat_byte(3),
            verifier_program: AccountId::new([4; 32]),
        },
        Actor::new(AccountId::new([5; 32]), AccountId::new([6; 32])),
        B256::repeat_byte(7),
    )
}

fn message(claim: &Claim) -> Message {
    Message::Claim(serde_json::to_vec(claim).unwrap())
}

#[test]
fn one_deposit_uses_one_marker_across_two_authenticated_header_paths() {
    let (policy, receiver, secret) = policy_and_recipient();
    let first = proof(&policy, receiver, secret, 8, U256::from(40));
    let mut second = proof(&policy, receiver, secret, 9, U256::from(40));
    let mut first = Claim {
        siblings: history::second_leaf_path(&second.context),
        ..first
    };
    second.index = 1;
    second.siblings = history::second_leaf_path(&first.context);
    let tracker = Actor::new(
        AccountId::new(policy.instance_id.0),
        policy.verifier_program,
    );
    let id = nullifier(&policy, receiver, secret);
    let marker = claim_marker(receiver.program_account_id, id);
    let roots = [&first, &second].map(|proof| {
        let effect = handle(&input(receiver), &policy, message(proof)).unwrap();
        assert_eq!(read_balance(&effect.post_state.unwrap()), 40);
        assert!(effect.events.is_empty());
        let [check, consume] = &effect.sync_messages[..] else {
            panic!("expected an atomic root check and marker consume")
        };
        assert_eq!(check.to, tracker);
        let data: Vec<u8> = borsh::from_slice(&check.message).unwrap();
        let Command::RequireHeaderRoot(root) = serde_json::from_slice(&data).unwrap() else {
            panic!("expected RFP-021 root command")
        };
        assert_eq!(consume.to, marker);
        assert!(matches!(
            borsh::from_slice::<Message>(&consume.message).unwrap(),
            Message::Consume(derived) if derived == id.0
        ));
        root
    });
    assert_eq!(roots[0], roots[1]);
    assert_ne!(
        first.context.verify(&first.request).unwrap().fact_id,
        second.context.verify(&second.request).unwrap().fact_id
    );

    let mut mark = input(marker);
    assert_eq!(
        handle(&mark, &policy, Message::Consume(id.0)),
        Err(Error::Binding)
    );
    mark.from = Some(receiver);
    mark.pre_state = handle(&mark, &policy, Message::Consume(id.0))
        .unwrap()
        .post_state
        .unwrap();
    assert_eq!(
        handle(&mark, &policy, Message::Consume(id.0)),
        Err(Error::Duplicate)
    );

    first.request.value += U256::ONE;
    assert_eq!(
        handle(&input(receiver), &policy, message(&first)),
        Err(Error::Inclusion)
    );
}

#[test]
fn claim_binds_wallet_storage_and_amount() {
    let (policy, receiver, secret) = policy_and_recipient();
    let mut witness = proof(&policy, receiver, secret, 8, U256::from(40));
    let impostor = Actor::new(AccountId::new([10; 32]), receiver.program_account_id);
    assert_eq!(
        handle(&input(impostor), &policy, message(&witness)),
        Err(Error::Binding)
    );
    let mut unsigned = input(receiver);
    unsigned.is_authorized = false;
    assert_eq!(
        handle(&unsigned, &policy, message(&witness)),
        Err(Error::Binding)
    );
    witness.request.address = Address::repeat_byte(11);
    assert_eq!(
        handle(&input(receiver), &policy, message(&witness)),
        Err(Error::Binding)
    );
    let overflow = proof(
        &policy,
        receiver,
        secret,
        8,
        U256::from(u128::MAX) + U256::ONE,
    );
    assert_eq!(
        handle(&input(receiver), &policy, message(&overflow)),
        Err(Error::Malformed)
    );
}
