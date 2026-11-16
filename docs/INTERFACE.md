# Experimental acceptance/checkpoint boundary (not a deployed protocol)

The implementation adds no header field and changes no Logos/LEZ consensus rule. `acceptance-core` is an executable reference for one restricted native private action. Native `apply_block` remains the block-level oracle. A new state digest is an off-chain proof commitment, not a claim that Logos currently commits to that digest.

## What the fragment verifies

1. A non-fake RISC Zero 3.1 native PPE receipt, bound to the exact PPE image and its original Borsh-framed journal (`public_context`, `ProvenExecution`). The checkpoint guest resolves this receipt through `env::verify`; its proof is not replaced with a Boolean.
2. A byte-compatible complete `V03State` snapshot. Its commitment includes public accounts, deployed program storage, the actual native Merkle-tree representation, commitment positions, root history, spent nullifiers, publications and recovery bindings. Roots alone omit security-relevant state.
3. The live immutable TOKEN loader header and the fixed burn guest image; signature/nonces, authorized accounts, validity window, no duplicate/new commitments, no spent nullifiers, known input roots.
4. The actual native `lee_core::execution_state::PublicPart`, with a restricted backend that executes the same `burn-core::handle` Rust function as the native token guest. It admits only the public exit sink. Async messages, recovery bindings, undisclosed code, other programs and fee fallbacks are rejected.
5. Updated public state, signer nonces, native commitment Merkle tree, root history and nullifier set. Host checks the entire serialized result against `ValidatedStateDiff::from_privacy_preserving_transaction` + `V03State::apply_state_diff`, not just the sink balance.

Output: `Journal { before, after, exit_root, tx_hash, block, timestamp }`. Hashes are SHA-256 with specified domains in source. `tx_hash` uses the real native transaction bytes. The experiment's list accumulator is sufficient to demonstrate accepted exits; it is not a scalable private membership construction.

The fragment does NOT execute fee reserve/refund, block summary, clock, system upgrades, all program dispatch or Logos consensus in the guest. Full native `apply_block` does execute fee/clock and receipts independently. Its result must not be relabelled as a proof of the whole block.

## Required production source-history interface

A permissionless history prover should produce a proof whose public journal commits, at minimum, to:

```
version + chain/genesis ID + era/rules/guest IDs
previous verified source checkpoint digest
next source ledger/consensus checkpoint digest
bootstrap/online state + slot/time bounds + native LIB header ID
channel ID + authenticated channel config/key history + final message tip
accepted LEZ tip hash + complete LEZ state commitment
common exit accumulator root/window
```

The guest must derive these values from authenticated state, not receive each as an independent operator assertion. The consumer must pin rules/image IDs, verify the proof, require continuity with its previous verified checkpoint and check both domain and source/LEZ linkage. The full state commitment must include root history, nullifiers, code/upgrade state, fee/clock and message state; a balances-only root is insufficient.

A user proof would consume the SAME accepted common exit root/window and prove secret possession, membership, token/amount and destination/fee/chain/vault binding, producing a release nullifier. Publishing `leaf` or a membership index on Ethereum would reveal the selected burn, so this experiment deliberately does not provide such a release path. Burn `binding` bytes here are public synthetic data, not a completed user-privacy proof.

## Initialization and canonicality

- **Current experiment:** synthetic genesis with 100 units and deterministic test keys. Native execution builds the input state; differential checks establish consistency. These keys and balances are not production trust anchors. The fragment receipt proves a transition relative to its predecessor commitment. It does not certify the predecessor's origin.
- **Genesis initialization:** pin the native genesis/config/rules, derive initial state within proof, then prove all necessary executed history, including era transitions and leadership inputs. Ordinary headers do not supply latest ledger roots.
- **Trusted checkpoint provider:** native bootstrap permits this, but trusting an additional provider for the starting ledger/LIB violates the requested strict bridge model unless an independently verified on-Ethereum anchor already authenticates it. A checkpoint root supplied by the prover is insufficient.
- **Fork choice:** one valid branch, even with native PoL and depth k, does not prove canonicality. Native bootstrap compares branches and has network/prolonged-bootstrap assumptions; online LIB depends on successful initialization. A prover cannot prove it saw every relevant competing fork simply by submitting a transcript.

## Native source replay

`native-auth::source_replay::SourceReplay` rebuilds the ledger from a native genesis transaction and checks the genesis header against the locally expected ID. The era schedule is local configuration. For each supplied block it checks the native body commitment and header signature, applies the ledger and its Proof of Leadership and transaction proofs, and runs the native bootstrap fork choice. Ledger state is retained by block ID, so each branch uses its own parent state. The result identifies any blocks removed or adopted by fork choice. Applied events identify their block and can be provisional. The replay rejects blocks carrying uncles until it can reuse the native uncle validation path.

The regression test uses a local fixture chain with genuine Logos Groth16 leadership proofs. It includes an inscription containing the bytes of the existing LEZ burn block, verifies the resulting Mantle channel tip and checks that the inscription disappears from the selected history when the competing branch takes over. Starting the engine online directly in the test produces different LIBs for the isolated and expanded views. The public replay API stays in bootstrap and returns no LIB or source checkpoint.

Ethereum still needs a source-finality rule that authenticates canonical Logos history under the RFP trust model. A zk proof of this replay could prove that the supplied blocks execute and that fork choice selected one of them. The verifier would still need an authenticated basis for treating that selection as final, such as a protocol-native commitment to a finalized header. Any construction based on another observation or challenge mechanism needs an explicit security argument for its synchronization and participation assumptions. After that decision, the source guest must also connect the finalized header to executed LEZ state and its exit root. The vault requires that proof before it can release funds.

## Concrete requests to Logos

1. Stable genesis, era schedule and complete deterministic ledger + Cryptarchia replay/serialization APIs, including UTXO roots, epoch state, uncle validation, transaction proofs and channel configuration history; supported zkVM compilation target and test histories.
2. A specified bridge-verifiable finality/initialization rule: genesis bootstrap, network/time assumptions, stale/offline recovery, branch competition, canonicality and any new monitor assumption. A sequencer signature on final tip is insufficient.
3. LEZ acceptance export built at the actual commit point after native block validation, not publication: full state commitment and accepted exit root, code/upgrade identity, receipts/fee/clock, nullifiers and historical commitment roots. A checkpoint guest must re-execute or verify this transition.
4. Agreement whether this is an off-chain zk replay primitive or Native Zone STF enforcement. The latter is a protocol change still marked incomplete in the pinned roadmap; merely adding a header root is not enforcement.

Native replay can in principle derive missing roots. Its cost and canonicality construction remain research obligations; this prototype does not establish impossibility or acceptable full-history costs.

## Execution cost experiment

The initial guest used `env::read::<Vec<u8>>()`. A pprof run attributed about 83% of cumulative cycles to deserializing and reading that byte vector. The final guest uses native `lee_core::program::read_input_frame`, paired with host `write_slice(to_frame(...))`, preserving the same Borsh payload and statement. User cycles fell from 208,006,046 to 26,574,665; input remained 1,229,647 bytes. The exact native crypto-bigint patch was also evaluated and retained for dependency alignment, but on its own it did not change the cycle count. No custom circuit was introduced. Baseline execution/profiling is not a completed baseline proof.

This full-snapshot reference has O(state size) I/O/hashing costs and is not a production incremental state commitment design. It is a measured correctness seam, not evidence that full native source-history replay fits a particular latency or budget.
