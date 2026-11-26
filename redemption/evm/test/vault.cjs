const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");

const toolDirectory = path.resolve(process.argv[2]);
const vendor = createRequire(path.join(toolDirectory, "package.json"));
const solc = vendor("solc");
const ganache = vendor("ganache");
const ethers = vendor("ethers");

const evm = path.resolve(__dirname, "..");
const official = path.join(toolDirectory, "upstream", "contracts", "src");

function readSource(file) {
  if (file.startsWith("@openzeppelin/")) {
    return fs.readFileSync(path.join(toolDirectory, "node_modules", file), "utf8");
  }
  if (file.startsWith("openzeppelin/contracts/")) {
    return readSource("@" + file);
  }
  if (file.startsWith("risc0/")) {
    return fs.readFileSync(path.join(official, file.slice("risc0/".length)), "utf8");
  }
  return fs.readFileSync(path.join(evm, file), "utf8");
}

const selected = "risc0/groth16/RiscZeroGroth16Verifier.sol";
const input = {
  language: "Solidity",
  sources: {
    [selected]: { content: readSource(selected) },
    "src/RedemptionVault.sol": { content: readSource("src/RedemptionVault.sol") },
    "test/DecodingProbe.sol": {
      content: `
      // SPDX-License-Identifier: MIT
      pragma solidity 0.8.30;
      import "../src/RedemptionVault.sol";
      contract DecodingProbe is RedemptionVault {
        constructor(IRiscZeroVerifier v) RedemptionVault(
          v, bytes32(0), bytes32(uint256(1)), bytes32(uint256(2)),
          bytes32(uint256(3)), bytes32(uint256(4)), bytes32(uint256(5)),
          address(0), _denominations(), bytes32(0), 0
        ) {}
        function _denominations() private pure returns (uint128[] memory values) {
          values = new uint128[](1);
          values[0] = 40;
        }
        function decode(bytes calldata journal) external pure returns (Release memory) {
          return _decode(journal);
        }
      }`
    },
    "test/ContestProbe.sol": {
      content: `
      // SPDX-License-Identifier: MIT
      pragma solidity 0.8.30;
      import "../src/CheckpointContest.sol";
      contract ContestProbe {
        using CheckpointContest for CheckpointContest.State;
        CheckpointContest.State private state;
        function getRound() external view returns (CheckpointContest.State memory) { return state; }
        function open(CheckpointContest.Candidate calldata candidate, uint64 observed, uint64 period) external {
          state.open(candidate, observed, period);
        }
        function replace(CheckpointContest.Candidate calldata candidate, bytes32 expected, uint64 period) external {
          state.replace(candidate, expected, period);
        }
        function close(bytes32 expected) external returns (CheckpointContest.Candidate memory) {
          return state.close(expected);
        }
      }`
    },
    "test/FixtureCheckpointVerifier.sol": {
      content: `
      // SPDX-License-Identifier: MIT
      pragma solidity 0.8.30;
      import "../src/RedemptionVault.sol";
      // Local test only. The replay fixture supplies exactly one trusted checkpoint digest.
      contract FixtureCheckpointVerifier {
        error UnselectedFixtureCheckpoint();
        IRiscZeroVerifier private immutable official;
        bytes32 private immutable fixtureImage;
        bytes32 private immutable fixtureDigest;

        constructor(IRiscZeroVerifier verifier, bytes32 image, bytes32 digest) {
          official = verifier;
          fixtureImage = image;
          fixtureDigest = digest;
        }

        function verify(bytes calldata seal, bytes32 imageId, bytes32 digest) external view {
          if (imageId == fixtureImage) {
            if (digest != fixtureDigest || seal.length != 0) revert UnselectedFixtureCheckpoint();
          } else {
            official.verify(seal, imageId, digest);
          }
        }
      }`
    },
  },
  settings: {
    optimizer: { enabled: true, runs: 200 },
    evmVersion: "shanghai",
    outputSelection: { "*": { "*": ["abi", "evm.bytecode.object"] } }
  }
};
const compiled = JSON.parse(solc.compile(JSON.stringify(input), {
  import(file) {
    try { return { contents: readSource(file) }; }
    catch (error) { return { error: String(error) }; }
  }
}));
const errors = (compiled.errors ?? []).filter(issue => issue.severity === "error");
assert.equal(errors.length, 0, errors.map(issue => issue.formattedMessage).join("\n"));

function repeated(value, length) {
  return "0x" + value.toString(16).padStart(2, "0").repeat(length);
}

async function expectRevert(action, selector) {
  await assert.rejects(action, error => {
    if (error.code !== "CALL_EXCEPTION") return false;
    if (!selector) return true;
    return error.data?.startsWith(selector);
  });
}

function selector(signature) {
  return ethers.id(signature).slice(0, 10);
}

function candidateId(observation, candidate) {
  return ethers.keccak256(ethers.AbiCoder.defaultAbiCoder().encode(
    ["uint64", "tuple(bytes32 history,bytes32 checkpoint,bytes32 exitRoot,uint64 slot)"],
    [observation, candidate],
  ));
}

async function contestLifecycle(chain, probe, other) {
  const period = 600n;
  const time = async () => BigInt((await chain.request({
    method: "eth_getBlockByNumber", params: ["latest", false],
  })).timestamp);
  const mineAt = async timestamp => {
    await chain.request({ method: "evm_mine", params: [Number(timestamp)] });
    assert.equal(await time(), timestamp);
  };
  const observed = await time();
  const candidate = [repeated(1, 32), repeated(2, 32), repeated(3, 32), 1n];
  const hold = [repeated(4, 32), repeated(5, 32), ethers.ZeroHash, 0n];
  const originalId = candidateId(observed, candidate);
  const holdId = candidateId(observed, hold);
  await expectRevert(() => probe.open.staticCall(candidate, observed + 10n, period),
    selector("InvalidObservationTime()"));
  await expectRevert(() => probe.open.staticCall(candidate, observed - period, period),
    selector("InvalidObservationTime()"));
  await (await probe.open(candidate, observed, period)).wait();
  const firstDeadline = (await probe.getRound()).deadline;
  await expectRevert(() => probe.open.staticCall(candidate, observed, period), selector("ContestAlreadyOpen()"));
  await expectRevert(() => probe.close.staticCall(originalId), selector("ChallengeWindowOpen()"));
  await expectRevert(() => probe.replace.staticCall(hold, ethers.ZeroHash, period), selector("CandidateChanged()"));
  await mineAt(firstDeadline - 10n);
  await (await probe.connect(other).replace(hold, originalId, period)).wait();
  const replaced = await probe.getRound();
  assert.equal(replaced.observationTime, observed);
  assert.equal(replaced.candidate.history, hold[0]);
  assert.equal(replaced.deadline, await time() + period);
  assert.ok(replaced.deadline > firstDeadline);
  await mineAt(firstDeadline);
  await expectRevert(() => probe.close.staticCall(holdId), selector("ChallengeWindowOpen()"));
  await mineAt(replaced.deadline);
  await expectRevert(() => probe.replace.staticCall(candidate, holdId, period),
    selector("ChallengeWindowClosed()"));
  await expectRevert(() => probe.close.staticCall(originalId), selector("CandidateChanged()"));
  assert.deepEqual(Array.from(await probe.close.staticCall(holdId)), hold);
  await (await probe.connect(other).close(holdId)).wait();
  assert.equal((await probe.getRound()).deadline, 0n);
  assert.equal((await probe.getRound()).observationTime, observed);
  await expectRevert(() => probe.close.staticCall(holdId), selector("NoOpenContest()"));
  await expectRevert(() => probe.open.staticCall(candidate, observed, period),
    selector("InvalidObservationTime()"));
  await (await probe.open(candidate, await time(), period)).wait();
  console.log("PASS contest timing, late replacement, stale calls, hold result and fresh next round");
}

async function releaseWithFixtureCheckpoint({ chain, provider, signer, deploy, verifier, seal, imageId, journal }) {
  const checkpoint = JSON.parse(fs.readFileSync(process.env.RFP022_EVM_CHECKPOINT, "utf8"));
  const { pairing, previous, next, exitRoot, displacedRoot, slot } = checkpoint;
  const { displacedCheckpoint, displacedSlot } = checkpoint;
  const address = repeated(4, 20);
  const digest = (candidate, root, candidateSlot) => ethers.sha256(ethers.AbiCoder.defaultAbiCoder().encode(
    ["bytes32", "bytes32", "bytes32", "bytes32", "uint64"],
    [pairing, previous, candidate, root, candidateSlot],
  ));
  const fixtureImage = ethers.id("RFP022 local source checkpoint fixture");
  const vaultCode = compiled.contracts["src/RedemptionVault.sol"].RedemptionVault;
  const vaultFactory = new ethers.ContractFactory(vaultCode.abi, vaultCode.evm.bytecode.object, signer);
  const config = [repeated(1, 32), repeated(2, 32), repeated(0x44, 32), ethers.ZeroAddress, [40],
    ethers.ZeroHash, 0];
  const setCode = code => chain.request({ method: "evm_setAccountCode", params: [address, code] });

  async function vaultFor(candidate, root, candidateSlot) {
    const source = await deploy("test/FixtureCheckpointVerifier.sol", "FixtureCheckpointVerifier",
      [await verifier.getAddress(), fixtureImage, digest(candidate, root, candidateSlot)]);
    const init = await vaultFactory.getDeployTransaction(
      await source.getAddress(), fixtureImage, imageId, previous, ...config,
    );
    // The archived real proof commits to this vault address. Run the actual constructor
    // at that address on Ganache and install the runtime it returns.
    await setCode(init.data);
    const runtime = await provider.call({ to: address, data: "0x" });
    assert.equal((await (await signer.sendTransaction({ to: address, gasLimit: 3_000_000 })).wait()).status, 1);
    await setCode(runtime);
    const vault = vaultFactory.attach(address);
    assert.equal(await vault.pairingId(), pairing);
    assert.equal(await vault.checkpoint(), previous);
    assert.equal(await vault.denomination(40), true);
    return vault;
  }

  assert.equal(ethers.hexlify(journal.subarray(0, 32)), pairing);
  assert.equal(ethers.hexlify(journal.subarray(32, 64)), exitRoot);
  const nullifier = ethers.hexlify(journal.subarray(64, 96));
  const snapshot = await chain.request({ method: "evm_snapshot", params: [] });
  const alternative = await vaultFor(displacedCheckpoint, displacedRoot, displacedSlot);
  await (await alternative.advanceCheckpoint(displacedCheckpoint, displacedRoot, displacedSlot, "0x")).wait();
  await (await alternative.deposit(repeated(0xaa, 32), 40, { value: 40 })).wait();
  await expectRevert(() => alternative.release.staticCall(journal, seal), selector("UnknownExitRoot()"));
  assert.equal(await alternative.spent(nullifier), false);
  assert.equal(await alternative.escrowed(), 40n);
  assert.equal(await chain.request({ method: "evm_revert", params: [snapshot] }), true);

  const vault = await vaultFor(next, exitRoot, slot);
  await expectRevert(() => vault.advanceCheckpoint.staticCall(displacedCheckpoint, displacedRoot, displacedSlot, "0x"),
    selector("UnselectedFixtureCheckpoint()"));
  assert.equal(await vault.checkpoint(), previous);
  const advance = await (await vault.advanceCheckpoint(next, exitRoot, slot, "0x")).wait();
  assert.equal(await vault.authenticatedRoots(exitRoot), true);
  assert.equal(await vault.authenticatedRoots(displacedRoot), false);
  await (await vault.deposit(repeated(0xaa, 32), 40, { value: 40 })).wait();
  const release = await (await vault.release(journal, seal)).wait();
  const balance = account => chain.request({ method: "eth_getBalance", params: [account, "latest"] });
  assert.equal(BigInt(await balance(repeated(6, 20))), 39n);
  assert.equal(BigInt(await balance(repeated(7, 20))), 1n);
  assert.equal(await vault.escrowed(), 0n);
  assert.equal(await vault.spent(nullifier), true);
  await expectRevert(() => vault.release.staticCall(journal, seal), selector("AlreadyRedeemed()"));
  await assert.rejects(async () => {
    const duplicate = await vault.release(journal, seal, { gasLimit: 750_000 });
    await duplicate.wait();
  }, error => error.receipt?.status === 0);
  assert.equal(BigInt(await balance(repeated(6, 20))), 39n);
  assert.equal(BigInt(await balance(repeated(7, 20))), 1n);
  console.log("PASS local checkpoint selects the payout root and rejects the displaced root and duplicate payout");
  const elapsedBlocks = release.blockNumber - advance.blockNumber;
  console.log(`Fixture checkpoint gas=${advance.gasUsed} genuine Groth16 release gas=${release.gasUsed} local blocks to payout=${elapsedBlocks}`);
}

async function main() {
  const chain = ganache.provider({ logging: { quiet: true }, chain: { chainId: 31337 } });
  try {
    const provider = new ethers.BrowserProvider(chain);
    provider.pollingInterval = 50;
    const signer = await provider.getSigner();
    async function deploy(source, name, args) {
      const code = compiled.contracts[source][name];
      const contract = await new ethers.ContractFactory(code.abi, code.evm.bytecode.object, signer)
        .deploy(...args);
      await contract.waitForDeployment();
      return contract;
    }
    const root = "0xe6feef13db7aed4a731c7b0852de1f5c89b137726073f625d05333184d3a551d";
    const control = "0x0ac2eaf9d5ae64ffb75c4747fde32456e9ee6db3b32350cb2a0614ba91d081de";
    const verifier = await deploy(selected, "RiscZeroGroth16Verifier", [root, control]);
    const common = [repeated(5, 32), repeated(1, 32), repeated(2, 32), repeated(0x42, 32), ethers.ZeroAddress, [40]];
    const vault = await deploy("src/RedemptionVault.sol", "RedemptionVault",
      [await verifier.getAddress(), ethers.ZeroHash, repeated(8, 32), ...common, ethers.ZeroHash, 0]);
    const expectedPairing = ethers.sha256(ethers.solidityPacked(
      ["string", "bytes32", "bytes32", "bytes32", "uint256", "address", "address"],
      ["RFP022/pairing/v1", common[1], common[2], common[3], 31337, await vault.getAddress(), ethers.ZeroAddress],
    ));
    assert.equal(await vault.pairingId(), expectedPairing);
    const unavailable = vault.interface.getError("SourceHistoryUnavailable").selector;
    await expectRevert(() => vault.advanceCheckpoint.staticCall(repeated(6, 32), repeated(7, 32), 1, "0x"), unavailable);
    await expectRevert(() => vault.deposit.staticCall(repeated(6, 32), 40, {value: 40}), unavailable);
    await expectRevert(() => vault.release.staticCall("0x", "0x"), unavailable);

    const pending = await deploy("src/RedemptionVault.sol", "RedemptionVault",
      [await verifier.getAddress(), repeated(9, 32), repeated(8, 32), ...common, ethers.ZeroHash, 0]);
    // Exercise the official verifier using its valid seal selector and invalid proof coordinates.
    const invalidSeal = ethers.concat([await verifier.SELECTOR(), new Uint8Array(256)]);
    await expectRevert(() => pending.advanceCheckpoint.staticCall(repeated(6, 32), repeated(7, 32), 1, invalidSeal));
    let rejected = false;
    try {
      const transaction = await pending.advanceCheckpoint(repeated(6, 32), repeated(7, 32), 1, invalidSeal,
        { gasLimit: 3_000_000 });
      await transaction.wait();
    } catch (error) {
      assert.equal(error.receipt?.status, 0);
      rejected = true;
    }
    assert.ok(rejected, "invalid checkpoint transaction was mined as a revert");
    assert.equal(await pending.sourceSlot(), 0n);
    assert.equal(await pending.checkpoint(), common[0]);
    assert.equal(await pending.authenticatedRoots(repeated(7, 32)), false);
    assert.equal(await pending.escrowed(), 0n);
    await expectRevert(() => pending.deposit.staticCall(repeated(6, 32), 40, {value: 40}), unavailable);
    await expectRevert(() => pending.release.staticCall("0x", invalidSeal), unavailable);
    console.log("PASS actual RISC Zero v3.1.0 EVM verifier rejects forged source checkpoint atomically");

    const optimistic = await deploy("src/RedemptionVault.sol", "RedemptionVault",
      [await verifier.getAddress(), repeated(9, 32), repeated(8, 32), ...common, repeated(10, 32), 600]);
    const history = "0x010203";
    const proposal = [ethers.sha256(history), repeated(6, 32), repeated(7, 32), 1n];
    const observed = BigInt((await chain.request({ method: "eth_getBlockByNumber", params: ["latest", false] })).timestamp);
    await expectRevert(() => optimistic.challengeCheckpoint.staticCall(ethers.ZeroHash, proposal, history, invalidSeal),
      selector("NoOpenContest()"));
    await expectRevert(() => optimistic.advanceCheckpoint.staticCall(...proposal.slice(1), invalidSeal), unavailable);
    await expectRevert(() => pending.proposeCheckpoint.staticCall(proposal, observed, history, invalidSeal), unavailable);
    await expectRevert(() => optimistic.proposeCheckpoint.staticCall(proposal, observed, "0x00", invalidSeal),
      selector("InvalidCheckpoint()"));
    await expectRevert(() => optimistic.proposeCheckpoint.staticCall(proposal, observed, history, invalidSeal));
    await assert.rejects(async () => {
      const tx = await optimistic.proposeCheckpoint(proposal, observed, history, invalidSeal, { gasLimit: 3_000_000 });
      await tx.wait();
    }, error => error.receipt?.status === 0);
    await expectRevert(() => optimistic.finalizeCheckpoint.staticCall(candidateId(observed, proposal)),
      selector("NoOpenContest()"));
    assert.equal((await optimistic.pendingCheckpoint()).deadline, 0n);
    assert.equal(await optimistic.sourceSlot(), 0n);
    assert.equal(await optimistic.checkpoint(), common[0]);
    assert.equal(await optimistic.authenticatedRoots(proposal[2]), false);
    assert.equal(await optimistic.escrowed(), 0n);
    await expectRevert(() => optimistic.deposit.staticCall(repeated(6, 32), 40, { value: 40 }), unavailable);
    const contest = await deploy("test/ContestProbe.sol", "ContestProbe", []);
    await contestLifecycle(chain, contest, await provider.getSigner(1));

    const probe = await deploy("test/DecodingProbe.sol", "DecodingProbe", [await verifier.getAddress()]);
    const bytes = fs.readFileSync("evidence/redemption-encoding-vector.borsh");
    const decoded = await probe.decode(bytes);
    assert.equal(decoded.pairing, ethers.sha256(ethers.solidityPacked(
      ["string", "bytes32", "bytes32", "bytes32", "uint256", "address", "address"],
      ["RFP022/pairing/v1", common[1], common[2], common[3], 31337, repeated(4, 20), ethers.ZeroAddress],
    )));
    assert.equal(decoded.asset, ethers.ZeroAddress);
    assert.equal(decoded.recipient.toLowerCase(), repeated(6, 20));
    assert.equal(decoded.relayer.toLowerCase(), repeated(7, 20));
    assert.equal(decoded.amount, 40n);
    assert.equal(decoded.fee, 1n);
    await expectRevert(() => probe.decode(ethers.concat([bytes, "0x00"])),
      probe.interface.getError("InvalidJournal").selector);
    console.log("PASS Solidity decodes exact Rust guest journal and rejects noncanonical length");

    if (process.env.VERIFY_GROTH16 === "1") {
      const prefix = "evidence/redemption-native-v2";
      const seal = fs.readFileSync(prefix + "-evm-seal.bin");
      const imageId = fs.readFileSync(prefix + "-image-id.bin");
      const journal = fs.readFileSync(prefix + "-journal.borsh");
      const digest = ethers.sha256(journal);
      await verifier.verify.staticCall(seal, imageId, digest);
      await expectRevert(() => pending.release.staticCall(journal, seal), unavailable);
      await expectRevert(() => optimistic.release.staticCall(journal, seal), unavailable);
      const recentObservation = BigInt((await chain.request({
        method: "eth_getBlockByNumber", params: ["latest", false],
      })).timestamp);
      await expectRevert(() => optimistic.proposeCheckpoint.staticCall(proposal, recentObservation, history, seal));
      assert.equal((await optimistic.pendingCheckpoint()).deadline, 0n);
      await expectRevert(() => pending.advanceCheckpoint.staticCall(repeated(6, 32), repeated(7, 32), 1, seal));
      assert.equal(await pending.sourceSlot(), 0n);
      assert.equal(await pending.authenticatedRoots(repeated(7, 32)), false);
      const sent = await signer.sendTransaction({
        to: await verifier.getAddress(),
        data: verifier.interface.encodeFunctionData("verify", [seal, imageId, digest]),
        gasLimit: 3_000_000
      });
      const verified = await sent.wait();
      assert.equal(verified.status, 1);
      if (process.env.RFP022_EVM_CHECKPOINT) {
        await releaseWithFixtureCheckpoint({ chain, provider, signer, deploy, verifier, seal, imageId, journal });
      }
      journal[64] ^= 1;
      await expectRevert(() => verifier.verify.staticCall(seal, imageId, ethers.sha256(journal)));
      await expectRevert(() => verifier.verify.staticCall(seal, ethers.ZeroHash, digest));
      console.log("PASS local EVM verifies the real redemption proof, verification gas used=" + verified.gasUsed);
    }
  } finally {
    await chain.disconnect();
  }
}

main().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
