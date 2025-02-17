// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

#[macro_use]
extern crate lazy_static;

mod chain_rand;
mod errors;
pub mod utils;
mod vm_circ_supply;

pub use self::errors::*;
use actor::*;
use address::{Address, BLSPublicKey, Payload, Protocol, BLS_PUB_LEN};
use async_log::span;
use async_std::{sync::RwLock, task};
use blockstore::BlockStore;
use blockstore::BufferedBlockStore;
use chain::{ChainStore, HeadChange};
use chain_rand::ChainRand;
use cid::Cid;
use clock::ChainEpoch;
use encoding::de::DeserializeOwned;
use encoding::Cbor;
use fil_types::{get_network_version_default, verifier::ProofVerifier};
use flo_stream::Subscriber;
use forest_blocks::{BlockHeader, Tipset, TipsetKeys};
use futures::channel::oneshot;
use futures::stream::{FuturesUnordered, StreamExt};
use interpreter::{resolve_to_key_addr, ApplyRet, BlockMessages, Rand, VM};
use ipld_amt::Amt;
use lazycell::AtomicLazyCell;
use log::{debug, info, trace, warn};
use message::{message_receipt, unsigned_message};
use message::{ChainMessage, Message, MessageReceipt, UnsignedMessage};
use num_bigint::{bigint_ser, BigInt};
use serde::{Deserialize, Serialize};
use state_tree::StateTree;
use std::collections::HashMap;
use std::error::Error as StdError;
use std::sync::Arc;
use vm_circ_supply::GenesisInfoPair;

/// Intermediary for retrieving state objects and updating actor states
pub type CidPair = (Cid, Cid);

/// Type to represent invocation of state call results
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct InvocResult {
    #[serde(with = "unsigned_message::json")]
    pub msg: UnsignedMessage,
    #[serde(with = "message_receipt::json::opt")]
    pub msg_rct: Option<MessageReceipt>,
    pub error: Option<String>,
}

// An alias Result that represents an InvocResult and an Error
pub type StateCallResult = Result<InvocResult, Error>;

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MarketBalance {
    #[serde(with = "bigint_ser")]
    escrow: BigInt,
    #[serde(with = "bigint_ser")]
    locked: BigInt,
}

pub struct StateManager<DB> {
    cs: Arc<ChainStore<DB>>,

    /// This is a cache which indexes tipsets to their calculated state.
    /// The calculated state is wrapped in a mutex to avoid duplicate computation
    /// of the state/receipt root.
    cache: RwLock<HashMap<TipsetKeys, Arc<RwLock<Option<CidPair>>>>>,
    subscriber: Option<Subscriber<HeadChange>>,
    genesis_info: GenesisInfoPair,
}

impl<DB> StateManager<DB>
where
    DB: BlockStore + Send + Sync + 'static,
{
    pub fn new(cs: Arc<ChainStore<DB>>) -> Self {
        Self {
            cs,
            cache: RwLock::new(HashMap::new()),
            subscriber: None,
            genesis_info: GenesisInfoPair::default(),
        }
    }

    // Creates a constructor that passes in a HeadChange subscriber and a back_search subscriber
    pub fn new_with_subscribers(
        cs: Arc<ChainStore<DB>>,
        chain_subs: Subscriber<HeadChange>,
    ) -> Self {
        Self {
            cs,
            cache: RwLock::new(HashMap::new()),
            subscriber: Some(chain_subs),
            genesis_info: GenesisInfoPair::default(),
        }
    }
    /// Loads actor state from IPLD Store
    pub fn load_actor_state<D>(&self, addr: &Address, state_cid: &Cid) -> Result<D, Error>
    where
        D: DeserializeOwned,
    {
        let actor = self
            .get_actor(addr, state_cid)?
            .ok_or_else(|| Error::ActorNotFound(addr.to_string()))?;
        let act: D = self
            .blockstore()
            .get(&actor.state)
            .map_err(|e| Error::State(e.to_string()))?
            .ok_or_else(|| Error::ActorStateNotFound(actor.state.to_string()))?;
        Ok(act)
    }
    pub fn get_actor(&self, addr: &Address, state_cid: &Cid) -> Result<Option<ActorState>, Error> {
        let state = StateTree::new_from_root(self.blockstore(), state_cid)
            .map_err(|e| Error::State(e.to_string()))?;
        state
            .get_actor(addr)
            .map_err(|e| Error::State(e.to_string()))
    }

    pub fn blockstore_cloned(&self) -> Arc<DB> {
        self.cs.blockstore_cloned()
    }

    pub fn blockstore(&self) -> &DB {
        self.cs.blockstore()
    }

    pub fn chain_store(&self) -> &Arc<ChainStore<DB>> {
        &self.cs
    }

    /// Returns the network name from the init actor state
    pub fn get_network_name(&self, st: &Cid) -> Result<String, Error> {
        let state: init::State = self.load_actor_state(&*INIT_ACTOR_ADDR, st)?;
        Ok(state.network_name)
    }
    /// Returns true if miner has been slashed or is considered invalid
    pub fn is_miner_slashed(&self, addr: &Address, state_cid: &Cid) -> Result<bool, Error> {
        let spas: power::State = self.load_actor_state(&*STORAGE_POWER_ACTOR_ADDR, state_cid)?;

        let claims = make_map_with_root::<_, power::Claim>(&spas.claims, self.blockstore())
            .map_err(|e| Error::State(e.to_string()))?;

        Ok(!claims
            .contains_key(&addr.to_bytes())
            .map_err(|e| Error::State(e.to_string()))?)
    }
    /// Returns raw work address of a miner
    pub fn get_miner_work_addr(&self, state_cid: &Cid, addr: &Address) -> Result<Address, Error> {
        let ms: miner::State = self.load_actor_state(addr, state_cid)?;

        let state = StateTree::new_from_root(self.blockstore(), state_cid)
            .map_err(|e| Error::State(e.to_string()))?;

        let info = ms.get_info(self.blockstore()).map_err(|e| e.to_string())?;

        let addr = resolve_to_key_addr(&state, self.blockstore(), &info.worker)
            .map_err(|e| Error::Other(format!("Failed to resolve key address; error: {}", e)))?;
        Ok(addr)
    }
    /// Returns specified actor's claimed power and total network power as a tuple
    pub fn get_power(
        &self,
        state_cid: &Cid,
        addr: &Address,
    ) -> Result<(power::Claim, power::Claim), Error> {
        let ps: power::State = self.load_actor_state(&*STORAGE_POWER_ACTOR_ADDR, state_cid)?;

        let cm = make_map_with_root::<_, power::Claim>(&ps.claims, self.blockstore())
            .map_err(|e| Error::State(e.to_string()))?;
        let claim = cm
            .get(&addr.to_bytes())
            .map_err(|e| Error::State(e.to_string()))?
            .ok_or_else(|| {
                Error::State("Failed to retrieve claimed power from actor state".to_owned())
            })?
            .clone();
        Ok((
            claim,
            power::Claim {
                raw_byte_power: ps.total_raw_byte_power,
                quality_adj_power: ps.total_quality_adj_power,
            },
        ))
    }

    pub fn get_subscriber(&self) -> Option<Subscriber<HeadChange>> {
        self.subscriber.clone()
    }

    /// Performs the state transition for the tipset and applies all unique messages in all blocks.
    /// This function returns the state root and receipt root of the transition.
    #[allow(clippy::too_many_arguments)]
    pub fn apply_blocks<R, V, CB>(
        &self,
        parent_epoch: ChainEpoch,
        p_state: &Cid,
        messages: &[BlockMessages],
        epoch: ChainEpoch,
        rand: &R,
        base_fee: BigInt,
        callback: Option<CB>,
    ) -> Result<CidPair, Box<dyn StdError>>
    where
        R: Rand,
        V: ProofVerifier,
        CB: FnMut(&Cid, &ChainMessage, &ApplyRet) -> Result<(), String>,
    {
        let mut buf_store = BufferedBlockStore::new(self.blockstore());
        // TODO change from statically using devnet params when needed
        let mut vm = VM::<_, _, _, _, V>::new(
            p_state,
            &buf_store,
            epoch,
            rand,
            base_fee,
            get_network_version_default,
            &self.genesis_info,
        )?;

        // Apply tipset messages
        let receipts = vm.apply_block_messages(messages, parent_epoch, epoch, callback)?;

        // Construct receipt root from receipts
        let rect_root = Amt::new_from_slice(self.blockstore(), &receipts)?;

        // Flush changes to blockstore
        let state_root = vm.flush()?;
        // Persist changes connected to root
        buf_store.flush(&state_root)?;

        Ok((state_root, rect_root))
    }

    /// Returns the pair of (parent state root, message receipt root)
    pub async fn tipset_state<V>(
        self: &Arc<Self>,
        tipset: &Tipset,
    ) -> Result<CidPair, Box<dyn StdError>>
    where
        V: ProofVerifier,
    {
        span!("tipset_state", {
            // Get entry in cache, if it exists.
            // Arc is cloned here to avoid holding the entire cache lock until function ends.
            // (tasks should be able to compute different tipset state's in parallel)
            //
            // In the case of task `A` computing the same tipset as task `B`, `A` will hold the
            // mutex until the value is updated, which task `B` will await.
            //
            // If two tasks are computing different tipset states, they will only block computation
            // when accessing/initializing the entry in cache, not during the whole tipset calc.
            let cache_entry: Arc<_> = self
                .cache
                .write()
                .await
                .entry(tipset.key().clone())
                .or_default()
                // Clone Arc to drop lock of cache
                .clone();

            // Try to lock cache entry to ensure task is first to compute state.
            // If another task has the lock, it will overwrite the state before releasing lock.
            let mut entry_lock = cache_entry.write().await;
            if let Some(ref entry) = *entry_lock {
                // Entry had successfully populated state, return Cid and drop lock
                trace!("hit cache for tipset {:?}", tipset.cids());
                return Ok(*entry);
            }

            // Entry does not have state computed yet, this task will fill entry if successful.
            debug!("calculating tipset state {:?}", tipset.cids());

            let cid_pair = if tipset.epoch() == 0 {
                // NB: This is here because the process that executes blocks requires that the
                // block miner reference a valid miner in the state tree. Unless we create some
                // magical genesis miner, this won't work properly, so we short circuit here
                // This avoids the question of 'who gets paid the genesis block reward'
                let message_receipts = tipset
                    .blocks()
                    .first()
                    .ok_or_else(|| Error::Other("Could not get message receipts".to_string()))?;

                (*tipset.parent_state(), *message_receipts.message_receipts())
            } else {
                // generic constants are not implemented yet this is a lowcost method for now
                let no_func = None::<fn(&Cid, &ChainMessage, &ApplyRet) -> Result<(), String>>;
                self.compute_tipset_state::<V, _>(&tipset, no_func).await?
            };

            // Fill entry with calculated cid pair
            *entry_lock = Some(cid_pair);
            Ok(cid_pair)
        })
    }

    fn call_raw<V>(
        &self,
        msg: &mut UnsignedMessage,
        bstate: &Cid,
        rand: &ChainRand<DB>,
        bheight: &ChainEpoch,
    ) -> StateCallResult
    where
        V: ProofVerifier,
    {
        span!("state_call_raw", {
            let block_store = self.blockstore();
            let buf_store = BufferedBlockStore::new(block_store);
            let mut vm = VM::<_, _, _, _, V>::new(
                bstate,
                &buf_store,
                *bheight,
                rand,
                0.into(),
                get_network_version_default,
                &self.genesis_info,
            )?;

            if msg.gas_limit() == 0 {
                msg.set_gas_limit(10000000000)
            }

            let actor = self
                .get_actor(msg.from(), bstate)?
                .ok_or_else(|| Error::Other("Could not get actor".to_string()))?;
            msg.set_sequence(actor.sequence);
            let apply_ret = vm.apply_implicit_message(msg);
            trace!(
                "gas limit {:},gas premium{:?},value {:?}",
                msg.gas_limit(),
                msg.gas_premium(),
                msg.value()
            );
            if let Some(err) = &apply_ret.act_error {
                warn!("chain call failed: {:?}", err);
            }

            Ok(InvocResult {
                msg: msg.clone(),
                msg_rct: Some(apply_ret.msg_receipt.clone()),
                error: apply_ret.act_error.map(|e| e.to_string()),
            })
        })
    }

    /// runs the given message and returns its result without any persisted changes.
    pub async fn call<V>(
        &self,
        message: &mut UnsignedMessage,
        tipset: Option<Arc<Tipset>>,
    ) -> StateCallResult
    where
        V: ProofVerifier,
    {
        let ts = if let Some(t_set) = tipset {
            t_set
        } else {
            self.cs
                .heaviest_tipset()
                .await
                .ok_or_else(|| Error::Other("No heaviest tipset".to_string()))?
        };
        let state = ts.parent_state();
        let chain_rand = ChainRand::new(ts.key().to_owned(), self.cs.clone());
        self.call_raw::<V>(message, state, &chain_rand, &ts.epoch())
    }

    pub async fn call_with_gas<V>(
        self: &Arc<Self>,
        message: &mut ChainMessage,
        prior_messages: &[ChainMessage],
        tipset: Option<Arc<Tipset>>,
    ) -> StateCallResult
    where
        V: ProofVerifier,
    {
        let ts = if let Some(t_set) = tipset {
            t_set
        } else {
            self.cs
                .heaviest_tipset()
                .await
                .ok_or_else(|| Error::Other("No heaviest tipset".to_string()))?
        };
        let (st, _) = self
            .tipset_state::<V>(&ts)
            .await
            .map_err(|_| Error::Other("Could not load tipset state".to_string()))?;
        let chain_rand = ChainRand::new(ts.key().to_owned(), self.cs.clone());

        let mut vm = VM::<_, _, _, _, V>::new(
            &st,
            self.blockstore(),
            ts.epoch() + 1,
            &chain_rand,
            ts.blocks()[0].parent_base_fee().clone(),
            get_network_version_default,
            &self.genesis_info,
        )?;

        for msg in prior_messages {
            vm.apply_message(&msg)?;
        }
        let from_actor = vm
            .state()
            .get_actor(message.from())
            .map_err(|e| Error::Other(format!("Could not get actor from state: {}", e)))?
            .ok_or_else(|| Error::Other("cant find actor in state tree".to_string()))?;
        message.set_sequence(from_actor.sequence);

        let ret = vm.apply_message(&message)?;

        Ok(InvocResult {
            msg: message.message().clone(),
            msg_rct: Some(ret.msg_receipt.clone()),
            error: ret.act_error.map(|e| e.to_string()),
        })
    }

    /// returns the result of executing the indicated message, assuming it was executed in the indicated tipset.
    pub async fn replay<V>(
        self: &Arc<Self>,
        ts: &Tipset,
        mcid: Cid,
    ) -> Result<(UnsignedMessage, ApplyRet), Error>
    where
        V: ProofVerifier,
    {
        // This isn't ideal to have, since the execution is syncronous, but this needs to be the
        // case because the state transition has to be in blocking thread to avoid starving executor
        let outm: AtomicLazyCell<UnsignedMessage> = Default::default();
        let outr: AtomicLazyCell<ApplyRet> = Default::default();
        let m_clone = outm.clone();
        let r_clone = outr.clone();
        let callback = move |cid: &Cid, unsigned: &ChainMessage, apply_ret: &ApplyRet| {
            if *cid == mcid {
                let _ = m_clone.fill(unsigned.message().clone());
                let _ = r_clone.fill(apply_ret.clone());
                return Err("halt".to_string());
            }

            Ok(())
        };
        let result = self.compute_tipset_state::<V, _>(&ts, Some(callback)).await;

        if let Err(error_message) = result {
            if error_message.to_string() != "halt" {
                return Err(Error::Other(format!(
                    "unexpected error during execution : {:}",
                    error_message
                )));
            }
        }

        let out_mes = outm
            .into_inner()
            .ok_or_else(|| Error::Other("given message not found in tipset".to_string()))?;
        let out_ret = outr
            .into_inner()
            .ok_or_else(|| Error::Other("message did not have a return".to_string()))?;
        Ok((out_mes, out_ret))
    }

    pub async fn compute_tipset_state<V, CB: 'static>(
        self: &Arc<Self>,
        tipset: &Tipset,
        callback: Option<CB>,
    ) -> Result<CidPair, Error>
    where
        V: ProofVerifier,
        CB: FnMut(&Cid, &ChainMessage, &ApplyRet) -> Result<(), String> + Send,
    {
        span!("compute_tipset_state", {
            let block_headers = tipset.blocks();
            let first_block = block_headers
                .first()
                .ok_or_else(|| Error::Other("Empty tipset in compute_tipset_state".to_string()))?;

            let check_for_duplicates = |s: &BlockHeader| {
                block_headers
                    .iter()
                    .filter(|val| val.miner_address() == s.miner_address())
                    .take(2)
                    .count()
            };
            if let Some(a) = block_headers.iter().find(|s| check_for_duplicates(s) > 1) {
                // Duplicate Miner found
                return Err(Error::Other(format!("duplicate miner in a tipset ({})", a)));
            }

            let parent_epoch = if first_block.epoch() > 0 {
                let parent_cid = first_block
                    .parents()
                    .cids()
                    .get(0)
                    .ok_or_else(|| Error::Other("block must have parents".to_string()))?;
                let parent: BlockHeader = self
                    .blockstore()
                    .get(parent_cid)
                    .map_err(|e| Error::Other(e.to_string()))?
                    .ok_or_else(|| {
                        format!("Could not find parent block with cid {}", parent_cid)
                    })?;
                parent.epoch()
            } else {
                Default::default()
            };

            let tipset_keys =
                TipsetKeys::new(block_headers.iter().map(|s| s.cid()).cloned().collect());
            let chain_rand = ChainRand::new(tipset_keys, self.cs.clone());
            let base_fee = first_block.parent_base_fee().clone();

            let blocks = self
                .chain_store()
                .block_msgs_for_tipset(tipset)
                .map_err(|e| Error::Other(e.to_string()))?;

            let sm = self.clone();
            let sr = *first_block.state_root();
            let epoch = first_block.epoch();
            task::spawn_blocking(move || {
                sm.apply_blocks::<_, V, _>(
                    parent_epoch,
                    &sr,
                    &blocks,
                    epoch,
                    &chain_rand,
                    base_fee,
                    callback,
                )
                .map_err(|e| Error::Other(e.to_string()))
            })
            .await
        })
    }

    fn tipset_executed_message(
        &self,
        tipset: &Tipset,
        cid: &Cid,
        (message_from_address, message_sequence): (&Address, &u64),
    ) -> Result<Option<MessageReceipt>, Error> {
        if tipset.epoch() == 0 {
            return Ok(None);
        }
        let tipset = self
            .cs
            .tipset_from_keys(tipset.parents())
            .map_err(|err| Error::Other(err.to_string()))?;
        let messages = chain::messages_for_tipset(self.blockstore(), &tipset)
            .map_err(|err| Error::Other(err.to_string()))?;
        messages
            .iter()
            .enumerate()
            .rev()
            .filter(|(_, s)| s.from() == message_from_address)
            .filter_map(|(index,s)| {
                if s.sequence() == *message_sequence {
                    if s.cid().map(|s| &s == cid).unwrap_or_default() {
                        return Some(
                            chain::get_parent_reciept(
                                self.blockstore(),
                                tipset.blocks().first().unwrap(),
                                index as u64,
                            )
                            .map_err(|err| {
                                Error::Other(err.to_string())
                            }),
                        );
                    }
                    let error_msg = format!("found message with equal nonce as the one we are looking for (F:{:} n {:}, TS: `Error Converting message to Cid` n{:})", cid, message_sequence, s.sequence());
                    return Some(Err(Error::Other(error_msg)))
                }
                if s.sequence() < *message_sequence {
                    return Some(Ok(None));
                }

                None
            })
            .next()
            .unwrap_or_else(|| Ok(None))
    }
    fn search_back_for_message(
        &self,
        current: &Tipset,
        (message_from_address, message_cid, message_sequence): (&Address, &Cid, &u64),
    ) -> Result<Option<(Tipset, MessageReceipt)>, Error> {
        if current.epoch() == 0 {
            return Ok(None);
        }
        let state = StateTree::new_from_root(self.blockstore(), current.parent_state())
            .map_err(|e| Error::State(e.to_string()))?;

        if let Some(actor_state) = state
            .get_actor(message_from_address)
            .map_err(|e| Error::State(e.to_string()))?
        {
            if actor_state.sequence == 0 || actor_state.sequence < *message_sequence {
                return Ok(None);
            }
        }

        let tipset = self.cs.tipset_from_keys(current.parents()).map_err(|err| {
            Error::Other(format!(
                "failed to load tipset during msg wait searchback: {:}",
                err
            ))
        })?;
        let r = self.tipset_executed_message(
            &tipset,
            message_cid,
            (message_from_address, message_sequence),
        )?;

        if let Some(receipt) = r {
            return Ok(Some((tipset, receipt)));
        }
        self.search_back_for_message(
            &tipset,
            (message_from_address, message_cid, message_sequence),
        )
    }
    /// returns a message receipt from a given tipset and message cid
    pub fn get_receipt(&self, tipset: &Tipset, msg: &Cid) -> Result<MessageReceipt, Error> {
        let m = chain::get_chain_message(self.blockstore(), msg)
            .map_err(|e| Error::Other(e.to_string()))?;
        let message_var = (m.from(), &m.sequence());
        let message_receipt = self.tipset_executed_message(tipset, msg, message_var)?;

        if let Some(receipt) = message_receipt {
            return Ok(receipt);
        }
        let cid = m
            .cid()
            .map_err(|e| Error::Other(format!("Could not convert message to cid {:?}", e)))?;
        let message_var = (m.from(), &cid, &m.sequence());
        let maybe_tuple = self.search_back_for_message(tipset, message_var)?;
        let message_receipt = maybe_tuple
            .ok_or_else(|| {
                Error::Other("Could not get receipt from search back message".to_string())
            })?
            .1;
        Ok(message_receipt)
    }

    /// WaitForMessage blocks until a message appears on chain. It looks backwards in the chain to see if this has already
    /// happened. It guarantees that the message has been on chain for at least confidence epochs without being reverted
    /// before returning.
    pub async fn wait_for_message(
        self: &Arc<Self>,
        subscriber: Option<Subscriber<HeadChange>>,
        cid: &Cid,
        confidence: i64,
    ) -> Result<(Option<Arc<Tipset>>, Option<MessageReceipt>), Error>
    where
        DB: BlockStore + Send + Sync + 'static,
    {
        let mut subscribers = subscriber.clone().ok_or_else(|| {
            Error::Other("State Manager not subscribed to tipset head changes".to_string())
        })?;
        let (sender, mut receiver) = oneshot::channel::<()>();
        let message = chain::get_chain_message(self.blockstore(), cid)
            .map_err(|err| Error::Other(format!("failed to load message {:}", err)))?;

        let maybe_subscriber: Option<HeadChange> = subscribers.next().await;
        let first_subscriber = maybe_subscriber.ok_or_else(|| {
            Error::Other("SubHeadChanges first entry should have been one item".to_string())
        })?;

        let tipset = match first_subscriber {
            HeadChange::Current(tipset) => tipset,
            _ => {
                return Err(Error::Other(format!(
                    "expected current head on SHC stream (got {:?})",
                    first_subscriber
                )))
            }
        };
        let message_var = (message.from(), &message.sequence());
        let maybe_message_reciept = self.tipset_executed_message(&tipset, cid, message_var)?;
        if let Some(r) = maybe_message_reciept {
            return Ok((Some(tipset.clone()), Some(r)));
        }

        let mut candidate_tipset: Option<Arc<Tipset>> = None;
        let mut candidate_receipt: Option<MessageReceipt> = None;

        let sm_cloned = self.clone();
        let cid = message
            .cid()
            .map_err(|e| Error::Other(format!("Could not get cid from message {:?}", e)))?;

        let cid_for_task = cid;
        let address_for_task = *message.from();
        let sequence_for_task = message.sequence();
        let height_of_head = tipset.epoch();
        let task = task::spawn(async move {
            let (back_t, back_r) = sm_cloned
                .search_back_for_message(
                    &tipset,
                    (&address_for_task, &cid_for_task, &sequence_for_task),
                )?
                .ok_or_else(|| {
                    Error::Other("State manager not subscribed to back search wait".to_string())
                })?;
            let back_tuple = (back_t, back_r);
            sender
                .send(())
                .map_err(|e| Error::Other(format!("Could not send to channel {:?}", e)))?;
            Ok::<_, Error>(back_tuple)
        });

        let reverts: Arc<RwLock<HashMap<TipsetKeys, bool>>> = Arc::new(RwLock::new(HashMap::new()));
        let block_revert = reverts.clone();
        let sm_cloned = self.clone();
        let mut futures = FuturesUnordered::new();
        let subscriber_poll = task::spawn(async move {
            while let Some(subscriber) = subscriber
                .clone()
                .ok_or_else(|| {
                    Error::Other("State Manager not subscribed to tipset head changes".to_string())
                })?
                .next()
                .await
            {
                match subscriber {
                    HeadChange::Revert(_tipset) => {
                        if candidate_tipset.is_some() {
                            candidate_tipset = None;
                            candidate_receipt = None;
                        }
                    }
                    HeadChange::Apply(tipset) => {
                        if candidate_tipset
                            .as_ref()
                            .map(|s| s.epoch() >= s.epoch() + tipset.epoch())
                            .unwrap_or_default()
                        {
                            return Ok((candidate_tipset, candidate_receipt));
                        }
                        let poll_receiver = receiver.try_recv().map_err(|e| {
                            Error::Other(format!("Could not receieve from channel {:?}", e))
                        })?;
                        if poll_receiver.is_some() {
                            block_revert
                                .write()
                                .await
                                .insert(tipset.key().to_owned(), true);
                        }

                        let message_var = (message.from(), &message.sequence());
                        let maybe_receipt =
                            sm_cloned.tipset_executed_message(&tipset, &cid, message_var)?;
                        if let Some(receipt) = maybe_receipt {
                            if confidence == 0 {
                                return Ok((Some(tipset), Some(receipt)));
                            }
                            candidate_tipset = Some(tipset);
                            candidate_receipt = Some(receipt)
                        }
                    }
                    _ => (),
                }
            }

            Ok((None, None))
        });

        futures.push(subscriber_poll);
        let search_back_poll = task::spawn(async move {
            let (back_tipset, back_receipt) = task.await?;
            let should_revert = *reverts
                .read()
                .await
                .get(back_tipset.key())
                .unwrap_or(&false);
            let larger_height_of_head = height_of_head >= back_tipset.epoch() + confidence;
            if !should_revert && larger_height_of_head {
                return Ok((Some(Arc::new(back_tipset)), Some(back_receipt)));
            }

            Ok((None, None))
        });
        futures.push(search_back_poll);

        futures.next().await.ok_or_else(|| Error::Other("wait_for_message could not be completed due to failure of subscriber poll or search_back functionality".to_string()))?
    }

    /// Returns a bls public key from provided address
    pub fn get_bls_public_key(
        db: &DB,
        addr: &Address,
        state_cid: &Cid,
    ) -> Result<[u8; BLS_PUB_LEN], Error> {
        let state =
            StateTree::new_from_root(db, state_cid).map_err(|e| Error::State(e.to_string()))?;
        let kaddr = resolve_to_key_addr(&state, db, addr)
            .map_err(|e| format!("Failed to resolve key address, error: {}", e))?;

        match kaddr.into_payload() {
            Payload::BLS(BLSPublicKey(key)) => Ok(key),
            _ => Err(Error::State(
                "Address must be BLS address to load bls public key".to_owned(),
            )),
        }
    }

    /// Return the heaviest tipset's balance from self.db for a given address
    pub async fn get_heaviest_balance(&self, addr: &Address) -> Result<BigInt, Error> {
        let ts = self
            .cs
            .heaviest_tipset()
            .await
            .ok_or_else(|| Error::Other("could not get bs heaviest ts".to_owned()))?;
        let cid = ts.parent_state();
        self.get_balance(addr, cid)
    }

    /// Return the balance of a given address and state_cid
    pub fn get_balance(&self, addr: &Address, cid: &Cid) -> Result<BigInt, Error> {
        let act = self.get_actor(addr, cid)?;
        let actor = act.ok_or_else(|| "could not find actor".to_owned())?;
        Ok(actor.balance)
    }

    pub fn lookup_id(&self, addr: &Address, ts: &Tipset) -> Result<Option<Address>, Error> {
        let state_tree = StateTree::new_from_root(self.blockstore(), ts.parent_state())
            .map_err(|e| e.to_string())?;
        state_tree
            .lookup_id(addr)
            .map_err(|e| Error::State(e.to_string()))
    }

    pub fn market_balance(&self, addr: &Address, ts: &Tipset) -> Result<MarketBalance, Error> {
        let market_state: market::State =
            self.load_actor_state(&*STORAGE_MARKET_ACTOR_ADDR, ts.parent_state())?;

        let new_addr = self
            .lookup_id(addr, ts)?
            .ok_or_else(|| Error::State(format!("Failed to resolve address {}", addr)))?;

        let out = MarketBalance {
            escrow: {
                let et = BalanceTable::from_root(self.blockstore(), &market_state.escrow_table)
                    .map_err(|_x| Error::State("Failed to build Escrow Table".to_string()))?;
                et.get(&new_addr).unwrap_or_default()
            },
            locked: {
                let lt = BalanceTable::from_root(self.blockstore(), &market_state.locked_table)
                    .map_err(|_x| Error::State("Failed to build Locked Table".to_string()))?;
                lt.get(&new_addr).unwrap_or_default()
            },
        };

        Ok(out)
    }

    /// Similar to `resolve_to_key_addr` in the vm crate but does not allow `Actor` type of addresses.
    /// Uses `ts` to generate the VM state.
    pub async fn resolve_to_key_addr<V>(
        self: &Arc<Self>,
        addr: &Address,
        ts: &Tipset,
    ) -> Result<Address, Box<dyn StdError>>
    where
        V: ProofVerifier,
    {
        match addr.protocol() {
            Protocol::BLS | Protocol::Secp256k1 => return Ok(*addr),
            Protocol::Actor => {
                return Err(
                    Error::Other("cannot resolve actor address to key address".to_string()).into(),
                )
            }
            _ => {}
        };
        let (st, _) = self.tipset_state::<V>(&ts).await?;
        let state = StateTree::new_from_root(self.blockstore(), &st)
            .map_err(|e| Error::State(e.to_string()))?;

        Ok(interpreter::resolve_to_key_addr(
            &state,
            self.blockstore(),
            &addr,
        )?)
    }

    /// Checks power actor state for if miner meets consensus minimum requirements.
    pub fn miner_has_min_power(&self, addr: &Address, ts: &Tipset) -> Result<bool, String> {
        let ps: power::State = self
            .load_actor_state(&*STORAGE_POWER_ACTOR_ADDR, ts.parent_state())
            .map_err(|e| format!("loading power actor state: {}", e))?;
        ps.miner_nominal_power_meets_consensus_minimum(self.blockstore(), addr)
            .map_err(|e| e.to_string())
    }

    pub async fn validate_chain<V: ProofVerifier>(
        self: &Arc<Self>,
        mut ts: Tipset,
    ) -> Result<(), Box<dyn StdError>> {
        let mut ts_chain = Vec::<Tipset>::new();
        while ts.epoch() != 0 {
            let next = self.cs.tipset_from_keys(ts.parents())?;
            ts_chain.push(std::mem::replace(&mut ts, next));
        }
        ts_chain.push(ts);

        let mut last_state = *ts_chain.last().unwrap().parent_state();
        let mut last_receipt = *ts_chain.last().unwrap().blocks()[0].message_receipts();
        for ts in ts_chain.iter().rev() {
            if ts.parent_state() != &last_state {
                return Err(format!(
                    "Tipset chain has state mismatch at height: {}, {} != {}",
                    ts.epoch(),
                    ts.parent_state(),
                    last_state
                )
                .into());
            }
            if ts.blocks()[0].message_receipts() != &last_receipt {
                return Err(format!(
                    "Tipset message receipts has a mismatch at height: {}",
                    ts.epoch(),
                )
                .into());
            }
            info!(
                "Computing state (height: {}, ts={:?})",
                ts.epoch(),
                ts.cids()
            );
            let (st, msg_root) = self.tipset_state::<V>(&ts).await?;
            last_state = st;
            last_receipt = msg_root;
        }
        Ok(())
    }
}
