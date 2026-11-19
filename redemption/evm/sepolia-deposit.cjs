const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");

const source = "src/SepoliaDepositEscrow.sol";
const chainId = 11155111n;

function compile(solc) {
  const input = {
    language: "Solidity",
    sources: Object.fromEntries([source, "src/DepositNotes.sol"].map(file => [
      file, { content: fs.readFileSync(path.join(__dirname, file), "utf8") },
    ])),
    settings: {
      optimizer: { enabled: true, runs: 200 },
      evmVersion: "shanghai",
      outputSelection: { "*": { "*": ["abi", "evm.bytecode.object", "evm.deployedBytecode.object"] } },
    },
  };
  const output = JSON.parse(solc.compile(JSON.stringify(input)));
  const errors = (output.errors ?? []).filter(issue => issue.severity === "error");
  if (errors.length) throw new Error(errors.map(issue => issue.formattedMessage).join("\n"));
  return output.contracts[source].SepoliaDepositEscrow;
}

async function main([operation, modules, ...args]) {
  if (!modules || !["deploy", "deposit"].includes(operation)) {
    throw new Error("usage: sepolia-deposit.cjs deploy|deposit module-directory [escrow commitment]");
  }
  const vendor = createRequire(path.join(path.resolve(modules), "package.json"));
  const ethers = vendor("ethers");
  const provider = new ethers.JsonRpcProvider(process.env.SEPOLIA_RPC_URL);
  if (!process.env.SEPOLIA_RPC_URL || (await provider.getNetwork()).chainId !== chainId) {
    throw new Error("a Sepolia execution RPC is required");
  }
  const key = fs.readFileSync(process.env.SEPOLIA_SIGNER_FILE, "utf8").trim();
  const signer = new ethers.Wallet(key, provider);
  if (await provider.getBalance(signer.address) === 0n) {
    throw new Error(`Sepolia signer ${signer.address} has no test ETH`);
  }
  const escrow = compile(vendor("solc"));
  let transaction;
  let address;
  if (operation === "deploy") {
    const factory = new ethers.ContractFactory(escrow.abi, escrow.evm.bytecode.object, signer);
    const contract = await factory.deploy();
    transaction = contract.deploymentTransaction();
    address = await contract.getAddress();
  } else {
    const [contractAddress, commitment] = args;
    address = ethers.getAddress(contractAddress);
    if (await provider.getCode(address) !== "0x" + escrow.evm.deployedBytecode.object) {
      throw new Error("unexpected escrow runtime bytecode");
    }
    const contract = new ethers.Contract(address, escrow.abi, signer);
    const value = 1_000n;
    transaction = await contract.deposit(commitment, { value });
  }
  console.error(`submitted Sepolia ${operation} ${transaction.hash}`);
  const receipt = await transaction.wait();
  if (receipt.status !== 1) throw new Error("Sepolia transaction reverted");
  console.log(JSON.stringify({
    transaction: receipt.hash, block: receipt.blockNumber, blockHash: receipt.blockHash, escrow: address,
  }));
}

module.exports = { chainId, compile };

if (require.main === module) {
  main(process.argv.slice(2)).catch(error => {
    // ethers can attach a signed transaction to RPC errors. Only surface the short diagnostic.
    console.error(error.shortMessage ?? error.message);
    process.exitCode = 1;
  });
}
