// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

/// Tracks the contest for a single finalized predecessor. The caller verifies
/// the candidate and any replacement before letting the transaction commit.
library CheckpointContest {
    error ContestAlreadyOpen();
    error NoOpenContest();
    error InvalidObservationTime();
    error ChallengeWindowClosed();
    error ChallengeWindowOpen();
    error CandidateChanged();

    struct Candidate {
        bytes32 history;
        bytes32 checkpoint;
        bytes32 exitRoot;
        uint64 slot;
    }

    struct State {
        uint64 observationTime;
        uint256 deadline;
        Candidate candidate;
    }

    function candidateId(uint64 observationTime, Candidate memory candidate) internal pure returns (bytes32) {
        return keccak256(abi.encode(observationTime, candidate));
    }

    function open(State storage state, Candidate memory candidate, uint64 observationTime, uint64 period) internal {
        if (state.deadline != 0) revert ContestAlreadyOpen();
        if (
            observationTime <= state.observationTime || observationTime > block.timestamp
                || block.timestamp - observationTime >= period
        ) revert InvalidObservationTime();
        state.observationTime = observationTime;
        state.deadline = block.timestamp + period;
        state.candidate = candidate;
    }

    function replace(State storage state, Candidate memory candidate, bytes32 expected, uint64 period) internal {
        if (state.deadline == 0) revert NoOpenContest();
        if (block.timestamp >= state.deadline) revert ChallengeWindowClosed();
        if (candidateId(state.observationTime, state.candidate) != expected) revert CandidateChanged();
        state.candidate = candidate;
        state.deadline = block.timestamp + period;
    }

    function close(State storage state, bytes32 expected) internal returns (Candidate memory winner) {
        if (state.deadline == 0) revert NoOpenContest();
        if (block.timestamp < state.deadline) revert ChallengeWindowOpen();
        winner = state.candidate;
        if (candidateId(state.observationTime, winner) != expected) revert CandidateChanged();
        delete state.candidate;
        delete state.deadline;
    }
}
