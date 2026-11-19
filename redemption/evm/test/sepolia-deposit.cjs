const assert = require("node:assert/strict");
const path = require("node:path");
const { createRequire } = require("node:module");
const { chainId, compile } = require("../sepolia-deposit.cjs");

const vendor = createRequire(path.join(path.resolve(process.argv[2]), "package.json"));
const ethers = vendor("ethers");
const ganache = vendor("ganache");
const escrow = compile(vendor("solc"));

async function withChain(id, check) {
  const chain = ganache.provider({ chain: { chainId: Number(id) }, logging: { quiet: true } });
  try {
    const provider = new ethers.BrowserProvider(chain);
    const factory = new ethers.ContractFactory(escrow.abi, escrow.evm.bytecode.object, await provider.getSigner());
    await check(chain, factory);
  } finally {
    await chain.disconnect();
  }
}

async function test() {
  await withChain(31337n, async (_, factory) => {
    await assert.rejects(factory.deploy(), { code: "CALL_EXCEPTION" });
  });
  await withChain(chainId, async (chain, factory) => {
    const contract = await factory.deploy();
    await contract.waitForDeployment();
    const commitment = ethers.id("local escrow integration test");
    const slot = ethers.keccak256(ethers.concat([commitment, ethers.id("RFP022.deposits.v1")]));
    const receipt = await (await contract.deposit(commitment, { value: 1234n })).wait();
    assert.equal(receipt.status, 1);
    const proof = await chain.request({
      method: "eth_getProof",
      params: [await contract.getAddress(), [slot], ethers.toQuantity(receipt.blockNumber)],
    });
    assert.equal(BigInt(proof.storageProof[0].value), 1234n);
    assert.ok(proof.storageProof[0].proof.length);
    assert.equal(await contract.depositAmount(commitment), 1234n);
    await assert.rejects(contract.deposit.staticCall(commitment, { value: 2222n }), { code: "CALL_EXCEPTION" });
    await assert.rejects(contract.deposit.staticCall(ethers.ZeroHash, { value: 1234n }), { code: "CALL_EXCEPTION" });
    await assert.rejects(contract.deposit.staticCall(ethers.id("zero amount")), { code: "CALL_EXCEPTION" });
    assert.equal(await contract.depositAmount(commitment), 1234n);
  });
  console.log("PASS Sepolia escrow chain guard, storage proof layout and write-once note");
}

test().catch(error => {
  console.error(error);
  process.exitCode = 1;
});
