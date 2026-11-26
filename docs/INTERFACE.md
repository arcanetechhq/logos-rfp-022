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

## From the action proof to a source checkpoint

1. **Bind the action to native block bytes.** The block-link guest verifies the archived action receipt under its pinned image. It recomputes the native LEZ block hash and matches the proved transaction, block number and timestamp to the payload. Its journal binds that result to the exact inscription bytes and the action's `exit_root`. The supported shape has one private user transaction and two trailing public transactions. The included success flag is a claim in the block. The guest only proves the restricted private transition and its inclusion.
2. **Prove execution of the whole LEZ block.** Reuse native settlement rules in a proving environment, including fee and clock transactions, receipts and ordered application to the state from the preceding verified checkpoint. Verify program execution receipts against the program images resolved from that state. Compare complete state and the exit accumulator with native `apply_block` on successful and reverted transactions. Prove one block before composing successive transitions. Use an authenticated incremental state witness before extrapolating proof costs to production.
3. **Bind execution to a canonical Logos checkpoint.** Authenticate both the Logos checkpoint and the ordered zone payloads under it. The composed proof must derive the LEZ state and exit root from those payloads and the previous accepted checkpoint. The native checkpoint format and finality rule remain open. An explicitly labelled fixture can exercise the composition until the source network provides an authenticated input.
4. **Verify on Ethereum.** Keep the local fixture separate from the production vault. Accept a burn on the selected branch, reject a valid burn from the displaced branch, and reject a duplicate payout. Measure proof latency, the age of the checkpoint at payout and Ethereum gas.

The runner reads the archived native block and action receipt from `evidence/`. Run from the repository root.

```sh
BOOTSTRAP_ONLY=1 ./reproduce.sh
work="$(cd ../.. && pwd)/work"
export RISC0_HOME="$work/risc0" RISC0_SERVER_PATH="$work/tools/r0vm"
export RISC0_EXECUTOR=ipc RISC0_PROVER=ipc RISC0_DEV_MODE=0 RISC0_BUILD_LOCKED=1
export RUSTC_WRAPPER="$PWD/scripts/rustc-remap.sh" RUSTFLAGS=--cap-lints=allow
cargo +1.98.1 test --locked --test block_link
cargo +1.98.1 build --locked --features checkpoint --bin block-checkpoint
target/debug/block-checkpoint execute
target/debug/block-checkpoint prove "$work/block-link-receipt.borsh"
target/debug/block-checkpoint verify "$work/block-link-receipt.borsh"
```

The local run on 25 November 2026 verified a Composite receipt with dev mode disabled. The link guest used 4,267,896 user cycles. Proving and verifying the new receipt took about 11.6 minutes after the archived action receipt was checked. The receipt is 12,812,510 bytes and includes the action receipt as an assumption. These measurements cover the link guest and its verification, with Groth16 wrapping reserved for the composed checkpoint proof.

## Bounded block settlement

`settlement-guest` verifies a complete non-genesis block of charged native-token transfers between existing public accounts. It recomputes the native transaction and block commitments, checks Schnorr authorization and ordered nonces, and derives fee reserve, charge, refund, distribution, clock tick and receipts. A successful built-in transfer uses zero program cycles. A chargeable transfer failure uses its signed gas limit and advances the sender's nonce without taking the transfer. The fixture exercises both outcomes in one block and rejects a changed receipt, reordered signed transactions and a missing fee invocation.

The witness opens the accounts accessed by execution and carries a digest for each other account. The resulting commitment covers the address-indexed account store and the full native private tree, nullifiers, publications and recovery bindings. Its domain differs from the earlier private-action snapshot digest. The host reconstructs the whole post-state from the witness and checks its native Borsh bytes against `apply_block`. Omitting untouched executable segments from the witness reduced the fixture to about 11.9 million user cycles from 31.9 million with full bytecode.

The proof requires a predecessor state and block hash authenticated elsewhere. It reads the unchanged exit accumulator from that state after two native burns in the fixture. The guest pins the immutable fee and clock images and supports the corresponding distribution, refund and tick paths. It excludes upgrade registries, arbitrary public programs and private transactions. For arbitrary programs, the guest needs metering bound to a verifiable execution trace. A host-supplied RISC Zero cycle counter cannot bind native receipts.

With the preceding RISC Zero environment configured, run:

```sh
cargo +1.98.1 build --locked --features settlement --bin block-settlement
target/debug/block-settlement execute
target/debug/block-settlement prove "$work/settlement-receipt.borsh"
target/debug/block-settlement verify "$work/settlement-receipt.borsh"
```

On 26 November 2026, the mixed block used 11,925,317 user cycles. The CPU run generated and verified a 3,644,782-byte Composite receipt in 30 minutes and 17 seconds with dev mode disabled. The follow-up block used 433,497 user cycles and produced a 281,914-byte Composite receipt that verified independently.

### Metered wrapped burn inside a block

The separate `burn-block` image settles the existing wrapped-burn program under its immutable LEZ loader image ID. It authenticates the original actor and kernel memory, replays their RV32IM instructions, and counts the same user cycles and syscall costs that LEZ bills. The supported syscall paths use the LEZ executor's empty guest environment and framed input. Unread random bytes are tracked as unknown, and attempting to read one invalidates the witness. The interpreter rejects instructions and calls outside its supported execution model.

The fixture starts at an empty exit tree. Two signed transactions in the same non-genesis block burn 40 wrapped units and then attempt to burn 61 from the remaining 60. The successful debit and append consume **120,120 user cycles**, with base fee **965,304** and refund **7,039,040**. The overspend consumes its full **1,000,000-cycle** signed budget, with base fee **8,004,344** and zero refund. Both advance the owner's nonce. The guest checks both native receipts, the closing fee distribution and clock tick, and derives the new exit root. The entire reconstructed state matches `apply_block` byte for byte. An additional differential case checks that a mispaired append rolls back the staged debit and keeps the fee. Altering the metering result in a rehashed block or substituting the actor memory is rejected.

With the RISC Zero environment above, run:

```sh
run="$work/burn-block"
mkdir -p "$run"
cargo +1.98.1 build --locked --features burn-block --bin burn-block --bin burn-meter-check
target/debug/burn-meter-check "$run/burn-image.bin"
target/debug/burn-block check
target/debug/burn-block execute
target/debug/burn-block prove "$run/burn-block-receipt.borsh"
target/debug/burn-block verify "$run/burn-block-receipt.borsh"
```

The scoped guest execution used 34,562,494 user cycles. The CPU run generated and checked a 10,397,014-byte Composite receipt in 1 hour, 52 minutes and 44 seconds. A separate invocation verified it with dev mode disabled. The program and its metering rules are pinned. Production still requires an authenticated predecessor and a proof of publication in finalized Logos history.

## Chained settlement and local Logos replay

The composition guest verifies two receipts for the pinned settlement image. It requires the second block to extend the first by block hash, state digest and height under the same pairing. The composed journal commits to both exact payload digests in execution order and to the resulting exit root. In the fixture, the second block advances the clock and distributes the smoothed payout without user transactions.

The native Logos fixture publishes those two payloads in consecutive Mantle inscriptions on one branch. A competing branch carries the archived private burn block. The native replay checks header signatures, block bodies, leadership proofs, transaction proofs and the channel state on each branch. After fork choice displaces the earlier branch, the fixture checks both payloads against the selected channel's message chain. The host checks that exact order against the composed proof.

The composition guest used 30,544 user cycles. The verified Composite receipt is 4,148,012 bytes and includes both settlement receipts. The outer proving and verification took about 22 seconds after checking the input receipts. Swapping the two replay payloads fails the binding check with the same valid Composite receipt.

Use the same environment as above to reproduce the local composition:

```sh
run="$work/block-settlement"
mkdir -p "$run"
cargo +1.98.1 build --locked --features settlement --bin block-settlement
target/debug/block-settlement payload "$run/selected-payload.borsh"
target/debug/block-settlement next-payload "$run/next-payload.borsh"
target/debug/block-settlement prove "$run/receipt.borsh"
target/debug/block-settlement next-prove "$run/next-receipt.borsh"
target/debug/block-settlement chain-prove "$run/receipt.borsh" "$run/next-receipt.borsh" "$run/chain-receipt.borsh"
RFP022_SETTLEMENT_PAYLOAD="$run/selected-payload.borsh" \
RFP022_SETTLEMENT_NEXT_PAYLOAD="$run/next-payload.borsh" \
RFP022_REPLAY_FIXTURE="$run/replay.json" \
  cargo +1.98.1 test --locked --manifest-path native-auth/Cargo.toml \
  exports_selected_lez_payloads -- --ignored --nocapture
target/debug/block-settlement bind-fixture "$run/chain-receipt.borsh" "$run/replay.json" "$run/evm-checkpoint.json"
```

Here the predecessor and replay output are local fixture inputs. Production needs an authenticated predecessor, a specified bridge-verifiable Logos finality rule and a proof that connects the accepted Mantle history to those exact LEZ payloads.

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

The Ethereum vault implements the [optimistic contest](REDEMPTION.md#optimistic-source-checkpoints) as a fallback with an explicit observation assumption. The host comparator checks two supplied histories within a common slot bound using the pinned Cryptarchia bootstrap engine. An on-chain comparison guest still needs a precise rule for observation timing, data availability, bootstrap and repeated replacements before its image can be pinned. The native checkpoint remains the preferred production design.

## Concrete requests to Logos

1. Stable genesis, era schedule and complete deterministic ledger + Cryptarchia replay/serialization APIs, including UTXO roots, epoch state, uncle validation, transaction proofs and channel configuration history; supported zkVM compilation target and test histories.
2. A specified bridge-verifiable finality/initialization rule: genesis bootstrap, network/time assumptions, stale/offline recovery, branch competition, canonicality and any new monitor assumption. A sequencer signature on final tip is insufficient.
3. LEZ acceptance export built at the actual commit point after native block validation, not publication: full state commitment and accepted exit root, code/upgrade identity, receipts/fee/clock, nullifiers and historical commitment roots. A checkpoint guest must re-execute or verify this transition.
4. Agreement whether this is an off-chain zk replay primitive or Native Zone STF enforcement. The latter is a protocol change still marked incomplete in the pinned roadmap; merely adding a header root is not enforcement.

Native replay can in principle derive missing roots. Its cost and canonicality construction remain research obligations; this prototype does not establish impossibility or acceptable full-history costs.

## Execution cost experiment

The initial guest used `env::read::<Vec<u8>>()`. A pprof run attributed about 83% of cumulative cycles to deserializing and reading that byte vector. The final guest uses native `lee_core::program::read_input_frame`, paired with host `write_slice(to_frame(...))`, preserving the same Borsh payload and statement. User cycles fell from 208,006,046 to 26,574,665; input remained 1,229,647 bytes. The exact native crypto-bigint patch was also evaluated and retained for dependency alignment, but on its own it did not change the cycle count. No custom circuit was introduced. Baseline execution/profiling is not a completed baseline proof.

This full-snapshot reference has O(state size) I/O/hashing costs and is not a production incremental state commitment design. It is a measured correctness seam, not evidence that full native source-history replay fits a particular latency or budget.
