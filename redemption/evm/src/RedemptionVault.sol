// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {IERC20} from "@openzeppelin/contracts/token/ERC20/IERC20.sol";
import {SafeERC20} from "@openzeppelin/contracts/token/ERC20/utils/SafeERC20.sol";
import {ReentrancyGuard} from "@openzeppelin/contracts/utils/ReentrancyGuard.sol";
import {CheckpointContest} from "./CheckpointContest.sol";

/// ABI of the released RISC Zero Ethereum verifier. An invalid proof must revert.
interface IRiscZeroVerifier {
    function verify(bytes calldata seal, bytes32 imageId, bytes32 journalDigest) external view;
}

/// One immutable Logos program / Ethereum asset pairing.
/// The pinned source image verifies the direct or contest statement for this deployment.
contract RedemptionVault is ReentrancyGuard {
    using SafeERC20 for IERC20;
    using CheckpointContest for CheckpointContest.State;

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
    event CheckpointProposed(bytes32 indexed candidateId, uint64 indexed observationTime, uint256 deadline);
    event CheckpointChallenged(bytes32 indexed previousId, bytes32 indexed candidateId, uint256 deadline);
    event CheckpointContestClosed(bytes32 indexed candidateId);
    event Deposited(uint256 indexed depositId, bytes32 commitment, uint128 amount);
    event Redeemed(bytes32 indexed nullifier, address indexed recipient, uint128 amount);

    uint256 internal constant REDEMPTION_JOURNAL_BYTES = 188;
    bytes32 public constant PROPOSAL_JOURNAL = keccak256("RFP022/source/proposal/v1");
    bytes32 public constant CHALLENGE_JOURNAL = keccak256("RFP022/source/challenge/v1");
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
    bytes32 public immutable challengeImageId;
    uint64 public immutable challengePeriod;
    bytes32 public immutable redemptionImageId;
    bytes32 public immutable pairingId;
    address public immutable token; // zero is ETH
    uint256 public immutable destinationChain;

    bytes32 public checkpoint;
    uint64 public sourceSlot;
    uint256 public escrowed;
    uint256 public depositCount;
    CheckpointContest.State private contest;
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
        uint128[] memory denominations,
        bytes32 challengeImageId_,
        uint64 challengePeriod_
    ) {
        if (
            address(verifier_).code.length == 0 || redemptionImageId_ == bytes32(0)
                || sourceImageId_ == redemptionImageId_ || initialCheckpoint == bytes32(0)
                || logosGenesis == bytes32(0) || zone == bytes32(0) || bridgeProgram == bytes32(0)
                || denominations.length == 0 || (token_ != address(0) && token_.code.length == 0)
                || ((challengePeriod_ == 0) != (challengeImageId_ == bytes32(0)))
                || (challengePeriod_ != 0 && (
                    sourceImageId_ == bytes32(0) || challengeImageId_ == sourceImageId_
                        || challengeImageId_ == redemptionImageId_
                ))
        ) revert BadConfiguration();
        for (uint256 i; i < denominations.length; ++i) {
            uint128 value = denominations[i];
            if (value == 0 || denomination[value]) revert BadConfiguration();
            denomination[value] = true;
        }
        verifier = verifier_;
        sourceImageId = sourceImageId_;
        challengeImageId = challengeImageId_;
        challengePeriod = challengePeriod_;
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
        _requireSourceMode(false);
        bytes32 previous = checkpoint;
        _checkAdvance(next, exitRoot, finalizedSlot);
        verifier.verify(seal, sourceImageId, sha256(abi.encode(pairingId, previous, next, exitRoot, finalizedSlot)));
        _authenticate(next, exitRoot, finalizedSlot);
    }

    function pendingCheckpoint() external view returns (CheckpointContest.State memory) {
        return contest;
    }

    /// The observation time fixes the comparison horizon for the whole contest.
    function proposeCheckpoint(
        CheckpointContest.Candidate calldata candidate,
        uint64 observationTime,
        bytes calldata history,
        bytes calldata seal
    ) external {
        _requireSourceMode(true);
        _checkHistory(candidate.history, history);
        _checkAdvance(candidate.checkpoint, candidate.exitRoot, candidate.slot);
        contest.open(candidate, observationTime, challengePeriod);
        bytes32 candidateId = CheckpointContest.candidateId(observationTime, candidate);
        verifier.verify(
            seal,
            sourceImageId,
            sha256(abi.encode(PROPOSAL_JOURNAL, pairingId, checkpoint, sourceSlot, candidateId))
        );
        emit CheckpointProposed(candidateId, observationTime, contest.deadline);
    }

    /// The comparison proof must prefer the challenger at the same observation
    /// time. It may prove that the previous checkpoint still has to be retained.
    function challengeCheckpoint(
        bytes32 expectedCandidateId,
        CheckpointContest.Candidate calldata challenger,
        bytes calldata history,
        bytes calldata seal
    ) external {
        _requireSourceMode(true);
        _checkHistory(challenger.history, history);
        if (challenger.history == contest.candidate.history) revert InvalidCheckpoint();
        if (challenger.checkpoint != checkpoint || challenger.slot != sourceSlot || challenger.exitRoot != bytes32(0)) {
            _checkAdvance(challenger.checkpoint, challenger.exitRoot, challenger.slot);
        }
        contest.replace(challenger, expectedCandidateId, challengePeriod);
        bytes32 candidateId = CheckpointContest.candidateId(contest.observationTime, challenger);
        verifier.verify(seal, challengeImageId, _challengeDigest(expectedCandidateId, candidateId));
        emit CheckpointChallenged(expectedCandidateId, candidateId, contest.deadline);
    }

    function finalizeCheckpoint(bytes32 expectedCandidateId) external {
        _requireSourceMode(true);
        CheckpointContest.Candidate memory winner = contest.close(expectedCandidateId);
        if (winner.slot > sourceSlot) {
            _authenticate(winner.checkpoint, winner.exitRoot, winner.slot);
        }
        emit CheckpointContestClosed(expectedCandidateId);
    }

    function _requireSourceMode(bool optimistic) private view {
        if (sourceImageId == bytes32(0) || destinationChain != block.chainid || optimistic != (challengePeriod != 0)) {
            revert SourceHistoryUnavailable();
        }
    }

    function _checkAdvance(bytes32 next, bytes32 exitRoot, uint64 slot) private view {
        if (next == bytes32(0) || next == checkpoint || exitRoot == bytes32(0) || slot <= sourceSlot) {
            revert InvalidCheckpoint();
        }
    }

    function _checkHistory(bytes32 commitment, bytes calldata history) private pure {
        if (history.length == 0 || sha256(history) != commitment) revert InvalidCheckpoint();
    }

    function _challengeDigest(bytes32 incumbent, bytes32 challenger) private view returns (bytes32) {
        return sha256(abi.encode(CHALLENGE_JOURNAL, pairingId, checkpoint, sourceSlot, incumbent, challenger));
    }

    function _authenticate(bytes32 next, bytes32 exitRoot, uint64 slot) private {
        checkpoint = next;
        sourceSlot = slot;
        authenticatedRoots[exitRoot] = true;
        emit CheckpointAdvanced(next, slot, exitRoot);
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
