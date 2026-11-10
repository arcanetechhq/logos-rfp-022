//! Native channel authentication on synthetic data. This does not establish chain validity/finality.
use lb_cryptarchia_engine::Slot;
use lb_key_management_system_keys::keys::Ed25519Key;
use logos_blockchain_core::{
    block::{UncleHeaders, body_root},
    era::ForkDigest,
    mantle::{
        Op,
        channel::Channels,
        ledger::{PreverifiableOperation, VerifiableOperation, verification_mode::StandardMode},
        ops::{
            SignedOperation,
            channel::{
                ChannelId, MsgId,
                inscribe::{
                    Inscription, InscriptionExecutionContext, InscriptionOp,
                    InscriptionPreverificationContext, InscriptionValidationContext,
                },
            },
        },
        traits::Hashable,
        transactions::{MantleTxBuilder, hash::TxHashView, states::Unverified},
    },
};
fn main() {
    let start = std::time::Instant::now();
    let file = std::env::args()
        .nth(1)
        .expect("path to actual LEZ block bytes");
    let bytes = std::fs::read(file).unwrap();
    let key = Ed25519Key::from_bytes(&[15; 32]); // public synthetic fixture key, never production
    let channel = ChannelId::from([0x22; 32]);
    let make_op = |payload: Vec<u8>, parent, signer: &Ed25519Key| InscriptionOp {
        channel_id: channel,
        inscription: Inscription::try_new(payload, 1_000_000).unwrap(),
        parent,
        signer: signer.public_key().into_unverified(),
    };
    let fork = ForkDigest::from([0x33; 32]); // fixture, no authenticated era initialization
    let sign = |op: InscriptionOp, signer: &Ed25519Key| {
        let ops = MantleTxBuilder::new()
            .push_op(Op::ChannelInscribe(op.clone()))
            .unwrap()
            .build(fork)
            .unwrap();
        let hash = ops.hash();
        let view = TxHashView::new(hash);
        let signed = SignedOperation::<_, Unverified, StandardMode>::new(
            op,
            signer.sign_payload(view.as_bytes().as_ref()),
        );
        (ops, view, signed)
    };
    let root = make_op(b"synthetic channel genesis".to_vec(), MsgId::root(), &key);
    let root_id = root.id();
    let (_, root_view, root_signed) = sign(root, &key);
    let pre = root_signed
        .into_preverified(&InscriptionPreverificationContext {
            tx_hash_view: &root_view,
        })
        .unwrap();
    let verified = pre
        .into_verified(&InscriptionValidationContext {
            channels: &Channels::new(),
            block_slot: Slot::from(0),
        })
        .unwrap();
    let (context, _) = verified
        .execute(InscriptionExecutionContext {
            channels: Channels::new(),
            block_slot: Slot::from(0),
        })
        .unwrap();
    let channels = context.channels;
    let op = make_op(bytes.clone(), root_id, &key);
    let inscription_id = op.id();
    let (ops, view, signed) = sign(op.clone(), &key);
    assert!(
        signed
            .preverify(&InscriptionPreverificationContext {
                tx_hash_view: &view
            })
            .is_ok()
    );
    let pre = signed
        .clone()
        .into_preverified(&InscriptionPreverificationContext {
            tx_hash_view: &view,
        })
        .unwrap();
    assert!(
        pre.verify(&InscriptionValidationContext {
            channels: &channels,
            block_slot: Slot::from(1)
        })
        .is_ok()
    );
    let valid = pre
        .into_verified(&InscriptionValidationContext {
            channels: &channels,
            block_slot: Slot::from(1),
        })
        .unwrap();
    let (after, _) = valid
        .execute(InscriptionExecutionContext {
            channels: channels.clone(),
            block_slot: Slot::from(1),
        })
        .unwrap();
    assert_eq!(
        after.channels.channel_state(&channel).unwrap().tip_message,
        inscription_id
    );
    println!(
        "PASS native inscription signature + accredited signer + parent + channel state update"
    );
    let mut altered = bytes;
    altered[0] ^= 1;
    let changed_op = make_op(altered, root_id, &key);
    assert_ne!(changed_op.id(), inscription_id);
    let (changed_ops, changed_view, _) = sign(changed_op, &key);
    assert!(
        signed
            .preverify(&InscriptionPreverificationContext {
                tx_hash_view: &changed_view
            })
            .is_err()
    );
    assert_ne!(
        body_root(&UncleHeaders::empty(), &[ops.clone()]),
        body_root(&UncleHeaders::empty(), &[changed_ops])
    );
    println!(
        "PASS payload mutation changes native message id, tx signing hash and body_root; old signature rejected"
    );
    let (_, bad_view, bad_signed) = sign(
        make_op(op.inscription.to_vec(), MsgId::from([7; 32]), &key),
        &key,
    );
    let bad_pre = bad_signed
        .into_preverified(&InscriptionPreverificationContext {
            tx_hash_view: &bad_view,
        })
        .unwrap();
    assert!(
        bad_pre
            .verify(&InscriptionValidationContext {
                channels: &channels,
                block_slot: Slot::from(1)
            })
            .is_err()
    );
    let attacker = Ed25519Key::from_bytes(&[16; 32]);
    let (_, attack_view, attack_signed) = sign(
        make_op(op.inscription.to_vec(), root_id, &attacker),
        &attacker,
    );
    let attack_pre = attack_signed
        .into_preverified(&InscriptionPreverificationContext {
            tx_hash_view: &attack_view,
        })
        .unwrap();
    assert!(
        attack_pre
            .verify(&InscriptionValidationContext {
                channels: &channels,
                block_slot: Slot::from(1)
            })
            .is_err()
    );
    println!("PASS valid signature with wrong parent or unaccredited key rejected");
    let mut rotating = channels.clone();
    let mut config = rotating.channel_state(&channel).unwrap().clone();
    let keys: logos_blockchain_core::mantle::ops::channel::UnverifiedChannelKeys = [
        key.public_key().into_unverified(),
        attacker.public_key().into_unverified(),
    ]
    .into();
    config.accredited_keys = keys.into();
    config.posting_timeframe = 1.into();
    rotating.channels.insert_mut(channel, config);
    let (_, first_view, first_signed) = sign(op.clone(), &key);
    let first = first_signed
        .into_preverified(&InscriptionPreverificationContext {
            tx_hash_view: &first_view,
        })
        .unwrap();
    assert!(
        first
            .verify(&InscriptionValidationContext {
                channels: &rotating,
                block_slot: Slot::from(1)
            })
            .is_err()
    );
    let (_, second_view, second_signed) = sign(
        make_op(op.inscription.to_vec(), root_id, &attacker),
        &attacker,
    );
    let second = second_signed
        .into_preverified(&InscriptionPreverificationContext {
            tx_hash_view: &second_view,
        })
        .unwrap();
    assert!(
        second
            .verify(&InscriptionValidationContext {
                channels: &rotating,
                block_slot: Slot::from(1)
            })
            .is_ok()
    );
    println!(
        "PASS native slot rotation rejects an accredited key outside its turn and accepts the scheduled key"
    );
    let out = serde_json::json!({"fixture":true,"channel_initialized_from":"synthetic local inscription", "msg_id":hex::encode(<[u8;32]>::from(inscription_id)), "tx_hash":hex::encode(view.as_bytes()), "body_root":hex::encode(<[u8;32]>::from(body_root(&UncleHeaders::empty(), &[ops]))),"elapsed_ms":start.elapsed().as_millis(),"ledger_validated":false,"pol_verified":false,"finality_verified":false,"release":false});
    std::fs::write(
        "evidence/native-auth.json",
        serde_json::to_vec_pretty(&out).unwrap(),
    )
    .unwrap();
    println!("{}", out);
}
