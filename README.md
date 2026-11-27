# Logos RFP-022 proof of building

This repository contains the inbound and outbound prototypes for the LEZ wrapped-asset bridge. A real Sepolia deposit was attested with RFP-021 and minted privately on a standalone LEZ validator. In a separate local run, a proved Burn in a LEZ block funded a withdrawal through the released RISC Zero Groth16 verifier. The local withdrawal authorizes its exit root through a fixture checkpoint. Public withdrawals need a checkpoint authenticated against canonical Logos history.

[Review the evidence and rerun verification](docs/PROOF-OF-BUILDING.md) · [Download the proof bundle](https://github.com/arcanetechhq/logos-rfp-022/releases/tag/proof-of-building-2026-11-28)

## What we built

| Component | Verified behavior |
| --- | --- |
| Sepolia deposit to private mint | RFP-021 authenticated a real Sepolia ETH deposit. A standalone LEZ v0.4-rc3 validator accepted the private mint. It rejected exact replay and a fresh claim for the same deposit under a later finalized header. [Deposit and evidence](docs/INBOUND.md#verified) |
| Proved Burn block to EVM withdrawal | A separate proof checks a successful public Burn, a reverted overspend, fee accounting and the complete post-state against native `apply_block`. The local vault paid out against the resulting root and rejected a duplicate claim. [Block proof](docs/INTERFACE.md#metered-wrapped-burn-inside-a-block) · [EVM withdrawal](docs/REDEMPTION.md#release-from-a-proved-burn-block) |
| LEZ burn and exit accumulator | Two public fixture burns execute through native LEZ `apply_block`, debit the wrapped balance and append commitments. Replaying the accepted events reproduces the stored root. Overspend, wrong pairing and unauthorized append leave the balance and root intact. [Test output](evidence/redemption-actor-tests.log) |
| Private LEZ execution | A separate fixture executes a private burn with a real native PPE proof. The historical checkpoint guest proves a restricted private action and checks its resulting state against native execution. [Settlement](evidence/redemption-native-settlement.log) · [Proof boundary](docs/INTERFACE.md#what-the-fragment-verifies) |
| Private redemption statement | The RISC Zero guest proves membership in the exit root reconstructed from the two applied public burns. The proof binds the asset and payout to the selected pairing and produces a replay nullifier. The leaf position, membership path and secret stay private. [Receipt generation](evidence/redemption-native-v2-generation.log) |
| Ethereum verification | The RISC Zero v3.1.0 Ethereum verifier accepts the 260-byte Groth16 seal. Calling that verifier directly used 251,671 gas. Changed journals and image IDs fail. The seal cannot advance a source checkpoint or release funds while source history is unavailable. [Verification result](evidence/redemption-evm-result.json) |
| Logos source history | Host replay checks native block signatures, transaction execution and genuine Proof of Leadership on two fixture branches. The competing branch displaces the inscription carrying the LEZ burn. A separate EVM test exercises the deadline, replacement and stale-call rules for a permissionless checkpoint contest. [Logos regression test](native-auth/src/source_replay/tests.rs) · [Contest proof interface](docs/REDEMPTION.md#contest-proof-interface) |

The outbound fixture uses synthetic keys, genesis balances and source history. The Sepolia deposit is a public testnet transaction. The 251,671 gas figure covers a direct call to the proof verifier for the earlier redemption fixture.

## Reproduce the checks

Use the archive's `outputs/reverse-settlement/` layout. For a Git checkout, place it at `<review-directory>/outputs/reverse-settlement/` so the scripts can create `<review-directory>/work/` for pinned dependencies and run output.

From the repository root, run the native burn, saved receipt and Groth16 verification checks, then the Logos fork replay:

```sh
VERIFY_NATIVE_V2_PROOF=1 REUSE_REDEMPTION=1 VERIFY_GROTH16=1 ./verify-redemption.sh
RUSTFLAGS=--cap-lints=allow cargo +1.98.1 test --locked --manifest-path native-auth/Cargo.toml --lib
```

Prerequisites include Git, rustup, curl, ripgrep, Node.js/npm and a native C/C++ build toolchain. The verifier and upstream Logos revisions are pinned. These commands were tested on macOS arm64. The first command builds the guests, executes the LEZ fixtures and verifies the supplied proof artifacts. It does not need Docker. The second command generates and verifies the fixture leadership proofs. Intentional guest failures are exercised by the negative tests. Both commands should exit successfully.

The [verification guide](docs/REDEMPTION.md#check-the-code) covers fresh proof generation, Groth16 wrapping and the separate native private-burn case.

## What remains for the grant

- **Logos → Ethereum.** The vault still needs an authenticated proof linking the canonical Logos history to the executed LEZ exit root. A protocol-native checkpoint verifiable by Ethereum is the preferred production design. The prototype's challenge-window fallback adds an assumption that a participant can observe a better branch, produce a challenge proof and have it included before the deadline. Its source proposal and comparison guests remain to be built.
- **Ethereum → LEZ.** The [inbound runner](docs/INBOUND.md) connects RFP-021 finalized Sepolia storage proofs to private wrapped minting on LEZ rc3. It checks replay across different finalized headers and supports a Sepolia-only ETH escrow.
- **Integration and operation.** The grant plan should validate the full round trip on the required testnets and cover recovery, operating procedures and the audit boundary. Our proposed post-delivery role covers running the prover and relaying infrastructure, monitoring checkpoint health, maintaining compatibility with upstream releases and handling incidents. Proof verification follows the public deployment configuration.

[Redemption design](docs/REDEMPTION.md) · [Source and version pins](docs/SOURCES.md) · [License](LICENSE)
