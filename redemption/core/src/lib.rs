//! Exit commitments and the private redemption statement shared by host and zkVM.
use borsh::{BorshDeserialize, BorshSerialize};
use risc0_zkvm::sha::{Impl, Sha256};

pub type Hash = [u8; 32];
pub type Address = [u8; 20];
pub const TREE_DEPTH: usize = 32;
const CAPACITY: u64 = 1 << TREE_DEPTH;
pub const WITNESS_BYTES: usize = (5 * 32 + 2 * 20) + (3 * 20 + 2 * 16) + 32 + 4 + TREE_DEPTH * 32;

fn hash(bytes: &[u8]) -> Hash {
    Impl::hash_bytes(bytes).as_bytes().try_into().unwrap()
}

/// Full native state digest used by the historical checkpoint and native fixture exporter.
pub fn state_commitment(bytes: &[u8]) -> Hash {
    let mut encoded = b"RFP022/full-native-V03State/v1".to_vec();
    encoded.extend_from_slice(bytes);
    hash(&encoded)
}

fn encoded_hash(tag: &[u8], data: &impl BorshSerialize) -> Hash {
    let mut bytes = tag.to_vec();
    data.serialize(&mut bytes).expect("writing into a Vec");
    hash(&bytes)
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Pairing {
    pub logos_genesis: Hash,
    pub zone: Hash,
    pub bridge_program: Hash,
    /// Full EVM chain ID as a big-endian uint256.
    pub evm_chain_id: Hash,
    pub vault: Address,
    /// The underlying asset registered to this LEZ program. Zero denotes ETH.
    pub token: Address,
}

impl Pairing {
    pub fn id(&self) -> Hash {
        encoded_hash(b"RFP022/pairing/v1", self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Payout {
    /// Zero denotes native ETH. Other values are ERC-20 addresses.
    pub token: Address,
    pub amount: u128,
    pub recipient: Address,
    pub relayer: Address,
    pub fee: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    InvalidPayout,
    InvalidSecret,
    InvalidMembership,
    TreeFull,
    InvalidPosition,
}

impl Payout {
    fn validate(&self) -> Result<(), Error> {
        if self.recipient == [0; 20]
            || self.amount == 0
            || self.fee >= self.amount
            || ((self.fee == 0) != (self.relayer == [0; 20]))
        {
            return Err(Error::InvalidPayout);
        }
        Ok(())
    }
}

/// Sent to the LEZ burn program as its opaque `binding`. The secret stays in the wallet.
pub fn burn_binding(pairing: Hash, payout: &Payout, secret: Hash) -> Result<Hash, Error> {
    payout.validate()?;
    if secret == [0; 32] {
        return Err(Error::InvalidSecret);
    }
    Ok(encoded_hash(
        b"RFP022/redemption-note/v1",
        &(pairing, payout, secret),
    ))
}

/// Exactly the deployed burn actor's leaf encoding. `bridge_program` is the LEZ token
/// program account used by that actor, also included in the pairing ID.
pub fn burn_leaf(bridge_program: Hash, amount: u128, binding: Hash) -> Hash {
    encoded_hash(b"RFP022/exit/v1", &(bridge_program, amount, binding))
}

fn parent(left: Hash, right: Hash) -> Hash {
    let mut node = [0u8; 65];
    node[0] = 1;
    node[1..33].copy_from_slice(&left);
    node[33..].copy_from_slice(&right);
    hash(&node)
}

fn empty_nodes() -> [Hash; TREE_DEPTH + 1] {
    let mut empty = [[0; 32]; TREE_DEPTH + 1];
    empty[0] = hash(b"RFP022/empty-exit/v1");
    for level in 0..TREE_DEPTH {
        empty[level + 1] = parent(empty[level], empty[level]);
    }
    empty
}

/// Compact append-only Merkle state. All historical roots remain valid membership statements
/// for the leaves they cover. A source verifier must authenticate each accepted root.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ExitTree {
    count: u64,
    frontier: [Hash; TREE_DEPTH],
    root: Hash,
}

impl Default for ExitTree {
    fn default() -> Self {
        Self {
            count: 0,
            frontier: [[0; 32]; TREE_DEPTH],
            root: empty_nodes()[TREE_DEPTH],
        }
    }
}

impl ExitTree {
    pub fn root(&self) -> Hash {
        self.root
    }

    pub fn append(&mut self, leaf: Hash) -> Result<u32, Error> {
        let position = u32::try_from(self.count).map_err(|_| Error::TreeFull)?;
        let empty = empty_nodes();
        let mut node = leaf;
        let mut index = position;
        for (level, frontier) in self.frontier.iter_mut().enumerate() {
            node = if index & 1 == 0 {
                *frontier = node;
                parent(node, empty[level])
            } else {
                parent(*frontier, node)
            };
            index >>= 1;
        }
        self.count += 1;
        self.root = node;
        Ok(position)
    }
}

/// Rebuild a witness from the accepted leaves in canonical append order.
pub fn membership(leaves: &[Hash], position: u32) -> Result<(Hash, [Hash; TREE_DEPTH]), Error> {
    if u64::try_from(leaves.len()).unwrap_or(u64::MAX) > CAPACITY
        || position as usize >= leaves.len()
    {
        return Err(Error::InvalidPosition);
    }
    let empty = empty_nodes();
    let mut layer = leaves.to_vec();
    let mut index = position as usize;
    let mut siblings = [[0; 32]; TREE_DEPTH];
    for (level, sibling) in siblings.iter_mut().enumerate() {
        *sibling = layer.get(index ^ 1).copied().unwrap_or(empty[level]);
        layer = layer
            .chunks(2)
            .map(|pair| parent(pair[0], pair.get(1).copied().unwrap_or(empty[level])))
            .collect();
        index >>= 1;
    }
    Ok((layer[0], siblings))
}

#[derive(Debug, Clone, BorshSerialize, BorshDeserialize)]
pub struct Witness {
    pub pairing: Pairing,
    pub root: Hash,
    pub payout: Payout,
    pub secret: Hash,
    /// Exactly 32 bits, matching the tree depth. There are no unused high bits.
    pub position: u32,
    pub siblings: [Hash; TREE_DEPTH],
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Journal {
    pub pairing: Hash,
    pub root: Hash,
    pub nullifier: Hash,
    pub payout: Payout,
}

pub fn redeem(witness: &Witness) -> Result<Journal, Error> {
    if witness.payout.token != witness.pairing.token {
        return Err(Error::InvalidPayout);
    }
    let pairing = witness.pairing.id();
    let binding = burn_binding(pairing, &witness.payout, witness.secret)?;
    let leaf = burn_leaf(
        witness.pairing.bridge_program,
        witness.payout.amount,
        binding,
    );
    let mut node = leaf;
    let mut index = witness.position;
    for sibling in witness.siblings {
        node = if index & 1 == 0 {
            parent(node, sibling)
        } else {
            parent(sibling, node)
        };
        index >>= 1;
    }
    if node != witness.root {
        return Err(Error::InvalidMembership);
    }
    // Append-only order fixes a burn's hidden position across all later roots.
    let nullifier = encoded_hash(
        b"RFP022/release-nullifier/v1",
        &(pairing, leaf, witness.position, witness.secret),
    );
    Ok(Journal {
        pairing,
        root: witness.root,
        nullifier,
        payout: witness.payout.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Witness {
        let pairing = Pairing {
            logos_genesis: [1; 32],
            zone: [2; 32],
            bridge_program: [3; 32],
            evm_chain_id: [4; 32],
            vault: [5; 20],
            token: [0; 20],
        };
        let payout = Payout {
            token: pairing.token,
            amount: 40,
            recipient: [6; 20],
            relayer: [7; 20],
            fee: 1,
        };
        let secret = [8; 32];
        let binding = burn_binding(pairing.id(), &payout, secret).unwrap();
        let leaf = burn_leaf(pairing.bridge_program, payout.amount, binding);
        let (root, siblings) = membership(&[[0xa1; 32], leaf, [0xb2; 32]], 1).unwrap();
        Witness {
            pairing,
            root,
            payout,
            secret,
            position: 1,
            siblings,
        }
    }

    #[test]
    fn frontier_matches_independent_rebuild_at_each_prefix() {
        let mut tree = ExitTree::default();
        let mut leaves = Vec::new();
        for i in 0..130u32 {
            let leaf = hash(&i.to_le_bytes());
            assert_eq!(tree.append(leaf).unwrap(), i);
            leaves.push(leaf);
            for position in [0, i / 2, i] {
                assert_eq!(membership(&leaves, position).unwrap().0, tree.root());
            }
        }
        assert_eq!(membership(&leaves, 130), Err(Error::InvalidPosition));
    }

    #[test]
    fn replay_nullifier_survives_root_growth_without_pointing_to_leaf() {
        let mut witness = fixture();
        let older = redeem(&witness).unwrap();
        let note = burn_leaf(
            witness.pairing.bridge_program,
            witness.payout.amount,
            burn_binding(witness.pairing.id(), &witness.payout, witness.secret).unwrap(),
        );
        let leaves = [[0xa1; 32], note, [0xb2; 32], [0xc3; 32], note];
        (witness.root, witness.siblings) = membership(&leaves, 1).unwrap();
        let newer = redeem(&witness).unwrap();
        assert_ne!(older.root, newer.root);
        assert_eq!(older.nullifier, newer.nullifier);
        witness.position = 4;
        witness.siblings = membership(&leaves, 4).unwrap().1;
        assert_ne!(redeem(&witness).unwrap().nullifier, newer.nullifier);
        assert_eq!(borsh::to_vec(&witness).unwrap().len(), WITNESS_BYTES);
        for bytes in [&note[..], &witness.secret[..]] {
            assert!(
                !borsh::to_vec(&older)
                    .unwrap()
                    .windows(bytes.len())
                    .any(|window| window == bytes)
            );
        }
    }

    #[test]
    fn membership_rejects_changes_to_target_pair_and_private_witness() {
        let witness = fixture();
        assert!(redeem(&witness).is_ok());
        let mutations: [fn(&mut Witness); 12] = [
            |w| w.pairing.evm_chain_id[0] ^= 1,
            |w| w.pairing.vault[0] ^= 1,
            |w| w.pairing.logos_genesis[0] ^= 1,
            |w| w.pairing.zone[0] ^= 1,
            |w| w.pairing.bridge_program[0] ^= 1,
            |w| w.payout.token[0] ^= 1,
            |w| w.payout.recipient[0] ^= 1,
            |w| w.payout.relayer[0] ^= 1,
            |w| w.payout.amount += 1,
            |w| w.payout.fee += 1,
            |w| w.secret[0] ^= 1,
            |w| w.position ^= 1 << 31,
        ];
        for mutate in mutations {
            let mut changed = witness.clone();
            mutate(&mut changed);
            assert!(redeem(&changed).is_err());
        }
        let mut changed = witness;
        changed.root[0] ^= 1;
        assert_eq!(redeem(&changed), Err(Error::InvalidMembership));
    }
}
