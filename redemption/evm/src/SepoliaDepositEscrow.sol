// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {DepositNotes} from "./DepositNotes.sol";

/// ETH fixture for the inbound test on Sepolia. It has no withdrawal path.
contract SepoliaDepositEscrow {
    error WrongChain();
    error InvalidAmount();
    event Deposited(bytes32 indexed commitment, uint128 amount);

    constructor() {
        if (block.chainid != 11155111) revert WrongChain();
    }

    function deposit(bytes32 commitment) external payable {
        if (msg.value > type(uint128).max) revert InvalidAmount();
        DepositNotes.record(commitment, uint128(msg.value));
        emit Deposited(commitment, uint128(msg.value));
    }

    function depositAmount(bytes32 commitment) external view returns (uint128) {
        return DepositNotes.amount(commitment);
    }
}
