// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

/// Write-once deposit amounts at a stable slot for the RFP-021 storage proof.
library DepositNotes {
    error InvalidDepositNote();

    bytes32 internal constant NAMESPACE = keccak256("RFP022.deposits.v1");

    function slot(bytes32 commitment) internal pure returns (bytes32) {
        return keccak256(abi.encode(commitment, NAMESPACE));
    }

    function amount(bytes32 commitment) internal view returns (uint128 value) {
        bytes32 key = slot(commitment);
        assembly ("memory-safe") {
            value := sload(key)
        }
    }

    function record(bytes32 commitment, uint128 value) internal {
        if (commitment == bytes32(0) || value == 0 || amount(commitment) != 0) {
            revert InvalidDepositNote();
        }
        bytes32 key = slot(commitment);
        assembly ("memory-safe") {
            sstore(key, value)
        }
    }
}
