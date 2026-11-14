// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";

/// ABI of the released RISC Zero Ethereum verifier. An invalid proof must revert.
interface IRiscZeroVerifier {
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view;
}

/// One immutable Logos program / Ethereum asset pairing.
/// The source guest must prove canonical history, full LEZ acceptance and exit root derivation.
contract RedemptionVault is ReentrancyGuard {
    using SafeERC20 for IERC20;

    error BadConfiguration();
    error SourceHistoryUnavailable();
    error InvalidCheckpoint();
    error UnknownExitRoot();
    error InvalidJournal();
    error AlreadyRedeemed();
    error InvalidDeposit();
    error InsufficientEscrow();
    error TransferFailed();

    event CheckpointAdvanced(bytes32 indexed checkpoint, uint64 indexed slot, bytes32 indexed exitRoot);
    event Deposited(uint256 indexed depositId, bytes32 commitment, uint128 amount);
    event Redeemed(bytes32 indexed nullifier, address indexed recipient, uint128 amount);

    uint256 internal constant REDEMPTION_JOURNAL_BYTES = 188;
    struct Release {
        bytes32 pairing;
        bytes32 root;
        bytes32 nullifier;
        address asset;
        uint128 amount;
        address recipient;
        address relayer;
        uint128 fee;
    }
    IRiscZeroVerifier public immutable verifier;
    bytes32 public immutable sourceImageId;
    bytes32 public immutable redemptionImageId;
    bytes32 public immutable pairingId;
    address public immutable token; // zero is ETH
    uint256 public immutable destinationChain;

    bytes32 public checkpoint;
    uint64 public sourceSlot;
    uint256 public escrowed;
    uint256 public depositCount;
    mapping(bytes32 => bool) public authenticatedRoots;
    mapping(bytes32 => bool) public spent;
    mapping(uint128 => bool) public denomination;

    constructor(
        IRiscZeroVerifier verifier_,
        bytes32 sourceImageId_,
        bytes32 redemptionImageId_,
        bytes32 initialCheckpoint,
        bytes32 logosGenesis,
        bytes32 zone,
        bytes32 bridgeProgram,
        address token_,
        uint128[] memory denominations
    ) {
        if (
            address(verifier_).code.length == 0 || redemptionImageId_ == bytes32(0)
                || sourceImageId_ == redemptionImageId_ || initialCheckpoint == bytes32(0)
                || logosGenesis == bytes32(0) || zone == bytes32(0) || bridgeProgram == bytes32(0)
                || denominations.length == 0 || (token_ != address(0) && token_.code.length == 0)
        ) revert BadConfiguration();
        for (uint256 i; i < denominations.length; ++i) {
            uint128 value = denominations[i];
            if (value == 0 || denomination[value]) revert BadConfiguration();
            denomination[value] = true;
        }
        verifier = verifier_;
        sourceImageId = sourceImageId_;
        redemptionImageId = redemptionImageId_;
        destinationChain = block.chainid;
        token = token_;
        checkpoint = initialCheckpoint;
        pairingId = sha256(
            abi.encodePacked(
                "RFP022/pairing/v1",
                logosGenesis,
                zone,
                bridgeProgram,
                bytes32(block.chainid),
                address(this),
                token_
            )
        );
    }

    /// The source proof's journal is ABI(pairing, previous, next, exitRoot, finalizedSlot).
    /// The verifier image is pinned at deployment. Zero leaves the vault disabled.
    function advanceCheckpoint(bytes32 next, bytes32 exitRoot, uint64 finalizedSlot, bytes calldata seal) external {
        if (sourceImageId == bytes32(0) || destinationChain != block.chainid) {
            revert SourceHistoryUnavailable();
        }
        bytes32 previous = checkpoint;
        if (next == bytes32(0) || next == previous || exitRoot == bytes32(0) || finalizedSlot <= sourceSlot) {
            revert InvalidCheckpoint();
        }
        verifier.verify(seal, sourceImageId, sha256(abi.encode(pairingId, previous, next, exitRoot, finalizedSlot)));
        checkpoint = next;
        sourceSlot = finalizedSlot;
        authenticatedRoots[exitRoot] = true;
        emit CheckpointAdvanced(next, finalizedSlot, exitRoot);
    }

    /// The deposit event commits to an opaque claim note. The inbound LEZ program consumes its attestation.
    function deposit(bytes32 commitment, uint128 amount) external payable nonReentrant {
        _requireLiveHistory();
        if (commitment == bytes32(0) || !denomination[amount]) revert InvalidDeposit();
        if (token == address(0)) {
            if (msg.value != amount) revert InvalidDeposit();
        } else {
            if (msg.value != 0) revert InvalidDeposit();
            IERC20 asset = IERC20(token);
            uint256 balance = asset.balanceOf(address(this));
            asset.safeTransferFrom(msg.sender, address(this), amount);
            if (asset.balanceOf(address(this)) != balance + amount) revert InvalidDeposit();
        }
        escrowed += amount;
        emit Deposited(++depositCount, commitment, amount);
    }

    /// The relayer may submit the proof. The proof fixes every payout argument.
    function release(bytes calldata journal, bytes calldata seal) external nonReentrant {
        _requireLiveHistory();
        Release memory request = _decode(journal);
        if (
            request.pairing != pairingId || request.asset != token || !denomination[request.amount]
                || request.recipient == address(0) || request.fee >= request.amount
                || ((request.fee == 0) != (request.relayer == address(0)))
        ) revert InvalidJournal();
        if (!authenticatedRoots[request.root]) revert UnknownExitRoot();
        if (spent[request.nullifier]) revert AlreadyRedeemed();
        if (escrowed < request.amount) revert InsufficientEscrow();

        verifier.verify(seal, redemptionImageId, sha256(journal));
        spent[request.nullifier] = true;
        escrowed -= request.amount;
        _pay(request.recipient, request.amount - request.fee);
        if (request.fee != 0) _pay(request.relayer, request.fee);
        emit Redeemed(request.nullifier, request.recipient, request.amount);
    }

    function _decode(bytes calldata journal) internal pure returns (Release memory) {
        if (journal.length != REDEMPTION_JOURNAL_BYTES) revert InvalidJournal();
        return Release({
            pairing: bytes32(journal[0:32]),
            root: bytes32(journal[32:64]),
            nullifier: bytes32(journal[64:96]),
            asset: address(bytes20(journal[96:116])),
            amount: _readLittleEndian128(journal[116:132]),
            recipient: address(bytes20(journal[132:152])),
            relayer: address(bytes20(journal[152:172])),
            fee: _readLittleEndian128(journal[172:188])
        });
    }

    function _requireLiveHistory() private view {
        if (sourceSlot == 0 || destinationChain != block.chainid) revert SourceHistoryUnavailable();
    }

    function _readLittleEndian128(bytes calldata data) private pure returns (uint128 result) {
        for (uint256 i = 0; i < 16; ++i) {
            result |= uint128(uint256(uint8(data[i])) << (8 * i));
        }
    }

    function _pay(address recipient, uint128 amount) private {
        if (token == address(0)) {
            (bool success,) = recipient.call{value: amount}("");
            if (!success) revert TransferFailed();
        } else {
            IERC20 asset = IERC20(token);
            uint256 fromBalance = asset.balanceOf(address(this));
            uint256 recipientBalance = asset.balanceOf(recipient);
            asset.safeTransfer(recipient, amount);
            if (asset.balanceOf(address(this)) != fromBalance - amount
                || asset.balanceOf(recipient) != recipientBalance + amount) revert TransferFailed();
        }
    }
}
