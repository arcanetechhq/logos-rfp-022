# Redemption code and verification boundary

The bridge code now has an incremental native LEZ burn actor, a private ZK redemption statement, and an immutable one-asset EVM vault. The original v1 experiment remains runnable against its pinned release and receipts. The incremental actor is a new image and requires an initialized pairing in LEZ state.

## Check the code

Run `./verify-redemption.sh` from this repository. It builds the pinned guests, checks Merkle paths and payout bindings, executes two burns through native `apply_block`, and runs the vault tests in a local EVM with the released RISC Zero v3.1.0 Groth16 verifier. The checks include an actual reverted checkpoint submission. The Solidity decoder reads the exact 188-byte journal from the Rust guest. Node.js/npm and the prerequisites of the original bootstrap script are required.

`VERIFY_NATIVE_V2_PROOF=1 ./verify-redemption.sh` also proves the redemption statement against the root extracted after both burns. Set `REUSE_REDEMPTION=1` to reverify a saved receipt. The v2 fixture uses public burns and synthetic genesis. `VERIFY_NATIVE_REDEMPTION=1` uses the historical actor and runs the separate genuine private PPE burn case with the same commitment format (set `REUSE_PROOFS=1` to use saved PPE receipts). All addresses and secrets in these fixtures are publicly known test values.

## State and claims

1. The LEZ actor debits the holder and synchronously appends a leaf. The exit PDA's initialized pairing and native program ownership are checked at the append. Invalid appends and overspends revert the debit. The public PDA stores a 32-level frontier, count and root in constant space. The append event contains the position and leaf so any wallet can reconstruct paths.
2. A wallet binds its payout to one pairing (Logos genesis, zone, bridge program, EVM chain ID, vault, underlying asset), amount, recipient and relayer fee using a high-entropy wallet secret. The burn records the commitment. The redemption guest proves membership and commits the pairing, root, nullifier and payout. The path, index and secret stay out of the journal. The nullifier for one leaf position is stable across later roots. The actor accepts a user-supplied commitment, so the wallet must construct and persist or recover it correctly before burning.
3. The vault pins both verifier image IDs and the verifier contract. A source-history proof must advance from its current checkpoint digest, with a strictly increasing finalized slot, and identify the accepted exit root. Previously authenticated roots remain recognized. The source journal is Ethereum ABI encoding of `(bytes32 pairing, bytes32 previous, bytes32 next, bytes32 exitRoot, uint64 finalizedSlot)`; the contract passes the SHA-256 digest to the verifier.
4. The vault escrows ETH or one ERC-20. It rejects tokens that move a different amount from the amount in the call. Once live, a release verifies its redemption journal against an authenticated root and atomically consumes the nullifier and pays out. A payout revert rolls back consumption and accounting. The vault cannot start accepting deposits until the source verifier has admitted a checkpoint.

## Remaining bridge work

The source-history guest and its authenticated production initialization are still missing. That guest must prove the pinned Logos finality and canonicality rules, the LEZ block transitions (including revert handling), and the immutable bridge program plus its pairing and append-only exit state. The local snapshots are diagnostic exports. They cannot authorize an Ethereum root. The local vault test deploys with source verification disabled or with a verifier that fails all supplied source proofs. No test pretends to have finality, and no test moves funds through a fabricated root.

The escrow sides still need the RFP-021 consumer and atomic mint, wallet recovery, deployment and migration procedure, operational controls and a full integration review. The current actor starts with synthetic balances. A target deployment must supply an audited wrapped-token issuance path and a corresponding verified deposit statement. The one-asset vault is a building block for the registry and fee/cap policy.

The local EVM uses [RISC Zero Ethereum verifier v3.1.0](https://github.com/risc0/risc0-ethereum/blob/ff30da77ba4afdd496290ebebedb205348b15d83/contracts/src/groth16/RiscZeroGroth16Verifier.sol) as a separate contract and [OpenZeppelin SafeERC20 v5.4.0](https://github.com/OpenZeppelin/openzeppelin-contracts/blob/v5.4.0/contracts/token/ERC20/utils/SafeERC20.sol) for optional-return tokens.
