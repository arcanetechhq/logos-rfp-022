//! Validate the supplied Logos blocks. The selected branch is local to this
//! replay and must never authorize an Ethereum checkpoint.

use std::sync::Arc;

use lb_cryptarchia_engine::{Cryptarchia, Slot, State};
use lb_ledger::{
    Ledger, LedgerError, LedgerState,
    config::{EraScheduledConfig, config_for_slot},
};
use logos_blockchain_core::{
    block::{Block, BlockTransactions},
    events::Events,
    header::{Header, HeaderId},
    mantle::{
        ledger::verification_mode::StandardMode,
        transactions::{GenesisTx, SignedOps, states::Preverified},
    },
};

pub type SourceBlock = Block<SignedOps<Preverified, StandardMode>>;

#[derive(Debug, thiserror::Error)]
pub enum ReplayError {
    #[error("genesis transaction does not match the expected header")]
    WrongGenesis,
    #[error("block slot is later than the replay observation slot")]
    FutureSlot,
    #[error("replay does not support blocks carrying uncle references")]
    UnsupportedUncles,
    #[error("block {0:?} was already applied")]
    AlreadyApplied(HeaderId),
    #[error("block exceeds the transaction count for its era")]
    TransactionCount,
    #[error("invalid block: {0}")]
    Block(#[from] logos_blockchain_core::block::Error),
    #[error("ledger rejected block: {0}")]
    Ledger(#[from] LedgerError<HeaderId>),
    #[error("ledger batch verification rejected block: {0}")]
    Batch(#[from] logos_blockchain_core::mantle::batch::Error),
    #[error("consensus rejected block: {0}")]
    Consensus(#[from] lb_cryptarchia_engine::Error<HeaderId>),
}

/// Replays an imported history from a locally configured genesis and era
/// schedule. Imported blocks cannot attest that an online node saw all
/// competing branches, so this replay stays in native bootstrap mode.
pub struct SourceReplay {
    ledger: Ledger<HeaderId>,
    consensus: Cryptarchia<HeaderId>,
}

/// Result of validating one supplied block. Events belong to that block even
/// if fork choice did not select it. All branch changes are provisional.
#[derive(Debug)]
pub struct ReplayUpdate {
    pub block_id: HeaderId,
    pub events: Events,
    pub removed_from_selected_branch: Vec<HeaderId>,
    pub added_to_selected_branch: Vec<HeaderId>,
}

impl SourceReplay {
    pub fn from_genesis(
        expected_genesis: HeaderId,
        genesis: GenesisTx,
        eras: Arc<EraScheduledConfig>,
    ) -> Result<Self, ReplayError> {
        if Header::genesis(&genesis).id() != expected_genesis {
            return Err(ReplayError::WrongGenesis);
        }
        let (state, _) = LedgerState::from_genesis_tx(genesis, eras.genesis().parameters)?;
        let consensus_eras = eras.map(|era| era.parameters.consensus_config.clone());
        Ok(Self {
            ledger: Ledger::new(expected_genesis, state, eras),
            consensus: Cryptarchia::from_genesis(
                expected_genesis,
                Arc::new(consensus_eras),
                State::Bootstrapping,
            ),
        })
    }

    #[must_use]
    pub fn selected_tip(&self) -> HeaderId {
        self.consensus.tip()
    }

    /// `observation_slot` is a caller-supplied upper bound for this offline
    /// replay. It cannot advance finality or move the replay online.
    pub fn apply(
        &mut self,
        block: SourceBlock,
        observation_slot: Slot,
    ) -> Result<ReplayUpdate, ReplayError> {
        let header = block.header();
        let slot = header.slot();
        if slot > observation_slot {
            return Err(ReplayError::FutureSlot);
        }
        if !block.uncle_headers().is_empty() {
            return Err(ReplayError::UnsupportedUncles);
        }
        let block_id = header.id();
        if self.ledger.state(&block_id).is_some() {
            return Err(ReplayError::AlreadyApplied(block_id));
        }
        let config = config_for_slot(self.ledger.era_schedule(), slot);
        let header = header.clone();
        let signature = *block.signature();
        let uncle_headers = block.uncle_headers().clone();
        let transactions =
            BlockTransactions::try_new(block.into_transactions(), config.max_transactions)
                .map_err(|_| ReplayError::TransactionCount)?;
        let block = Block::reconstruct(
            header,
            uncle_headers,
            transactions,
            signature,
            config.max_transactions_size,
        )?;

        let header = block.header();
        let parent = header.parent();
        let uncle_slots = block.uncle_headers().slots();
        let prepared = self.ledger.prepare_update(
            block_id,
            parent,
            slot,
            header.leader_proof(),
            &uncle_slots,
            block.transactions_iter().cloned(),
        )?;
        let verified = prepared.verify_batch_proofs()?;
        let outcome = self.consensus.receive_block_with_canonical_change(
            block_id,
            parent,
            slot,
            uncle_slots,
        )?;
        let events = self.ledger.commit_update(verified);
        Ok(ReplayUpdate {
            block_id,
            events,
            removed_from_selected_branch: outcome.reorged_blocks.iter().copied().collect(),
            added_to_selected_branch: outcome.newly_canonical_blocks,
        })
    }
}

#[cfg(test)]
mod tests;
