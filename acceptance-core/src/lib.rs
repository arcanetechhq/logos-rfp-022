//! Executable experimental verifier for a deliberately restricted native private action.
//! NOT a general LEZ block verifier: no fees, clock, async, upgrades or source-chain consensus.
use std::collections::{BTreeMap,BTreeSet};
use borsh::{BorshDeserialize,BorshSerialize};
use lee_core::{account::{Account,AccountId,Nonce}, Commitment,Nullifier,CommitmentSetDigest,CommitmentPosition,EncryptedNote,
    ProvenExecution,ProgramImageClaim,program::{PdaSeed,Publication,ProgramHeader,PROGRAM_LOADER_ACCOUNT_ID,ReceiveInput,Transition},
    execution_state::{ExecutionEnvironment,ExecutionError,PublicExecutionContext,PublicPart,TransitionView}};
use program_loader_core::LoaderActorState;
use k256::ecdsa::signature::hazmat::PrehashVerifier;
use risc0_zkvm::sha::{Impl,Sha256};
pub mod encoding { pub mod persistent; }
#[path="native/merkle_tree/mod.rs"] pub mod merkle_tree;
#[derive(BorshSerialize,BorshDeserialize)]
#[cfg_attr(feature="host",derive(Clone))]
pub struct Commitments { pub tree:merkle_tree::MerkleTree,pub positions:BTreeMap<[u8;32],usize>,pub roots:BTreeSet<CommitmentSetDigest> }
/// Byte-for-byte native V03State encoding. Tested against serialize + native apply_state_diff.
#[derive(BorshSerialize,BorshDeserialize)]
#[cfg_attr(feature="host",derive(Clone))]
pub struct State {
    pub accounts:BTreeMap<AccountId,Account>,pub private:(Commitments,BTreeSet<[u8;32]>),
    pub publications:BTreeMap<CommitmentPosition,Publication>,pub recovery:BTreeMap<AccountId,EncryptedNote>,
}
#[derive(BorshSerialize,BorshDeserialize)]
#[cfg_attr(feature="host",derive(Clone))]
pub struct Fee { pub payer:AccountId,pub gas_limit:u64,pub tip:u64,pub max_fee:u128 }
#[derive(BorshSerialize,BorshDeserialize)]
#[cfg_attr(feature="host",derive(Clone))]
pub enum Evidence { Key([u8;32]),Pda{program:AccountId,seed:PdaSeed} }
#[derive(BorshSerialize,BorshDeserialize)]
#[cfg_attr(feature="host",derive(Clone))]
pub struct Message { pub public_context:PublicExecutionContext,pub execution:ProvenExecution,pub nonces:BTreeMap<AccountId,Nonce>,pub fee:Option<Fee>,pub admission_evidence:Vec<Evidence> }
#[derive(BorshSerialize,BorshDeserialize)]
#[cfg_attr(feature="host",derive(Clone))]
pub struct Witness {pub signatures:Vec<([u8;64],[u8;32])>,pub proof:Vec<u8>}
#[derive(BorshSerialize,BorshDeserialize)]
#[cfg_attr(feature="host",derive(Clone))]
pub struct Tx {pub message:Message,pub witness:Witness}
#[derive(BorshSerialize,BorshDeserialize)]
#[cfg_attr(feature="host",derive(Clone))]
pub struct Input {pub before:Vec<u8>,pub tx:Tx,pub token_image:[u32;8],pub block:u64,pub timestamp:u64}
#[derive(Debug,Clone,PartialEq,Eq,BorshSerialize,BorshDeserialize)]
pub struct Journal {pub before:[u8;32],pub after:[u8;32],pub exit_root:[u8;32],pub tx_hash:[u8;32],pub block:u64,pub timestamp:u64}
pub fn hash(bytes:&[u8])->[u8;32] {Impl::hash_bytes(bytes).as_bytes().try_into().unwrap()}
pub fn state_digest(bytes:&[u8])->[u8;32] {hash(&[b"RFP022/full-native-V03State/v1".as_slice(),bytes].concat())}
struct Backend<'a>(&'a State);
impl ExecutionEnvironment for Backend<'_> {
    type Error=ExecutionError;
    fn public_actor_state(&mut self,actor:lee_core::account::Actor)->Result<lee_core::account::ActorState,ExecutionError>{
        assert_eq!(actor,burn_core::sink(),"only the exit sink is supported");
        Ok(self.0.accounts.get(&actor.account_id).unwrap().data.actor_state(actor.program_account_id).clone())
    }
    fn admits(&mut self,id:AccountId)->Result<bool,ExecutionError>{Ok(self.0.accounts.contains_key(&id))}
    fn handle_message(&mut self,input:&ReceiveInput,_:&TransitionView<'_>)->Result<Transition,ExecutionError>{
        assert_eq!(input.receiver,burn_core::sink());
        let msg=borsh::from_slice(&input.message).unwrap();
        Ok(Transition{input:input.clone(),response:burn_core::handle(input,msg)})
    }
}
/// Caller MUST cryptographically verify the native PPE journal before calling this function.
/// The guest does so through env::verify. The native differential test uses Receipt::verify.
pub fn apply(input:Input)->(State,Journal) {
    let mut state:State=borsh::from_slice(&input.before).expect("full native state encoding");
    assert_eq!(borsh::to_vec(&state).unwrap(),input.before,"canonical round trip");
    let tx=&input.tx; let m=&tx.message; let ex=&m.execution;
    assert!(m.admission_evidence.is_empty() && !ex.private_actions.is_empty());
    assert!(ex.public_root.is_none() && ex.async_messages.is_empty() && ex.recovery_bindings.is_empty() && ex.proven_fee.is_none());
    assert_eq!(m.public_context.actors.iter().copied().collect::<Vec<_>>(),vec![burn_core::sink()]);
    assert!(ex.validity.is_valid_at(input.block,input.timestamp));
    let header=ProgramHeader::from_loader_actor_state(state.accounts.get(&burn_core::TOKEN).unwrap().data.actor_state(PROGRAM_LOADER_ACCOUNT_ID)).unwrap();
    assert!(header.immutable); assert_eq!(header.image_id,input.token_image);
    assert_eq!(ex.program_image_claims.len(),1);
    match ex.program_image_claims[0] {ProgramImageClaim::Disclosed{account_id,image_id}=>{assert_eq!(account_id,burn_core::TOKEN);assert_eq!(image_id,input.token_image);},_=>panic!("unsupported claim")}
    let signing_hash=hash(&[b"/LEE/v0.3/Message/Privacy/\x00\x00\x00\x00\x00\x00".as_slice(),&borsh::to_vec(m).unwrap()].concat());
    let mut signers=BTreeSet::new();
    for (sig,key) in &tx.witness.signatures {
        let id=AccountId::new(hash(&[b"/LEE/v0.3/AccountId/Public/\x00\x00\x00\x00\x00".as_slice(),key].concat()));
        assert!(signers.insert(id));
        k256::schnorr::VerifyingKey::from_bytes(key).unwrap().verify_prehash(&signing_hash,&k256::schnorr::Signature::try_from(sig.as_slice()).unwrap()).unwrap();
        assert_eq!(m.nonces.get(&id).unwrap(),&state.accounts.get(&id).unwrap().nonce);
    }
    assert!(m.nonces.keys().copied().eq(signers.iter().copied()));
    assert_eq!(m.public_context.authorized_accounts,signers);
    if let Some(fee)=&m.fee {assert!(signers.contains(&fee.payer));}
    let commits:Vec<[u8;32]>=ex.private_actions.iter().map(|a|a.commitment.to_byte_array()).collect();
    let nullifiers:Vec<([u8;32],[u8;32])>=ex.private_actions.iter().map(|a|(a.nullifier.to_byte_array(),a.root)).collect();
    assert_eq!(commits.iter().collect::<BTreeSet<_>>().len(),commits.len());
    assert_eq!(nullifiers.iter().map(|(n,_)|n).collect::<BTreeSet<_>>().len(),nullifiers.len());
    for c in &commits {assert!(!state.private.0.positions.contains_key(c),"commitment already seen");}
    for (n,r) in &nullifiers {assert!(!state.private.1.contains(n),"nullifier already seen");assert!(state.private.0.roots.contains(r),"unrecognized root");}
    let outcome=PublicPart::new(m.public_context.clone(),ex.public_root.clone(),ex.boundary.clone()).unwrap().execute(&mut Backend(&state)).unwrap();
    assert!(outcome.async_messages.is_empty());
    for (id,data) in outcome.accounts {state.accounts.entry(id).or_default().data.update(&data);}
    for id in signers {state.accounts.get_mut(&id).unwrap().nonce.public_account_nonce_increment();}
    for c in commits {let i=state.private.0.tree.insert(c);state.private.0.positions.insert(c,i);}
    state.private.0.roots.insert(state.private.0.tree.root());
    for (n,_) in nullifiers {state.private.1.insert(n);}
    let exits=burn_core::read_state(state.accounts.get(&burn_core::sink().account_id).unwrap().data.actor_state(burn_core::TOKEN)).exits;
    let journal=Journal{before:state_digest(&input.before),after:state_digest(&borsh::to_vec(&state).unwrap()),exit_root:burn_core::exit_root(&exits),tx_hash:hash(&borsh::to_vec(tx).unwrap()),block:input.block,timestamp:input.timestamp};
    (state,journal)
}
