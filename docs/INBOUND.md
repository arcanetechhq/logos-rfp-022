# Sepolia deposit and private LEZ mint

The inbound actor checks the escrow's write-once amount with RFP-021's Ethereum storage verifier. It asks the pinned 021 tracker to authenticate the header-history root and records the deposit as consumed in the same LEZ transaction that credits the private wrapped balance. The actor shares the outbound prototype's balance encoding and transfer/burn code.

The deposit commitment binds the Sepolia chain, escrow, mint program, private recipient and secret. The consume marker uses a separately domain-separated hash of the same inputs. The marker stays stable across finalized execution blocks, even though the attested RFP-021 fact ID changes. The private transaction exposes the tracker root and consume marker. Its wire encoding is checked for the recipient, secret, deposit commitment and storage slot.

The live runner uses a fresh local LEZ rc3 state with the native validator and a genuine RISC Zero succinct receipt. It decrypts the accepted balance with the recipient's wallet key, checks it against a commitment in native state, then checks exact-transaction replay and a fresh claim under a second finalized header. The Sepolia ETH escrow is a test contract with no withdrawal path. The general vault retains its authenticated outbound checkpoint gate.

## Run

Use sibling checkouts of this repository and [RFP-021 at `18eea80`](https://github.com/arcanetechhq/logos-rfp-021/tree/18eea809d7f96291898edded2628babe7b2a9537). Set up the pinned RISC Zero 3.0.5 runtime and guest compiler as described in [RFP-021 tool setup](https://github.com/arcanetechhq/logos-rfp-021/blob/18eea809d7f96291898edded2628babe7b2a9537/docs/tool-setup.md).

```sh
RFP021="$(cd ../logos-rfp-021 && pwd)"
RUN="$(mktemp -d)"
umask 077
python3 "$RFP021/scripts/prepare_staged_deps.py"
export CARGO_TARGET_DIR="$RUN/host"
cargo +1.98.1 build --locked --manifest-path inbound/Cargo.toml --features runner
HOST="$CARGO_TARGET_DIR/debug/inbound-run"
TRACKER="$RFP021/evidence/binaries/staged-v0.6.elf"

mkdir "$RUN/evm"
cp redemption/evm/package.json redemption/evm/package-lock.json "$RUN/evm/"
npm ci --prefix "$RUN/evm"
export SEPOLIA_RPC_URL=https://eth-sepolia.api.onfinality.io/public
# Set SEPOLIA_SIGNER_FILE to the absolute path of a funded test-only key file.
node redemption/evm/sepolia-deposit.cjs deploy "$RUN/evm" > "$RUN/escrow.json"
ESCROW="$(jq -r .escrow "$RUN/escrow.json")"
"$HOST" policy "$TRACKER" "$ESCROW" > "$RUN/policy.json"
inbound/build-guest.sh "$RUN/policy.json" "$RUN/guest"
MINT="$RUN/guest/riscv32im-risc0-zkvm-elf/release/inbound-actor"
"$HOST" note "$MINT" "$RUN/policy.json" "$RUN/private-note.json" > "$RUN/public-note.json"
COMMITMENT="$(jq -r .commitment "$RUN/public-note.json")"
node redemption/evm/sepolia-deposit.cjs deposit "$RUN/evm" "$ESCROW" "$COMMITMENT" > "$RUN/deposit.json"
```

Choose a trusted recent finalized Sepolia beacon root whose execution block includes the deposit. Wait for one later finalized header in the same sync-committee period. The collector can use a different execution RPC from the depositor. It needs EIP-1186 proofs at both finalized block hashes. On November 24, the ethPandaOps endpoint served both sampled blocks (12,204,544 and 12,204,638). OnFinality and PublicNode rejected the older proof request.

```sh
python3 inbound/collect-sepolia.py "$RUN/policy.json" "$CHECKPOINT" \
  --execution https://rpc.sepolia.ethpandaops.io \
  --deposit "$(jq -r .transaction "$RUN/deposit.json")" --note "$RUN/public-note.json" > "$RUN/inputs.json"
RISC0_DEV_MODE=0 RISC0_PROVER=ipc RISC0_EXECUTOR=ipc RISC0_SERVER_PATH="$RFP021/.tools/r0vm" \
  RFP022_MINT_TX_OUT="$RUN/private-mint.borsh" \
  "$HOST" run "$TRACKER" "$MINT" "$RUN/policy.json" "$RUN/private-note.json" "$RUN/inputs.json"
RISC0_DEV_MODE=0 "$HOST" verify-mint-proof "$RUN/private-mint.borsh"
```

The saved transaction contains its succinct receipt and public message. The offline command reconstructs the journal from that message and verifies it against the pinned LEZ privacy circuit image. Wallet recovery and the second-header replay check run during mint settlement.

The ordinary unit tests are available via `cargo +1.98.1 test --locked --manifest-path inbound/Cargo.toml`. The separate native runtime test uses synthetic Ethereum state. Supply `RFP022_TRACKER_ELF`, `RFP022_MINT_ELF` and `RFP022_INBOUND_POLICY` with the built images and policy, set the same RISC Zero environment, and run `cargo +1.98.1 test --locked --manifest-path inbound/Cargo.toml --features runner --bin inbound-run -- --ignored`. Set `RFP022_PROVE_FIXTURE=1` to also prove and settle the synthetic claim.

## Verified

- The native 021 tracker admitted Sepolia Gloas finalized execution blocks `0x5cfab95a30f1fd552b8ef79220d97332c288a3920d8cb905bf9eeca89ae04ed6` and `0x462d723a1f6128c6fd42884a76f407af2618e0e8bbb77b7ff86a4655a7212990` using signed consensus data.
- The synthetic fixture produced a genuine private receipt accepted by the local rc3 validator. The wallet recovered a wrapped balance of 40. The exact replay and an independently attested claim with a fresh wallet witness were rejected, with the native state digest preserved.
- On November 26, the [Sepolia escrow deposit](https://sepolia.etherscan.io/tx/0x85a4544ff3c7b4977d58e483bcef95170c40c98bb1d8d406e14820b7c089f1bd) transferred 1,000 wei in block 12,215,573. The tracker verified the deposit at finalized execution blocks 12,215,600 and 12,215,631. The local rc3 validator accepted the private mint in 211,365 settlement cycles. The recipient recovered 1,000 wrapped wei. Exact transaction replay failed. A fresh claim under the second header passed the control simulation with the consume marker cleared and failed with E004 when the marker was present. The state digest stayed unchanged. [Settlement log](../evidence/sepolia-inbound/settlement.log)
- The [private transaction with its succinct proof](../evidence/sepolia-inbound/private-mint.borsh) is 227,407 bytes. Its SHA-256 and LEZ transaction hash are `8f851b57c2ea69862e826658637084c82c8610ea7e793c5b700df79ce944f9f8`. Run `RISC0_DEV_MODE=0 "$HOST" verify-mint-proof evidence/sepolia-inbound/private-mint.borsh` from the repository root with the runner built above.
