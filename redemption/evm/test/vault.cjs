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
          address(0), _denominations()
        ) {}
        function _denominations() private pure returns (uint128[] memory values) {
          values = new uint128[](1);
          values[0] = 40;
        }
        function decode(bytes calldata journal) external pure returns (Release memory) {
          return _decode(journal);
        }
      }`
    }
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

async function main() {
  const chain = ganache.provider({ logging: { quiet: true }, chain: { chainId: 31337 } });
  try {
    const provider = new ethers.BrowserProvider(chain);
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
      [await verifier.getAddress(), ethers.ZeroHash, repeated(8, 32), ...common]);
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
      [await verifier.getAddress(), repeated(9, 32), repeated(8, 32), ...common]);
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
