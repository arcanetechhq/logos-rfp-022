use common::{
    HashType,
    block::{Block, HashableBlockData, TransactionReceipt},
    transaction::{FeePayee, LeeTransaction, clock_invocation, fee_invocation},
};
use lee::{GenesisBuilder, V03State};
use lee_core::account::{Account, AccountId};

pub fn build_block(
    state: &V03State,
    tip: Option<&chain_state::Tip>,
    txs: Vec<LeeTransaction>,
) -> Block {
    let id = tip.map_or(lee::GENESIS_BLOCK_ID, |t| t.block_id + 1);
    let timestamp = id * 10;
    let (summary, payout, receipts) = if tip.is_none() {
        (
            fee_core::BlockFeeSummary::default(),
            0,
            vec![TransactionReceipt::EXEMPT; txs.len()],
        )
    } else {
        chain_state::apply::derive_block_summary(state, &txs, id, timestamp)
            .expect("native settlement summary")
    };
    let mut all = txs;
    all.push(LeeTransaction::Public(fee_invocation(
        summary,
        payout,
        FeePayee::Present(AccountId::new([0x79; 32])),
    )));
    all.push(LeeTransaction::Public(clock_invocation(id, timestamp)));
    HashableBlockData {
        block_id: id,
        prev_block_hash: tip.map_or(HashType([0; 32]), |t| t.hash),
        timestamp,
        transactions: all,
        receipts,
    }
    .into_pending_block()
}
pub fn accept(
    state: &mut V03State,
    tip: &mut Option<chain_state::Tip>,
    block: &Block,
) -> Result<Vec<common::transaction::TxEvents>, chain_state::BlockIngestError> {
    let mut scratch = state.clone();
    let events = chain_state::apply_block(tip.as_ref(), block, &mut scratch)?;
    *state = scratch;
    *tip = Some(chain_state::Tip::from(block));
    Ok(events)
}
pub fn seeded_state(
    token: AccountId,
    program: lee::program::Program,
    exit_account: AccountId,
    exit_state: Option<Vec<u8>>,
    owner: AccountId,
    holding: Vec<u8>,
) -> V03State {
    GenesisBuilder::new()
        .with_programs_at([
            (token, program, true),
            (system_accounts::fee_program_id(), programs::fee(), true),
            (programs::clock_account_id(), programs::clock(), true),
        ])
        .with_public_accounts([
            (AccountId::new([0x79; 32]), Account::default()),
            (
                exit_account,
                exit_state.map_or_else(Account::default, |bytes| {
                    Account::default().with_actor_state(token, bytes.into())
                }),
            ),
            (
                system_accounts::clock_account_id(),
                Account::default().with_actor_state(
                    programs::clock_account_id(),
                    clock_core::ClockAccountData {
                        block_id: 0,
                        timestamp: 0,
                    }
                    .to_bytes()
                    .into(),
                ),
            ),
            (system_accounts::fee_escrow_account_id(), Account::default()),
            (system_accounts::fee_inbox_account_id(), Account::default()),
            (
                owner,
                Account::funded(1_000_000_000_000).with_actor_state(token, holding.into()),
            ),
            (
                system_accounts::fee_state_account_id(),
                Account::default().with_actor_state(
                    system_accounts::fee_program_id(),
                    fee_core::state::FeeState::genesis().to_bytes().into(),
                ),
            ),
        ])
        .build()
}
