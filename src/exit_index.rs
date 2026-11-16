//! Local exit index. Authenticate the corresponding LEZ state before supplying its root.
use redemption_core::{Error, ExitTree, Hash, TREE_DEPTH, membership};

#[derive(Default)]
pub struct ExitIndex {
    tree: ExitTree,
    leaves: Vec<Hash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayError {
    PositionMismatch,
    UnrecognizedRoot,
    Tree(Error),
}

impl ExitIndex {
    /// Commit an ordered batch atomically once its root matches the applied LEZ state.
    /// A validation error leaves the prior tree and witness data intact.
    pub fn apply(
        &mut self,
        events: &[(u32, Hash)],
        accepted_root: Hash,
    ) -> Result<(), ReplayError> {
        let mut next = self.tree.clone();
        for &(position, leaf) in events {
            if next.append(leaf).map_err(ReplayError::Tree)? != position {
                return Err(ReplayError::PositionMismatch);
            }
        }
        if next.root() != accepted_root {
            return Err(ReplayError::UnrecognizedRoot);
        }
        self.leaves.extend(events.iter().map(|(_, leaf)| *leaf));
        self.tree = next;
        Ok(())
    }

    pub fn leaves(&self) -> &[Hash] {
        &self.leaves
    }

    pub fn membership(&self, position: u32) -> Result<(Hash, [Hash; TREE_DEPTH]), Error> {
        membership(&self.leaves, position)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_batch_does_not_advance_index() {
        let mut reference = ExitTree::default();
        let first = [0x34; 32];
        let second = [0x35; 32];
        let mut index = ExitIndex::default();
        reference.append(first).unwrap();
        index.apply(&[(0, first)], reference.root()).unwrap();

        reference.append(second).unwrap();
        assert_eq!(
            index.apply(&[(1, second)], [0; 32]),
            Err(ReplayError::UnrecognizedRoot)
        );
        assert_eq!(
            index.apply(&[(0, second)], reference.root()),
            Err(ReplayError::PositionMismatch)
        );
        assert_eq!(index.leaves(), &[first]);
        index.apply(&[(1, second)], reference.root()).unwrap();
        assert_eq!(index.membership(0).unwrap().0, reference.root());
        assert_eq!(index.membership(1).unwrap().0, reference.root());
    }
}
