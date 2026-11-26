# Redemption code and verification boundary

The bridge code now has an incremental native LEZ burn actor, a private ZK redemption statement, and an immutable one-asset EVM vault. The original v1 experiment remains runnable against its pinned release and receipts. The incremental actor is a new image and requires an initialized pairing in LEZ state.

## Check the code

Run `./verify-redemption.sh` from this repository. It builds the pinned guests, checks Merkle paths and payout bindings, executes two burns through native `apply_block`, and runs the vault tests in a local EVM with the released RISC Zero v3.1.0 Groth16 verifier. The checks include an actual reverted checkpoint submission. The Solidity decoder reads the exact 188-byte journal from the Rust guest. Node.js/npm and the prerequisites of the original bootstrap script are required. Each run saves its outputs in a new `work/redemption-verification.*` directory.

`VERIFY_NATIVE_V2_PROOF=1 ./verify-redemption.sh` also proves the redemption statement against the root extracted after both burns. Set `REUSE_REDEMPTION=1` to reverify a saved receipt. The v2 fixture uses public burns and synthetic genesis. `VERIFY_NATIVE_REDEMPTION=1` uses the historical actor and runs the separate genuine private PPE burn case with the same commitment format (set `REUSE_PROOFS=1` to use saved PPE receipts). All addresses and secrets in these fixtures are publicly known test values.

Fresh guest builds in the verification scripts remap local source paths. The redemption guest and its core share a Cargo workspace so their crate identities stay stable across checkouts. The saved v2 redemption receipt is still bound to its archived image ID. When that ID differs from the current build, the runner verifies the receipt against the committed archived ID, executes the rebuilt guest on the same witness and checks that both produce the same journal. The original PPE and checkpoint replay settings use a separate `target/legacy` build cache and still require the original image IDs.

The v2 receipt also verified on the local EVM after wrapping to a 260-byte Groth16 seal. The official verifier used **251,671 gas** for the transaction calling it directly. Changing the journal or image ID failed, and the same valid seal could not advance the vault's source checkpoint. `VERIFY_NATIVE_V2_PROOF=1 REUSE_REDEMPTION=1 VERIFY_GROTH16=1 ./verify-redemption.sh` verifies the saved evidence without Docker. To regenerate the seal, also set `WRAP_GROTH16=1` and run the supported RISC Zero Docker image. On the tested macOS arm64 host the amd64 container needed `DOCKER_DEFAULT_PLATFORM=linux/amd64 LIMIT_WRAP_MEMORY=1`.

After `BOOTSTRAP_ONLY=1 ./reproduce.sh`, run `RUSTFLAGS=--cap-lints=allow cargo +1.98.1 test --locked --manifest-path native-auth/Cargo.toml --lib` to check Logos source replay with native leadership proofs, a real LEZ block inscription on a fixture chain, and competing valid branches.

### Local release from the selected history

Run the [chained settlement and local Logos replay](INTERFACE.md#chained-settlement-and-local-logos-replay) to get `evm-checkpoint.json`. Then run this from the repository root:

```sh
RFP022_EVM_CHECKPOINT="$run/evm-checkpoint.json" VERIFY_GROTH16=1 ./verify-redemption.sh
```

The Ganache fixture pins the expected source checkpoint digest and delegates user proof verification to the released Groth16 verifier. It runs the real vault constructor at the address committed in the archived redemption proof. The test rejects the displaced checkpoint and checks that a valid user proof cannot spend a root absent from the authorized history. With the selected root authorized, a funded vault pays the proof's recipient and relayer. A mined duplicate reverts and leaves both balances intact. This harness measures gas for the genuine user proof and payout. The source checkpoint is a fixture input, so its gas and the interval to payout describe the local harness.

The local run on 26 November 2026 used **373,518 gas** for the release with its Groth16 verification and **79,567 gas** to admit the fixture checkpoint. The release was mined two local blocks after the checkpoint.

## State and claims

1. The LEZ actor debits the holder and synchronously appends a leaf. The exit PDA's initialized pairing and native program ownership are checked at the append. Invalid appends and overspends revert the debit. The public PDA stores a 32-level frontier, count and root in constant space. The append event contains the position and leaf so any wallet can reconstruct paths. The local exit index checks order and commits a batch only if replay matches the root read from applied LEZ state.
2. A wallet binds its payout to one pairing (Logos genesis, zone, bridge program, EVM chain ID, vault, underlying asset), amount, recipient and relayer fee using a high-entropy wallet secret. The burn records the commitment. The redemption guest proves membership and commits the pairing, root, nullifier and payout. The path, index and secret stay out of the journal. The nullifier for one leaf position is stable across later roots. The actor accepts a user-supplied commitment, so the wallet must construct and persist or recover it correctly before burning.
3. The vault pins the verifier contract and image IDs. An advancing checkpoint starts from the current checkpoint digest and supplies a strictly increasing source slot and the accepted exit root. Previously authenticated roots remain recognized. In direct mode the journal is Ethereum ABI encoding of `(bytes32 pairing, bytes32 previous, bytes32 next, bytes32 exitRoot, uint64 finalizedSlot)`. Optimistic deployments use the contest journals below.
4. The vault escrows ETH or one ERC-20. It rejects tokens that move a different amount from the amount in the call. Once live, a release verifies its redemption journal against an authenticated root and atomically consumes the nullifier and pays out. A payout revert rolls back consumption and accounting. The vault cannot start accepting deposits until the source verifier has admitted a checkpoint.

## Optimistic source checkpoints

Deploy with a nonzero challenge period in seconds and distinct proposal, challenge and redemption image IDs. These settings are immutable. Anyone can propose, challenge or finalize. Direct advancement is unavailable in this mode.

1. A proposal publishes its comparison data in Ethereum calldata and proves a candidate extending the current checkpoint. The vault binds the proof to the hash of that data. The candidate is pending and its root is unavailable for payouts.
2. A challenge proves that the alternative wins against the exact pending candidate under the pinned fork-choice rules at the same observation time. The proof can select an advancing checkpoint or retain the existing checkpoint if the preferred history has no eligible new root. An accepted replacement gets a full new window and must publish its comparison data too.
3. Once the last window ends, anyone can close it by naming the current candidate ID. Closing an advancing candidate authenticates its exit root. Closing a candidate that retains the existing checkpoint leaves the active root set unchanged. Prior accepted roots continue to work while a new contest is open.

The observation time is an Ethereum Unix timestamp. It must advance between rounds, be at or before inclusion, and be less than one challenge period old at proposal inclusion. Replacements share that time. The contract uses the Ethereum block timestamp for deadlines. At the deadline, challenges close and finalization opens.

**Production preference:** A protocol-native Logos checkpoint verifiable on Ethereum is likely stronger. Its safety can follow from the native consensus assumptions. The optimistic mode additionally needs at least one honest participant who can observe the relevant Logos history, obtain the comparison data, generate a valid challenge and get it included on Ethereum before each deadline. If that assumption fails, a valid but noncanonical branch can become irreversible in the vault. Running extra watchers improves coverage. Permissionless participation alone does not establish the liveness-only role RFP-022 expects of off-chain actors. The deployment would need explicit approval of this trust model. [RFP-022 trust model](https://github.com/logos-co/rfp/blob/920fad928a4592f0fdf4356046a06973fb5c931d/RFPs/RFP-022-wrapped-erc20.md#trust-model)

### Contest proof interface

All tuples use Solidity `abi.encode`. For candidate `(bytes32 history, bytes32 checkpoint, bytes32 exitRoot, uint64 slot)`, `history = sha256(comparisonData)` and `id = keccak256(abi.encode(uint64 observationTime, candidate))`. A candidate that retains the current checkpoint repeats its digest and slot and sets `exitRoot = bytes32(0)`.

| Statement | Journal hashed with SHA-256 |
| --- | --- |
| Proposal | `(PROPOSAL_JOURNAL, pairingId, previousCheckpoint, previousSlot, candidateId)` |
| Challenge | `(CHALLENGE_JOURNAL, pairingId, previousCheckpoint, previousSlot, incumbentId, challengerId)` |

The domain constants are the Keccak-256 hashes of `RFP022/source/proposal/v1` and `RFP022/source/challenge/v1`. The slot is always `uint64`. The proposal guest must derive its candidate from the pinned genesis or authenticated predecessor, the configured era schedule, and executed Logos and LEZ history. The challenge guest must reconstruct both IDs and prove that the challenger is preferred under the same pinned rules and observation horizon. The comparison data must be sufficient to compare the incumbent from public calldata. Proofs must bind the candidate to the chain's slot/clock rules and prevent advances across unsupported or unobserved history.

## Remaining bridge work

Host-side Logos replay validates supplied uncle-free blocks from genesis and can compare two supplied branches with the native bootstrap engine. The EVM tests exercise contest timing separately from cryptographic verification and confirm that the real redemption seal leaves source history locked. There are no proposal and challenge proof images yet. Production requires a reviewed rule for bootstrap, stale recovery, and fork comparisons when successive alternatives arrive, as well as full source replay and executed LEZ exit-root derivation in the guests. The publication size and proof-generation and inclusion latency must be measured before choosing a production window. See [source replay and canonicality](INTERFACE.md#native-source-replay).

The escrow sides still need the RFP-021 consumer and atomic mint, wallet recovery, deployment and migration procedure, operational controls and a full integration review. The current actor starts with synthetic balances. A target deployment must supply an audited wrapped-token issuance path and a corresponding verified deposit statement. The one-asset vault is a building block for the registry and fee/cap policy.

The local EVM uses [RISC Zero Ethereum verifier v3.1.0](https://github.com/risc0/risc0-ethereum/blob/ff30da77ba4afdd496290ebebedb205348b15d83/contracts/src/groth16/RiscZeroGroth16Verifier.sol) as a separate contract and [OpenZeppelin SafeERC20 v5.4.0](https://github.com/OpenZeppelin/openzeppelin-contracts/blob/v5.4.0/contracts/token/ERC20/utils/SafeERC20.sol) for optional-return tokens.
