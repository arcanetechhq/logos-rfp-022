//! Fail-closed consumer boundary. No value-transfer operation is implemented.
use acceptance_core::Journal;
use risc0_zkvm::{InnerReceipt, Receipt};
#[derive(Debug, PartialEq, Eq)]
pub enum Rejection {
    FakeReceipt,
    InvalidActionProof,
    InvalidJournal,
    SourceHistoryVerifierUnavailable,
}
/// All callers may submit proofs. No RPC result, operator signature or depth is accepted.
pub struct ReleaseGate {
    action_image: [u32; 8],
}
impl ReleaseGate {
    pub fn new(action_image: [u32; 8]) -> Self {
        Self { action_image }
    }
    pub fn authorize(&self, receipt: &Receipt) -> Result<Journal, Rejection> {
        if matches!(receipt.inner, InnerReceipt::Fake(_)) {
            return Err(Rejection::FakeReceipt);
        }
        receipt
            .verify(self.action_image)
            .map_err(|_| Rejection::InvalidActionProof)?;
        let _journal: Journal =
            borsh::from_slice(&receipt.journal.bytes).map_err(|_| Rejection::InvalidJournal)?;
        // Implementing an actual verifier for authenticated canonical source history is required
        // before this branch can be replaced. A configurable acceptance Boolean is absent.
        Err(Rejection::SourceHistoryVerifierUnavailable)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_fake_receipt_before_any_anchor_processing() {
        let fake = Receipt::new(
            InnerReceipt::Fake(risc0_zkvm::FakeReceipt::new(risc0_zkvm::ReceiptClaim::ok(
                [0u32; 8],
                Vec::<u8>::new(),
            ))),
            Vec::new(),
        );
        assert_eq!(
            ReleaseGate::new([0u32; 8]).authorize(&fake),
            Err(Rejection::FakeReceipt)
        );
    }
}

/// Demonstrates chaining from a synthetic fixture digest. This cannot create a source anchor.
pub struct FixtureActionHead([u8; 32]);
impl FixtureActionHead {
    pub fn from_fixture(digest: [u8; 32]) -> Self {
        Self(digest)
    }
    /// Caller supplies the journal only after cryptographic receipt verification.
    pub fn advance(&mut self, journal: &Journal) -> Result<(), &'static str> {
        if journal.before != self.0 {
            return Err("unknown predecessor or replay");
        }
        self.0 = journal.after;
        Ok(())
    }
}
