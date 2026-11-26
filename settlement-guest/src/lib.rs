//! Settlement of restricted public transactions at the pinned LEZ revision.
use acceptance_core::{Evidence, Fee, State, Tx, hash};
use borsh::{BorshDeserialize, BorshSerialize};
use fee_core::{
    BlockFeeSummary,
    assess::{FeeTxView, fee_actual_base, fee_reserve},
    market,
    state::FeeState,
    validity::{accumulate_exec_gas, accumulate_stor_gas, validate_static_tx},
};
use k256::{ecdsa::signature::hazmat::PrehashVerifier, schnorr};
use lee_core::{
    GENESIS_BLOCK_ID, RootCall,
    account::{Account, AccountId, Actor, Balance, Nonce},
    execution_state::PublicExecutionContext,
    native_token::{self, NATIVE_TOKEN_PROGRAM_ID},
    program::{PROGRAM_LOADER_ACCOUNT_ID, ProgramHeader, ReceiveInput},
};
use program_loader_core::LoaderActorState;
use std::collections::BTreeMap;

#[cfg(feature = "metered-burn")]
pub mod burn;

const BLOCK_DOMAIN: &[u8; 32] = b"/LEE/v0.3/Message/Block/\x00\x00\x00\x00\x00\x00\x00\x00";

// Native Borsh layout at the LEZ pin in reproduce.sh. Using the wire types keeps the
// host program executor and its IPC dependencies out of the zkVM guest.
#[derive(BorshDeserialize, BorshSerialize)]
struct Block {
    block_id: u64,
    parent: [u8; 32],
    hash: [u8; 32],
    timestamp: u64,
    transactions: Vec<Transaction>,
    receipts: Vec<TransactionReceipt>,
    status: BedrockStatus,
}

#[derive(BorshDeserialize, BorshSerialize)]
enum BedrockStatus {
    Pending,
    Safe,
    Finalized,
}

#[derive(BorshDeserialize, BorshSerialize)]
enum Transaction {
    Public(PublicTransaction),
    PrivacyPreserving(Tx),
}

#[derive(BorshDeserialize, BorshSerialize)]
struct PublicTransaction {
    public_context: PublicExecutionContext,
    root: RootCall,
    nonces: BTreeMap<AccountId, Nonce>,
    fee: Option<Fee>,
    admission_evidence: Vec<Evidence>,
    signatures: Vec<([u8; 64], [u8; 32])>,
}

#[derive(BorshDeserialize, BorshSerialize)]
struct TransactionReceipt {
    success: bool,
    gas_exec: u64,
    gas_stor: u64,
    fee_base: u128,
    tip: u128,
    refund: u128,
}

// Native artifacts, checked against the generated artifact IDs by the runner.
pub const FEE_IMAGE: [u32; 8] = [
    1867176690, 1447473865, 1847877820, 1306343680, 2363819622, 1038646769, 3840698976, 2959008891,
];
pub const CLOCK_IMAGE: [u32; 8] = [
    2003601088, 3854535718, 4258979582, 424733494, 2087743250, 2422381388, 1949003261, 2489603991,
];

#[derive(BorshDeserialize, BorshSerialize)]
pub struct Input {
    pub before: Vec<u8>,
    pub concealed_accounts: BTreeMap<AccountId, [u8; 32]>,
    pub block: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshDeserialize, BorshSerialize)]
pub struct Journal {
    pub before: [u8; 32],
    pub after: [u8; 32],
    pub block_hash: [u8; 32],
    pub parent_hash: [u8; 32],
    pub payload_digest: [u8; 32],
    pub pairing: [u8; 32],
    pub exit_root: [u8; 32],
    pub height: u64,
    pub timestamp: u64,
}

pub fn account_digest(account: &Account) -> [u8; 32] {
    let mut bytes = b"RFP022/native-account/v1".to_vec();
    account.serialize(&mut bytes).unwrap();
    hash(&bytes)
}

pub fn snapshot_digest(
    state: &State,
    concealed_accounts: &BTreeMap<AccountId, [u8; 32]>,
) -> [u8; 32] {
    let mut accounts = concealed_accounts.clone();
    for (id, account) in &state.accounts {
        assert!(accounts.insert(*id, account_digest(account)).is_none());
    }
    let mut bytes = b"RFP022/native-state-accounts/v1".to_vec();
    (
        accounts,
        &state.private,
        &state.publications,
        &state.recovery,
    )
        .serialize(&mut bytes)
        .unwrap();
    hash(&bytes)
}

/// The verifier must independently authenticate the predecessor commitment and block hash.
/// Concealed accounts are committed by address and digest and never accessed or updated.
/// This commitment covers the full native state but has a different domain from the action proof.
pub fn apply(input: &Input) -> (Vec<u8>, Journal) {
    apply_inner(
        input,
        #[cfg(feature = "metered-burn")]
        None,
    )
}

fn apply_inner(
    input: &Input,
    #[cfg(feature = "metered-burn")] meter: Option<&burn_meter::Image>,
) -> (Vec<u8>, Journal) {
    let mut state: State = borsh::from_slice(&input.before).expect("native state");
    assert_eq!(borsh::to_vec(&state).unwrap(), input.before);
    let before = snapshot_digest(&state, &input.concealed_accounts);
    let block: Block = borsh::from_slice(&input.block).expect("native block");
    assert_eq!(borsh::to_vec(&block).unwrap(), input.block);
    let mut header = BLOCK_DOMAIN.to_vec();
    (
        block.block_id,
        block.parent,
        block.timestamp,
        &block.transactions,
        &block.receipts,
    )
        .serialize(&mut header)
        .unwrap();
    assert_eq!(block.hash, hash(&header));

    let clock = clock_core::clock_actor();
    let clock_before = clock_core::ClockAccountData::from_bytes(
        state.accounts[&clock.account_id]
            .data
            .actor_state(clock.program_account_id),
    );
    assert!(block.block_id > GENESIS_BLOCK_ID);
    assert_eq!(clock_before.block_id.checked_add(1), Some(block.block_id));
    let fee_program = fee_core::fee_account_id();
    for (program, image) in [
        (fee_program, FEE_IMAGE),
        (clock.program_account_id, CLOCK_IMAGE),
    ] {
        let loader = state.accounts[&program]
            .data
            .actor_state(PROGRAM_LOADER_ACCOUNT_ID);
        let header = ProgramHeader::from_loader_actor_state(loader).expect("deployed program");
        assert!(header.immutable);
        assert_eq!(header.image_id, image);
    }
    let registry = system_upgrader_core::registry_account_id();
    assert!(
        !state.accounts.contains_key(&registry)
            && !input.concealed_accounts.contains_key(&registry)
    );

    let (Transaction::Public(tick), front) = block.transactions.split_last().unwrap() else {
        panic!("clock transaction required");
    };
    let (Transaction::Public(distribute), transactions) = front.split_last().unwrap() else {
        panic!("fee transaction required");
    };
    let fee_actor = Actor::new(
        fee_core::compute_fee_state_account_id(fee_program),
        fee_program,
    );
    let escrow = fee_core::compute_fee_escrow_account_id(fee_program);
    let inbox = fee_core::compute_fee_inbox_account_id(fee_program);
    let restricted = [
        fee_actor.account_id,
        escrow,
        inbox,
        clock.account_id,
        bridge_core::compute_bridge_account_id(bridge_core::bridge_account_id()),
    ];
    let mut fee_state = FeeState::from_bytes(
        state.accounts[&fee_actor.account_id]
            .data
            .actor_state(fee_program),
    );
    assert!(fee_state.pending.is_none());
    assert!(
        (market::BASE_FEE_EXEC_MIN..=market::BASE_FEE_EXEC_MAX).contains(&fee_state.base_fee_exec)
    );
    assert!(
        (market::BASE_FEE_STOR_MIN..=market::BASE_FEE_STOR_MAX).contains(&fee_state.base_fee_stor)
    );
    assert_eq!(state.accounts[&inbox].data.native_balance().unwrap(), 0);
    assert_eq!(transactions.len(), block.receipts.len());
    let mut summary = BlockFeeSummary::default();
    for (tx, listed) in transactions.iter().zip(&block.receipts) {
        let actual = settle(
            tx,
            &mut state,
            inbox,
            &fee_state,
            &restricted,
            &mut summary,
            #[cfg(feature = "metered-burn")]
            meter,
        );
        assert_eq!(
            borsh::to_vec(&actual).unwrap(),
            borsh::to_vec(listed).unwrap()
        );
    }

    let fee_core::Message::Distribute { producer, .. } =
        borsh::from_slice(&distribute.root.message).expect("fee distribution")
    else {
        panic!("fee distribution required");
    };
    assert!(!restricted.contains(&producer));
    assert!(state.accounts.contains_key(&producer));
    let payout = fee_state.apply_block(&summary);
    check_system_tx(
        distribute,
        fee_actor,
        [
            fee_actor,
            Actor::native_balance(escrow),
            Actor::native_balance(inbox),
            Actor::native_balance(producer),
        ],
        fee_core::Message::Distribute {
            summary,
            payout,
            producer,
        },
    );
    assert_eq!(
        state.accounts[&inbox].data.native_balance().unwrap(),
        summary
            .revenue_base
            .checked_add(summary.revenue_tip)
            .unwrap()
    );
    for (from, to, amount) in [
        (inbox, escrow, summary.revenue_base),
        (inbox, producer, summary.revenue_tip),
        (escrow, producer, payout),
    ] {
        if amount > 0 {
            transfer(&mut state, from, to, amount).expect("fee payout");
        }
    }
    set_actor(&mut state, fee_actor, fee_state.to_bytes().into());

    let (height, timestamp) = (block.block_id, block.timestamp);
    check_system_tx(
        tick,
        clock,
        [clock],
        clock_core::Message::Tick {
            timestamp,
            block_id: height,
        },
    );
    set_actor(
        &mut state,
        clock,
        clock_core::ClockAccountData {
            block_id: height,
            timestamp,
        }
        .to_bytes()
        .into(),
    );
    // V03State::apply_state_diff records the current private root even for public diffs.
    state.private.0.roots.insert(state.private.0.tree.root());

    let sink = redemption_actor::sink();
    let exit = redemption_actor::read_exit_state(
        state.accounts[&sink.account_id]
            .data
            .actor_state(sink.program_account_id),
    );
    let after = borsh::to_vec(&state).unwrap();
    let journal = Journal {
        before,
        after: snapshot_digest(&state, &input.concealed_accounts),
        block_hash: block.hash,
        parent_hash: block.parent,
        payload_digest: hash(&input.block),
        pairing: exit.pairing,
        exit_root: exit.tree.root(),
        height,
        timestamp,
    };
    (after, journal)
}

enum Action {
    Transfer {
        to: AccountId,
        amount: Balance,
    },
    #[cfg(feature = "metered-burn")]
    Burn,
}

fn settle(
    tx: &Transaction,
    state: &mut State,
    inbox: AccountId,
    opening: &FeeState,
    restricted: &[AccountId],
    summary: &mut BlockFeeSummary,
    #[cfg(feature = "metered-burn")] meter: Option<&burn_meter::Image>,
) -> TransactionReceipt {
    let Transaction::Public(public) = tx else {
        panic!("public transaction required")
    };
    let fee = public.fee.as_ref().expect("charged transaction");
    let sender = fee.payer;
    let sender_actor = public.root.to;
    assert_eq!(sender_actor.account_id, sender);
    let action = match sender_actor.program_account_id {
        NATIVE_TOKEN_PROGRAM_ID => {
            let native_token::Message::Transfer { to, amount } =
                borsh::from_slice(&public.root.message).expect("native transfer")
            else {
                panic!("native transfer required");
            };
            Action::Transfer { to, amount }
        }
        #[cfg(feature = "metered-burn")]
        redemption_actor::TOKEN if meter.is_some() => {
            burn::check_program(state);
            let redemption_actor::Message::Burn { .. } =
                borsh::from_slice(&public.root.message).expect("wrapped burn")
            else {
                panic!("wrapped burn required");
            };
            Action::Burn
        }
        _ => panic!("unsupported charged actor"),
    };
    let other = match action {
        Action::Transfer { to, .. } => Actor::native_balance(to),
        #[cfg(feature = "metered-burn")]
        Action::Burn => redemption_actor::sink(),
    };
    assert_ne!(sender, other.account_id);
    assert!(!restricted.contains(&sender) && !restricted.contains(&other.account_id));
    assert!(state.accounts.contains_key(&other.account_id));
    assert!(public.admission_evidence.is_empty());
    assert_eq!(
        public.nonces,
        BTreeMap::from([(sender, state.accounts[&sender].nonce)])
    );
    check_context(&public.public_context, [sender_actor, other], [sender]);
    let [(signature, key)] = public.signatures.as_slice() else {
        panic!("one signer required")
    };
    let mut signing_data = b"/LEE/v0.3/AccountId/Public/\x00\x00\x00\x00\x00".to_vec();
    signing_data.extend(key);
    assert_eq!(sender, AccountId::new(hash(&signing_data)));
    let mut message = b"/LEE/v0.3/Message/Public/\x00\x00\x00\x00\x00\x00\x00".to_vec();
    (
        &public.public_context,
        &public.root,
        &public.nonces,
        &public.fee,
        &public.admission_evidence,
    )
        .serialize(&mut message)
        .unwrap();
    schnorr::VerifyingKey::from_bytes(key)
        .unwrap()
        .verify_prehash(
            &hash(&message),
            &schnorr::Signature::try_from(signature.as_slice()).unwrap(),
        )
        .expect("public transaction signature");

    let gas_stor = borsh::object_length(tx).unwrap().try_into().unwrap();
    let view = FeeTxView::Public {
        payer: sender,
        gas_limit: fee.gas_limit,
        data_bytes: gas_stor,
        tip: fee.tip,
        max_fee: fee.max_fee,
    };
    validate_static_tx(&view, opening).expect("fee declaration");
    let reserved = fee_reserve(&view, opening);
    transfer(state, sender, inbox, reserved).expect("fee reserve");
    let cycles = match action {
        Action::Transfer { to, amount } => match transfer(state, sender, to, amount) {
            Ok(()) => Some(0),
            Err(
                native_token::TransferError::InsufficientBalance { .. }
                | native_token::TransferError::BalanceOverflow { .. },
            ) => None,
            Err(err) => panic!("invalid native balance: {err}"),
        },
        #[cfg(feature = "metered-burn")]
        Action::Burn => burn::execute(state, &public.root, meter.unwrap(), fee.gas_limit),
    };
    state
        .accounts
        .get_mut(&sender)
        .unwrap()
        .nonce
        .public_account_nonce_increment();
    let success = cycles.is_some();
    let gas_exec = cycles.unwrap_or(fee.gas_limit).min(fee.gas_limit);
    let fee_base = fee_actual_base(gas_exec, &view, opening);
    let tip = u128::from(fee.tip);
    let refund = reserved
        .checked_sub(fee_base.checked_add(tip).unwrap())
        .unwrap();
    if refund > 0 {
        transfer(state, inbox, sender, refund).expect("fee refund");
    }
    summary.gas_used_exec = accumulate_exec_gas(summary.gas_used_exec, gas_exec).unwrap();
    summary.gas_used_stor = accumulate_stor_gas(summary.gas_used_stor, gas_stor).unwrap();
    summary.revenue_base = summary.revenue_base.checked_add(fee_base).unwrap();
    summary.revenue_tip = summary.revenue_tip.checked_add(tip).unwrap();
    TransactionReceipt {
        success,
        gas_exec,
        gas_stor,
        fee_base,
        tip,
        refund,
    }
}

fn check_context(
    actual: &PublicExecutionContext,
    actors: impl IntoIterator<Item = Actor>,
    signers: impl IntoIterator<Item = AccountId>,
) {
    let expected = PublicExecutionContext::new(actors, signers);
    assert_eq!(
        borsh::to_vec(actual).unwrap(),
        borsh::to_vec(&expected).unwrap()
    );
}

fn check_system_tx(
    tx: &PublicTransaction,
    to: Actor,
    actors: impl IntoIterator<Item = Actor>,
    message: impl BorshSerialize,
) {
    assert_eq!(
        tx.root,
        RootCall {
            to,
            message: borsh::to_vec(&message).unwrap()
        }
    );
    check_context(&tx.public_context, actors, []);
    assert!(tx.fee.is_none() && tx.nonces.is_empty());
    assert!(tx.signatures.is_empty() && tx.admission_evidence.is_empty());
}

fn set_actor(state: &mut State, actor: Actor, data: lee_core::account::ActorState) {
    state
        .accounts
        .get_mut(&actor.account_id)
        .unwrap()
        .data
        .set_actor_state(actor.program_account_id, data);
}

fn transfer(
    state: &mut State,
    from: AccountId,
    to: AccountId,
    amount: Balance,
) -> Result<(), native_token::TransferError> {
    let invoke = |account, caller, message| {
        native_token::handle_message(&ReceiveInput {
            receiver: Actor::native_balance(account),
            from: caller,
            is_authorized: caller.is_none(),
            pre_state: state.accounts[&account]
                .data
                .actor_state(NATIVE_TOKEN_PROGRAM_ID)
                .clone(),
            message,
        })
    };
    let debit = invoke(
        from,
        None,
        borsh::to_vec(&native_token::Message::Transfer { to, amount }).unwrap(),
    )?;
    let credit = invoke(
        to,
        Some(debit.input.receiver),
        debit.response.async_messages[0].message.clone(),
    )?;
    set_actor(
        state,
        debit.input.receiver,
        debit.response.post_state.unwrap(),
    );
    set_actor(
        state,
        credit.input.receiver,
        credit.response.post_state.unwrap(),
    );
    Ok(())
}
