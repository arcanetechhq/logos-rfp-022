use std::{collections::HashMap, time::Duration};

use lb_binary_codec::canonical::BinaryEncode as _;
use lb_cryptarchia_engine::{EpochConfig, EraSchedule};
use lb_groth16::Fr;
use lb_key_management_system_keys::keys::{Ed25519Key, UnverifiedEd25519PublicKey, ZkKey};
use lb_ledger::{Config, config, mantle::sdp};
use lb_time::era::EraTiming;
use lb_utils::math::{NonNegativeRatio, ProperFraction};
use logos_blockchain_core::{
    block::{SignedHeader, UncleHeaders, genesis::GenesisBlockBuilder},
    era::{AcceptedForkDigests, ForkDigest},
    mantle::{
        Note, Op, OpProof, Utxo,
        gas::GasProfile,
        ledger::{Inputs, Outputs},
        ops::{
            channel::{
                ChannelId, MsgId,
                inscribe::{Inscription, InscriptionOp},
            },
            leader_claim::VoucherCm,
            transfer::TransferOp,
        },
        traits::Hashable as _,
        transactions::{GenesisParameters, GenesisTime, OpProofs, Ops},
    },
    proofs::leader_proof::{Groth16LeaderProof, LeaderPrivate, LeaderPublic, check_winning},
    sdp::{MinStake, ServiceParameters, ServiceType},
};
use time::OffsetDateTime;

use super::*;

const FORK: ForkDigest = ForkDigest::new([0x11; 32]);
const CHANNEL: [u8; 32] = [0x22; 32];
const LEZ_PAYLOAD: &[u8] = include_bytes!("../../../evidence/accepted-burn-block.borsh");

#[test]
fn validates_native_history_and_keeps_branch_changes_provisional() {
    let stake_key = ZkKey::new(Fr::from(1u64));
    let fee_key = ZkKey::new(Fr::from(2u64));
    let genesis = make_genesis(&stake_key, &fee_key);
    let genesis_id = Header::genesis(&genesis).id();
    let stake = genesis.transfer().operation().utxo_by_index(0).unwrap();
    let fee_note = genesis.transfer().operation().utxo_by_index(1).unwrap();
    let eras = fixture_eras();
    assert!(matches!(
        SourceReplay::from_genesis([0x99; 32].into(), genesis.clone(), eras.clone()),
        Err(ReplayError::WrongGenesis)
    ));
    let mut replay = SourceReplay::from_genesis(genesis_id, genesis, eras).unwrap();
    let observation_slot = 2_000u64.into();

    let leader = Ed25519Key::from_bytes(&[11; 32]);
    let (tx, message) = inscription(&leader, &fee_key, fee_note, MsgId::root(), LEZ_PAYLOAD);
    let first_block = build_block(&replay, genesis_id, stake, &stake_key, &leader, vec![tx]);
    let first = first_block.header().id();
    let first_slot = first_block.header().slot();
    let proof = first_block.header().leader_proof().clone();
    let make_empty = |parent, slot, uncles| {
        SourceBlock::create(
            parent,
            slot,
            uncles,
            proof.clone(),
            BlockTransactions::empty(),
            replay
                .ledger
                .era_schedule()
                .genesis()
                .parameters
                .max_transactions_size,
            &leader,
        )
        .unwrap()
    };
    let wrong_slot = make_empty(
        genesis_id,
        (u64::from(first_slot) + 1).into(),
        UncleHeaders::empty(),
    );
    let wrong_parent = make_empty([0xff; 32].into(), first_slot, UncleHeaders::empty());
    let with_uncle = make_empty(
        genesis_id,
        first_slot,
        UncleHeaders::try_new(
            vec![SignedHeader::new(
                first_block.header().clone(),
                *first_block.signature(),
            )],
            1,
        )
        .unwrap(),
    );
    assert!(matches!(
        SourceBlock::reconstruct(
            first_block.header().clone(),
            UncleHeaders::empty(),
            BlockTransactions::empty(),
            *first_block.signature(),
            u32::MAX,
        ),
        Err(logos_blockchain_core::block::Error::BodyRootMismatch)
    ));
    let mut signature_tampered = serde_json::to_value(&first_block).unwrap();
    let different_signature = Ed25519Key::from_bytes(&[91; 32]).sign_payload(b"wrong header");
    signature_tampered["signature"] = serde_json::to_value(different_signature).unwrap();
    assert!(serde_json::from_value::<SourceBlock>(signature_tampered).is_err());
    assert!(matches!(
        replay.apply(first_block.clone(), Slot::genesis()),
        Err(ReplayError::FutureSlot)
    ));
    assert!(matches!(
        replay.apply(wrong_slot, observation_slot),
        Err(ReplayError::Ledger(LedgerError::InvalidProof))
    ));
    assert!(matches!(
        replay.apply(wrong_parent, observation_slot),
        Err(ReplayError::Ledger(LedgerError::ParentNotFound(_)))
    ));
    assert!(matches!(
        replay.apply(with_uncle, observation_slot),
        Err(ReplayError::UnsupportedUncles)
    ));
    assert_eq!(replay.selected_tip(), genesis_id);
    assert!(replay.ledger.state(&first).is_none());
    let valid_serialized = serde_json::to_vec(&first_block).unwrap();
    let update = replay
        .apply(
            serde_json::from_slice(&valid_serialized).unwrap(),
            observation_slot,
        )
        .unwrap();
    assert_eq!(update.added_to_selected_branch, [first]);
    assert!(update.removed_from_selected_branch.is_empty());
    let first_state = replay.ledger.state(&first).unwrap();
    assert_eq!(
        first_state
            .mantle_ledger()
            .channels()
            .channel_state(&ChannelId::from(CHANNEL))
            .unwrap()
            .tip_message,
        message,
    );
    assert!(
        matches!(replay.apply(first_block, observation_slot), Err(ReplayError::AlreadyApplied(id)) if id == first)
    );

    let first_child = build_block(&replay, first, stake, &stake_key, &leader, vec![]);
    // Entering online mode here deliberately assumes bootstrap already happened.
    let mut isolated_view = SourceReplay {
        ledger: replay.ledger.clone(),
        consensus: replay.consensus.clone().online().0,
    };
    isolated_view.apply(first_child, observation_slot).unwrap();
    assert_eq!(isolated_view.consensus.lib(), first);

    let other_leader = Ed25519Key::from_bytes(&[12; 32]);
    let competing_block = build_block(
        &replay,
        genesis_id,
        stake,
        &stake_key,
        &other_leader,
        vec![],
    );
    let competing = competing_block.header().id();
    assert_eq!(competing_block.header().slot(), first_slot);
    assert_ne!(competing, first);
    assert!(
        !replay
            .prefers_alternative([competing_block.clone()], observation_slot)
            .unwrap()
    );
    let mut alternative = SourceReplay {
        ledger: replay.ledger.clone(),
        consensus: replay.consensus.clone(),
    };
    alternative
        .apply(competing_block.clone(), observation_slot)
        .unwrap();
    let competing_child = build_block(
        &alternative,
        competing,
        stake,
        &stake_key,
        &other_leader,
        vec![],
    );
    assert!(
        replay
            .prefers_alternative(
                [competing_block.clone(), competing_child.clone()],
                observation_slot
            )
            .unwrap()
    );
    assert!(matches!(
        replay.prefers_alternative(
            [competing_block.clone(), competing_block.clone()],
            observation_slot
        ),
        Err(ReplayError::NonContiguousFork)
    ));
    assert!(matches!(
        replay.prefers_alternative([competing_block.clone()], Slot::genesis()),
        Err(ReplayError::FutureSlot)
    ));
    assert_eq!(replay.selected_tip(), first);
    assert!(replay.ledger.state(&competing).is_none());

    let update = replay.apply(competing_block, observation_slot).unwrap();
    assert!(update.added_to_selected_branch.is_empty());
    assert_eq!(replay.selected_tip(), first);

    let tip = competing_child.header().id();
    let update = replay.apply(competing_child, observation_slot).unwrap();
    assert_eq!(update.removed_from_selected_branch, [first]);
    assert_eq!(update.added_to_selected_branch, [competing, tip]);
    assert_eq!(replay.selected_tip(), tip);
    assert_eq!(replay.consensus.lib(), genesis_id);
    assert_eq!(replay.consensus.clone().online().0.lib(), competing);
    assert!(
        replay
            .ledger
            .state(&tip)
            .unwrap()
            .mantle_ledger()
            .channels()
            .channel_state(&ChannelId::from(CHANNEL))
            .is_none()
    );
}

#[test]
#[ignore = "takes the proved LEZ block from the settlement fixture"]
fn exports_selected_lez_payloads_from_competing_native_histories() {
    let input = std::env::var_os("RFP022_SETTLEMENT_PAYLOAD").expect("settlement payload path");
    let next_input = std::env::var_os("RFP022_SETTLEMENT_NEXT_PAYLOAD").expect("next payload path");
    let output = std::env::var_os("RFP022_REPLAY_FIXTURE").expect("replay output path");
    let selected_payload = std::fs::read(input).unwrap();
    let next_payload = std::fs::read(next_input).unwrap();
    assert_ne!(selected_payload, LEZ_PAYLOAD);
    assert_ne!(next_payload, selected_payload);
    let stake_key = ZkKey::new(Fr::from(1u64));
    let fee_key = ZkKey::new(Fr::from(2u64));
    let genesis = make_genesis(&stake_key, &fee_key);
    let genesis_id = Header::genesis(&genesis).id();
    let stake = genesis.transfer().operation().utxo_by_index(0).unwrap();
    let fee_note = genesis.transfer().operation().utxo_by_index(1).unwrap();
    let mut replay = SourceReplay::from_genesis(genesis_id, genesis, fixture_eras()).unwrap();
    let observation_slot = 2_000u64.into();

    let leader = Ed25519Key::from_bytes(&[11; 32]);
    let (tx, losing_message) = inscription(&leader, &fee_key, fee_note, MsgId::root(), LEZ_PAYLOAD);
    let losing_block = build_block(&replay, genesis_id, stake, &stake_key, &leader, vec![tx]);
    let displaced_slot = u64::from(losing_block.header().slot());
    let losing = replay.apply(losing_block, observation_slot).unwrap();
    assert_eq!(losing.added_to_selected_branch, [losing.block_id]);
    assert_eq!(losing.inscriptions.len(), 1);
    assert_eq!(losing.inscriptions[0].id(), losing_message);

    let other_leader = Ed25519Key::from_bytes(&[12; 32]);
    let (tx, selected_message) = inscription(
        &other_leader,
        &fee_key,
        fee_note,
        MsgId::root(),
        &selected_payload,
    );
    let next_fee_note = tx
        .op_refs_iter()
        .find_map(|op| match op {
            logos_blockchain_core::mantle::OpRef::Transfer(transfer) => transfer.utxo_by_index(0),
            _ => None,
        })
        .unwrap();
    let selected_block = build_block(
        &replay,
        genesis_id,
        stake,
        &stake_key,
        &other_leader,
        vec![tx],
    );
    let selected = replay.apply(selected_block, observation_slot).unwrap();
    assert_eq!(selected.inscriptions.len(), 1);
    assert_eq!(selected.inscriptions[0].id(), selected_message);
    let (tx, next_message) = inscription(
        &other_leader,
        &fee_key,
        next_fee_note,
        selected_message,
        &next_payload,
    );
    let child = build_block(
        &replay,
        selected.block_id,
        stake,
        &stake_key,
        &other_leader,
        vec![tx],
    );
    let selected_tip = child.header().id();
    let slot = u64::from(child.header().slot());
    let chosen = replay.apply(child, observation_slot).unwrap();
    assert_eq!(chosen.removed_from_selected_branch, [losing.block_id]);
    assert_eq!(replay.selected_tip(), selected_tip);
    assert_eq!(replay.consensus.lib(), genesis_id);
    let selected_state = replay.ledger.state(&selected_tip).unwrap();
    let channel = selected_state
        .mantle_ledger()
        .channels()
        .channel_state(&ChannelId::from(CHANNEL))
        .unwrap();
    assert_eq!(channel.tip_message, next_message);
    let selected_op = &selected.inscriptions[0];
    assert_eq!(chosen.inscriptions.len(), 1);
    let next_op = &chosen.inscriptions[0];
    assert_eq!(next_op.channel_id, ChannelId::from(CHANNEL));
    assert_eq!(next_op.parent, selected_op.id());
    assert_eq!(next_op.id(), next_message);
    assert_eq!(&next_op.inscription[..], next_payload);
    let displaced_op = &losing.inscriptions[0];
    assert_eq!(selected_op.channel_id, ChannelId::from(CHANNEL));
    assert_eq!(&selected_op.inscription[..], selected_payload);
    assert_eq!(&displaced_op.inscription[..], LEZ_PAYLOAD);
    let record = serde_json::json!({
        "genesis": hex::encode(genesis_id.as_ref()),
        "channel": hex::encode(CHANNEL),
        "selected_tip": hex::encode(selected_tip.as_ref()),
        "slot": slot,
        "selected_message": hex::encode(next_message.as_ref()),
        "selected_payloads": [
            hex::encode(&selected_op.inscription[..]),
            hex::encode(&next_op.inscription[..]),
        ],
        "displaced_payload": hex::encode(&displaced_op.inscription[..]),
        "displaced_tip": hex::encode(losing.block_id.as_ref()),
        "displaced_message": hex::encode(losing_message.as_ref()),
        "displaced_slot": displaced_slot,
    });
    std::fs::write(output, serde_json::to_vec_pretty(&record).unwrap()).unwrap();
    println!(
        "PASS ordered Logos inscriptions match the selected Mantle history and displaced bytes remain provisional"
    );
}

fn make_genesis(stake: &ZkKey, fees: &ZkKey) -> GenesisTx {
    let parameters = GenesisParameters {
        chain_id: "rfp022-replay-fixture".to_owned().try_into().unwrap(),
        start_time: GenesisTime::new(0),
        epoch_nonce: Fr::from(0u64),
        pow_reward_pool: 1_000_000_000,
        consensus_excluded_keys: lb_ledger::ConsensusExcludedKeys::from(fees.to_public_key()),
    };
    let op = InscriptionOp {
        channel_id: ChannelId::from([0; 32]),
        inscription: Inscription::try_new(parameters.encode_to_vec(), 1_000_000).unwrap(),
        parent: MsgId::root(),
        signer: UnverifiedEd25519PublicKey::from_bytes(&[0; 32]).unwrap(),
    };
    GenesisBlockBuilder::new()
        .add_notes([
            Note::new(1_000_000_000, stake.to_public_key()),
            Note::new(1_000_000_000, fees.to_public_key()),
        ])
        .set_inscription(op)
        .build()
        .unwrap()
        .genesis_tx()
        .clone()
}

fn inscription(
    key: &Ed25519Key,
    fee_key: &ZkKey,
    fee_note: Utxo,
    parent: MsgId,
    payload: &[u8],
) -> (SignedOps<Preverified, StandardMode>, MsgId) {
    let op = InscriptionOp {
        channel_id: ChannelId::from(CHANNEL),
        inscription: Inscription::try_new(payload.to_owned(), 1_000_000).unwrap(),
        parent,
        signer: key.public_key().into_unverified(),
    };
    let message = op.id();
    let spend = TransferOp::new(
        Inputs::try_new(vec![fee_note.id()]).unwrap(),
        Outputs::new([Note::new(
            fee_note.note.value.checked_sub(1_000_000).unwrap(),
            fee_key.to_public_key(),
        )]),
    );
    let ops = Ops::new(FORK, [Op::Transfer(spend), Op::ChannelInscribe(op)]);
    let hash = ops.hash();
    let proofs = OpProofs::from([
        OpProof::ZkSig(ZkKey::multi_sign(std::slice::from_ref(fee_key), &hash.to_fr()).unwrap()),
        OpProof::Ed25519Sig(key.sign_payload(hash.as_signing_bytes())),
    ]);
    let tx = SignedOps::from_parts(ops, proofs)
        .unwrap()
        .preverify()
        .unwrap();
    (tx, message)
}

fn build_block(
    replay: &SourceReplay,
    parent: HeaderId,
    stake: Utxo,
    stake_key: &ZkKey,
    signing_key: &Ed25519Key,
    transactions: Vec<SignedOps<Preverified, StandardMode>>,
) -> SourceBlock {
    let eras = replay.ledger.era_schedule();
    let state = replay.ledger.state(&parent).unwrap();
    let latest = state.latest_utxos();
    let first_slot = u64::from(state.slot()) + 1;
    for slot in first_slot..first_slot + 1_000 {
        let epoch = state
            .epoch_state_for_slot::<HeaderId>(slot.into(), eras)
            .unwrap();
        let public = LeaderPublic::new(
            epoch.utxo_merkle_root(),
            latest.root(),
            epoch.nonce,
            slot,
            epoch.lottery_0,
            epoch.lottery_1,
        );
        if !check_winning(
            stake,
            public,
            &stake_key.to_public_key(),
            *stake_key.as_fr(),
        ) {
            continue;
        }
        let witness = LeaderPrivate::new(
            public,
            stake,
            &epoch.utxo_merkle_path(&stake).unwrap(),
            &latest.path(&stake.id()).unwrap(),
            *stake_key.as_fr(),
            &signing_key.public_key(),
        );
        let proof = Groth16LeaderProof::prove(witness, VoucherCm::default()).unwrap();
        let config = config_for_slot(eras, slot.into());
        return SourceBlock::create(
            parent,
            slot.into(),
            UncleHeaders::empty(),
            proof,
            BlockTransactions::try_new(transactions, config.max_transactions).unwrap(),
            config.max_transactions_size,
            signing_key,
        )
        .unwrap();
    }
    panic!("the fixture stake did not win a slot");
}

fn fixture_eras() -> Arc<EraScheduledConfig> {
    let fraction = |num, denom| ProperFraction::new(num, denom).unwrap();
    let config = Config {
        epoch_config: EpochConfig {
            epoch_stake_distribution_stabilization: 3.try_into().unwrap(),
            epoch_period_nonce_buffer: 3.try_into().unwrap(),
            epoch_period_nonce_stabilization: 4.try_into().unwrap(),
        },
        consensus_config: lb_cryptarchia_engine::Config::new(
            1.try_into().unwrap(),
            NonNegativeRatio::new(1, 10.try_into().unwrap()),
            1.0.try_into().unwrap(),
            12,
            4,
        ),
        max_transactions: 32,
        max_transactions_size: 2_000_000,
        max_op_data_size: 1_000_000,
        sdp_config: sdp::Config {
            service_params: Arc::new(HashMap::from([(
                ServiceType::BlendNetwork,
                ServiceParameters {
                    inactivity_period: 2.try_into().unwrap(),
                    epoch: 0.into(),
                },
            )])),
            service_rewards_params: sdp::ServiceRewardsParameters {
                blend: sdp::rewards::blend::RewardsParameters {
                    rounds_per_epoch: 100.try_into().unwrap(),
                    message_frequency_per_round: 1.0.try_into().unwrap(),
                    cover_replication_factor: 0,
                    num_blend_layers: 3.try_into().unwrap(),
                    minimum_network_size: 1.try_into().unwrap(),
                    data_replication_factor: 0,
                    activity_threshold_sensitivity: 1,
                },
            },
            min_stake: MinStake {
                threshold: 1,
                timestamp: 0,
            },
        },
        pow_config: config::PoWConfig {
            blend: config::BlendPoWConfig {
                base_difficulty: config::ModulusShift::new::<19>(),
                target_transactions_per_block: 1.try_into().unwrap(),
                max_step: 1.try_into().unwrap(),
                damping_num: 1.try_into().unwrap(),
                damping_den_offset: 0,
            },
            reward: config::RewardPoWConfig {
                minimum_difficulty: config::ModulusShift::new::<26>(),
                ema_smoothing_factor: 9,
                ema_smoothing_precision: 10.try_into().unwrap(),
                target_claims_per_block: 1,
                rate_num: 0,
                rate_den: 1.try_into().unwrap(),
                target_claim_per_block: 1.try_into().unwrap(),
                pow_share: 0,
                share_den: 1.try_into().unwrap(),
                slot_window: 100.try_into().unwrap(),
            },
        },
        execution_market: config::ExecutionMarketConfig {
            max_gas_per_block: config::MaxGasPerBlock::new(3_193_460).unwrap(),
            fee_adjustment_rate: fraction(1, 8.try_into().unwrap()),
            ema_smoothing_factor: fraction(9, 10.try_into().unwrap()),
        },
        storage_market: config::StorageMarketConfig {
            max_adjustment_factor: fraction(1, 8.try_into().unwrap()),
            ema_smoothing_factor: fraction(1, 2.try_into().unwrap()),
        },
        block_rewards: config::BlockRewardsConfig::new(
            3_000_000_000,
            120_000_000.try_into().unwrap(),
            10_512,
            NonNegativeRatio::new(62_500, 657.try_into().unwrap()),
            fraction(6, 10.try_into().unwrap()),
        )
        .unwrap(),
        gas_profile: GasProfile {
            transfer: 590.into(),
            channel_config: 56.into(),
            channel_inscribe: 56.into(),
            channel_deposit: 590.into(),
            channel_withdraw: 56.into(),
            channel_transfer: 56.into(),
            sdp_declare: 646.into(),
            sdp_withdraw: 590.into(),
            sdp_active: 590.into(),
            leader_claim: 580.into(),
            claim_pow_reward: 1.into(),
        },
        accepted_fork_digests: AcceptedForkDigests::new(vec![FORK]),
    };
    let timing = EraTiming {
        slot_duration: Duration::from_secs(1),
        epoch_length: config.epoch_length().try_into().unwrap(),
    };
    Arc::new(
        EraSchedule::new(OffsetDateTime::UNIX_EPOCH, timing, Default::default())
            .unwrap()
            .map(|_| config.clone()),
    )
}
