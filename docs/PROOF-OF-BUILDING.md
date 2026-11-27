# RFP-021 and RFP-022 proof of building

These two runs exercise different parts of the bridge.

| Run | Evidence | Environment |
| --- | --- | --- |
| Deposit → private mint | A 1,000 wei deposit was authenticated at two finalized execution blocks through RFP-021. The native validator accepted the private mint. Exact replay and a fresh claim under the later header were rejected. [Deposit transaction](https://sepolia.etherscan.io/tx/0x85a4544ff3c7b4977d58e483bcef95170c40c98bb1d8d406e14820b7c089f1bd) · [Settlement log](../evidence/sepolia-inbound/settlement.log) | Public Sepolia and standalone LEZ v0.4-rc3 |
| Burn → withdrawal | One signed public Burn and one reverted overspend were proved in the same LEZ block. The complete state and fees match native `apply_block`. The local vault paid 39 units to the recipient and 1 to the relayer using a Groth16 proof tied to the proved root. The duplicate reverted without moving funds. [Burn block](INTERFACE.md#metered-wrapped-burn-inside-a-block) · [Withdrawal](REDEMPTION.md#release-from-a-proved-burn-block) | Local LEZ fixture and local EVM with the RISC Zero v3.1.0 Ethereum verifier |

The outbound run uses a fixture checkpoint to authorize the proved exit root. The outstanding public-withdrawal test must link an authenticated Logos checkpoint to the exact LEZ block and reject a valid Burn from a competing history displaced by fork choice. The inbound receipt uses the RISC Zero version shipped with LEZ rc3. Production acceptance requires rerunning the private path on a patched LEZ runtime. [RISC Zero advisory](https://github.com/risc0/risc0/security/advisories/GHSA-g6qg-9j8p-7j87)

## Verify the saved proofs

Use the [release](https://github.com/arcanetechhq/logos-rfp-022/releases/tag/proof-of-building-2026-11-28) source revision and the checkout layout in the [README](../README.md#reproduce-the-checks). The commands below run from the RFP-022 repository root. Verification of the saved proofs uses no wallet keys or Docker.

For the outbound proof, download and extract `burn-block-evidence.tar.gz` to an empty directory. Set `run` to that directory's absolute path. The checksums cover the receipt, the original guest images, the withdrawal seal and journal, the expected fixture checkpoint and the logs. The verifier authenticates both original guest images against pinned image IDs so the saved proof can be checked even when a rebuild produces different IDs. The harness derives the checkpoint from the verified receipt and compares the proved exit root with the withdrawal journal before testing the vault.

```sh
(cd "$run" && shasum -a 256 -c SHA256SUMS)
./verify-redemption.sh
work="$(cd ../.. && pwd)/work"
export RISC0_HOME="$work/risc0" RISC0_SERVER_PATH="$work/tools/r0vm"
export RISC0_EXECUTOR=ipc RISC0_PROVER=ipc RISC0_DEV_MODE=0 RISC0_BUILD_LOCKED=1
export RUSTC_WRAPPER="$PWD/scripts/rustc-remap.sh" RUSTFLAGS=--cap-lints=allow
export CARGO_TARGET_DIR="$PWD/target"
cargo +1.98.1 build --locked --features burn-block --bin burn-block
RFP022_BURN_IMAGE_DIR="$run" \
  bash redemption/evm/test/burn-block.sh "$run/burn-block-receipt.borsh" "$run"
```

For the saved inbound proof, use the sibling [RFP-021 checkout at `18eea80`](https://github.com/arcanetechhq/logos-rfp-021/tree/18eea809d7f96291898edded2628babe7b2a9537). The offline check verifies the archived private transaction proof. The settlement log records the native balance and replay checks from the original run. [Full inbound procedure](INBOUND.md)

```sh
python3 ../logos-rfp-021/scripts/prepare_staged_deps.py
export CARGO_TARGET_DIR="$PWD/target/inbound-review" RUSTFLAGS=--cap-lints=allow
cargo +1.98.1 build --locked --manifest-path inbound/Cargo.toml --features runner
RISC0_DEV_MODE=0 "$CARGO_TARGET_DIR/debug/inbound-run" \
  verify-mint-proof evidence/sepolia-inbound/private-mint.borsh
```
