"""Collect raw Sepolia Beacon and Execution inputs. The LEZ runner verifies them."""

import argparse
import json
import os
from pathlib import Path
import urllib.request


def query(url, payload=None):
    request = urllib.request.Request(
        url, data=None if payload is None else json.dumps(payload).encode(),
        headers={
            "Content-Type": "application/json",
            "User-Agent": "arcane-logos-rfp-022/0.1 (Sepolia proof collector)",
        },
    )
    with urllib.request.urlopen(request, timeout=20) as response:
        return json.load(response)


def collect(args):
    beacon = args.beacon.rstrip("/") + "/eth/v1/"
    policy = json.loads(args.policy.read_text())

    def rpc(method, *params):
        response = query(args.execution, {
            "jsonrpc": "2.0", "id": 1, "method": method, "params": params,
        })
        if response.get("result") is None:
            raise ValueError(f"{method}: {response.get('error', 'record not found')}")
        return response["result"]

    if int(rpc("eth_chainId"), 16) != 11155111:
        raise ValueError("execution RPC must serve Sepolia")
    genesis = query(beacon + "beacon/genesis")["data"]
    schedule = query(beacon + "config/fork_schedule")["data"]
    names = ["genesis", "altair", "bellatrix", "capella", "deneb", "electra", "fulu", "gloas"]
    forks = {
        name: {"epoch": int(row["epoch"]), "fork_version": row["current_version"]}
        for name, row in zip(names, schedule, strict=False)
    }
    client = beacon + "beacon/light_client/"
    bootstrap = query(client + "bootstrap/" + args.checkpoint)
    update = query(client + "finality_update")
    if bootstrap["version"] != "gloas" or update["version"] != "gloas":
        raise ValueError("this runner uses the Gloas light-client format")
    bootstrap, update = bootstrap["data"], update["data"]
    first, second = bootstrap["header"], update["finalized_header"]
    if int(second["beacon"]["slot"]) <= int(first["beacon"]["slot"]):
        raise ValueError("wait for a newer finalized header than the bootstrap")
    first_header, second_header = [
        rpc("eth_getBlockByHash", row["execution_block_hash"], False)
        for row in (first, second)
    ]
    finality = {
        "config": {
            "instance_id": policy["instance_id"],
            "chain_id": 11155111,
            "genesis_validators_root": genesis["genesis_validators_root"],
            "checkpoint": args.checkpoint,
            "genesis_time": int(genesis["genesis_time"]),
            "seconds_per_slot": 12,
            "weak_subjectivity_seconds": 14 * 86400,
            "forks": forks,
        },
        "bootstrap": bootstrap,
        "update": update,
        "bootstrap_execution": first_header,
        "update_execution": second_header,
    }
    if args.deposit is None:
        return finality
    deposit = rpc("eth_getTransactionReceipt", args.deposit)
    if int(deposit["status"], 16) != 1 or deposit["to"].lower() != policy["escrow"].lower():
        raise ValueError("deposit receipt failed or used a different escrow")
    note = json.loads(args.note.read_text())
    transaction = rpc("eth_getTransactionByHash", args.deposit)
    expected_input = "0xb214faa5" + note["commitment"][2:]  # deposit(bytes32)
    if transaction["input"].lower() != expected_input.lower():
        raise ValueError("deposit transaction did not fund this note")
    amount = int(transaction["value"], 16)
    if amount <= 0:
        raise ValueError("deposit transaction did not transfer ETH")
    canonical = rpc("eth_getBlockByNumber", deposit["blockNumber"], False)
    if canonical["hash"] != deposit["blockHash"]:
        raise ValueError("deposit receipt is outside the canonical chain")
    if int(first_header["number"], 16) < int(deposit["blockNumber"], 16):
        raise ValueError("the bootstrap must already finalize the deposit")
    proofs = [
        rpc("eth_getProof", policy["escrow"], [note["slot"]],
            {"blockHash": header["hash"], "requireCanonical": True})
        for header in (first_header, second_header)
    ]
    if any(int(proof["storageProof"][0]["value"], 16) != amount for proof in proofs):
        raise ValueError("proof amount differs from the deposit")
    return {"finality": finality, "proofs": proofs}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("policy", type=Path)
    parser.add_argument("checkpoint", help="independently checked finalized Sepolia beacon root")
    parser.add_argument("--beacon", default="https://lodestar-sepolia.chainsafe.io")
    parser.add_argument("--execution", default=os.environ.get("SEPOLIA_RPC_URL"))
    parser.add_argument("--deposit", help="successful escrow deposit transaction hash")
    parser.add_argument("--note", type=Path, help="public output of inbound-run note")
    args = parser.parse_args()
    if not args.execution:
        parser.error("provide the Sepolia execution RPC via --execution or SEPOLIA_RPC_URL")
    if bool(args.deposit) != bool(args.note):
        parser.error("--deposit and --note are used together")
    try:
        print(json.dumps(collect(args)))
    except (KeyError, ValueError, OSError) as error:
        parser.exit(1, f"{error}\n")


if __name__ == "__main__":
    main()
