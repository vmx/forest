// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

mod state;
mod types;

pub use self::state::State;
pub use self::types::*;
use crate::{
    make_map, make_map_with_root, ActorDowncast, STORAGE_MARKET_ACTOR_ADDR, SYSTEM_ACTOR_ADDR,
};
use address::Address;
use ipld_blockstore::BlockStore;
use num_bigint::bigint_ser::BigIntDe;
use num_bigint::Sign;
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use runtime::{ActorCode, Runtime};
use vm::{actor_error, ActorError, ExitCode, MethodNum, Serialized, METHOD_CONSTRUCTOR};

// * Updated to specs-actors commit: 17d3c602059e5c48407fb3c34343da87e6ea6586 (v0.9.12)

/// Account actor methods available
#[derive(FromPrimitive)]
#[repr(u64)]
pub enum Method {
    Constructor = METHOD_CONSTRUCTOR,
    AddVerifier = 2,
    RemoveVerifier = 3,
    AddVerifiedClient = 4,
    UseBytes = 5,
    RestoreBytes = 6,
}

pub struct Actor;
impl Actor {
    /// Constructor for Registry Actor
    pub fn constructor<BS, RT>(rt: &mut RT, root_key: Address) -> Result<(), ActorError>
    where
        BS: BlockStore,
        RT: Runtime<BS>,
    {
        rt.validate_immediate_caller_is(std::iter::once(&*SYSTEM_ACTOR_ADDR))?;

        // root should be an ID address
        let id_addr = rt
            .resolve_address(&root_key)?
            .ok_or_else(|| actor_error!(ErrIllegalArgument; "root should be an ID address"))?;

        let empty_root = make_map::<_, ()>(rt.store()).flush().map_err(|e| {
            e.downcast_default(ExitCode::ErrIllegalState, "Failed to create registry state")
        })?;

        let st = State::new(empty_root, id_addr);
        rt.create(&st)?;
        Ok(())
    }

    pub fn add_verifier<BS, RT>(rt: &mut RT, params: AddVerifierParams) -> Result<(), ActorError>
    where
        BS: BlockStore,
        RT: Runtime<BS>,
    {
        if &params.allowance < &MINIMUM_VERIFIED_DEAL_SIZE {
            return Err(actor_error!(ErrIllegalArgument;
                    "Allowance {} below minimum deal size for add verifier {}",
                    params.allowance, params.address));
        }
        let st: State = rt.state()?;
        rt.validate_immediate_caller_is(std::iter::once(&st.root_key))?;

        // TODO track issue https://github.com/filecoin-project/specs-actors/issues/556
        if params.address == st.root_key {
            return Err(actor_error!(ErrIllegalArgument; "Rootkey cannot be added as verifier"));
        }

        rt.transaction(|st: &mut State, rt| {
            let mut verifiers = make_map_with_root(&st.verifiers, rt.store()).map_err(|e| {
                e.downcast_default(ExitCode::ErrIllegalState, "failed to load verified clients")
            })?;
            let verified_clients = make_map_with_root::<_, BigIntDe>(
                &st.verified_clients,
                rt.store(),
            )
            .map_err(|e| {
                e.downcast_default(ExitCode::ErrIllegalState, "failed to load verified clients")
            })?;

            let found = verified_clients
                .contains_key(&params.address.to_bytes())
                .map_err(|e| {
                    e.downcast_default(
                        ExitCode::ErrIllegalState,
                        format!("failed to get client state for {}", params.address,),
                    )
                })?;
            if found {
                return Err(actor_error!(ErrIllegalArgument;
                        "verified client {} cannot become a verifier", params.address));
            }

            verifiers
                .set(
                    params.address.to_bytes().into(),
                    BigIntDe(params.allowance.clone()),
                )
                .map_err(|e| {
                    e.downcast_default(ExitCode::ErrIllegalState, "failed to add verifier")
                })?;
            st.verifiers = verifiers.flush().map_err(|e| {
                e.downcast_default(ExitCode::ErrIllegalState, "failed to flush verifiers")
            })?;

            Ok(())
        })?;

        Ok(())
    }

    pub fn remove_verifier<BS, RT>(rt: &mut RT, verifier_addr: Address) -> Result<(), ActorError>
    where
        BS: BlockStore,
        RT: Runtime<BS>,
    {
        let state: State = rt.state()?;
        rt.validate_immediate_caller_is(std::iter::once(&state.root_key))?;

        rt.transaction(|st: &mut State, rt| {
            let mut verifiers = make_map_with_root::<_, BigIntDe>(&st.verifiers, rt.store())
                .map_err(|e| {
                    e.downcast_default(ExitCode::ErrIllegalState, "failed to load verified clients")
                })?;
            verifiers
                .delete(&verifier_addr.to_bytes())
                .map_err(|e| {
                    e.downcast_default(ExitCode::ErrIllegalState, "failed to remove verifier")
                })?
                .ok_or_else(
                    || actor_error!(ErrIllegalState; "failed to remove verifier: not found"),
                )?;

            st.verifiers = verifiers.flush().map_err(|e| {
                e.downcast_default(ExitCode::ErrIllegalState, "failed to flush verifiers")
            })?;
            Ok(())
        })?;

        Ok(())
    }

    pub fn add_verified_client<BS, RT>(
        rt: &mut RT,
        params: AddVerifierClientParams,
    ) -> Result<(), ActorError>
    where
        BS: BlockStore,
        RT: Runtime<BS>,
    {
        if params.allowance < *MINIMUM_VERIFIED_DEAL_SIZE {
            return Err(actor_error!(ErrIllegalArgument;
                    "Allowance {} below MinVerifiedDealSize for add verified client {}",
                    params.allowance, params.address
            ));
        }

        rt.validate_immediate_caller_accept_any()?;

        let st: State = rt.state()?;
        // TODO track issue https://github.com/filecoin-project/specs-actors/issues/556
        if params.address == st.root_key {
            return Err(actor_error!(ErrIllegalArgument; "Rootkey cannot be added as verifier"));
        }

        rt.transaction(|st: &mut State, rt| {
            let mut verifiers = make_map_with_root(&st.verifiers, rt.store()).map_err(|e| {
                e.downcast_default(ExitCode::ErrIllegalState, "failed to load verified clients")
            })?;
            let mut verified_clients = make_map_with_root(&st.verified_clients, rt.store())
                .map_err(|e| {
                    e.downcast_default(ExitCode::ErrIllegalState, "failed to load verified clients")
                })?;

            // Validate caller is one of the verifiers.
            let verifier_addr = rt.message().caller();
            let BigIntDe(verifier_cap) = verifiers
                .get(&verifier_addr.to_bytes())
                .map_err(|e| {
                    e.downcast_default(
                        ExitCode::ErrIllegalState,
                        format!("failed to get Verifier {}", verifier_addr),
                    )
                })?
                .ok_or_else(|| {
                    actor_error!(ErrNotFound;
                        format!("no such Verifier {}", verifier_addr)
                    )
                })?;

            // Validate client to be added isn't a verifier
            let found = verifiers
                .contains_key(&params.address.to_bytes())
                .map_err(|e| {
                    e.downcast_default(ExitCode::ErrIllegalState, "failed to get verifier")
                })?;
            if found {
                return Err(actor_error!(ErrIllegalArgument;
                    "verifier {} cannot be added as a verified client", params.address));
            }

            // Compute new verifier cap and update.
            if verifier_cap < &params.allowance {
                return Err(actor_error!(ErrIllegalArgument;
                        "Add more DataCap {} for VerifiedClient than allocated {}",
                        params.allowance, verifier_cap
                ));
            }
            let new_verifier_cap = verifier_cap - &params.allowance;

            verifiers
                .set(verifier_addr.to_bytes().into(), BigIntDe(new_verifier_cap))
                .map_err(|e| {
                    e.downcast_default(
                        ExitCode::ErrIllegalState,
                        format!("Failed to update new verifier cap for {}", params.allowance),
                    )
                })?;

            // This is a one-time, upfront allocation.
            // This allowance cannot be changed by calls to AddVerifiedClient as long as the
            // client has not been removed. If parties need more allowance, they need to create a
            // new verified client or use up the the current allowance and then create a new
            // verified client.
            let found = verified_clients
                .contains_key(&params.address.to_bytes())
                .map_err(|e| {
                    e.downcast_default(
                        ExitCode::ErrIllegalState,
                        format!("Failed to get verified client {}", params.address,),
                    )
                })?;
            if found {
                return Err(actor_error!(ErrIllegalArgument;
                    "verified client already exists: {}", params.address));
            }

            verified_clients
                .set(
                    params.address.to_bytes().into(),
                    BigIntDe(params.allowance.clone()),
                )
                .map_err(|e| {
                    e.downcast_default(
                        ExitCode::ErrIllegalState,
                        format!(
                            "Failed to add verified client {} with cap {}",
                            params.address, params.allowance,
                        ),
                    )
                })?;

            st.verifiers = verifiers.flush().map_err(|e| {
                e.downcast_default(ExitCode::ErrIllegalState, "failed to flush verifiers")
            })?;
            st.verified_clients = verified_clients.flush().map_err(|e| {
                e.downcast_default(
                    ExitCode::ErrIllegalState,
                    "failed to flush verified clients",
                )
            })?;

            Ok(())
        })?;

        Ok(())
    }

    /// Called by StorageMarketActor during PublishStorageDeals.
    /// Do not allow partially verified deals (DealSize must be greater than equal to allowed cap).
    /// Delete VerifiedClient if remaining DataCap is smaller than minimum VerifiedDealSize.
    pub fn use_bytes<BS, RT>(rt: &mut RT, params: UseBytesParams) -> Result<(), ActorError>
    where
        BS: BlockStore,
        RT: Runtime<BS>,
    {
        rt.validate_immediate_caller_is(std::iter::once(&*STORAGE_MARKET_ACTOR_ADDR))?;
        if params.deal_size < *MINIMUM_VERIFIED_DEAL_SIZE {
            return Err(actor_error!(ErrIllegalArgument;
                "Verified Dealsize {} is below minimum in usedbytes", params.deal_size
            ));
        }

        rt.transaction(|st: &mut State, rt| {
            let mut verified_clients = make_map_with_root(&st.verified_clients, rt.store())
                .map_err(|e| {
                    e.downcast_default(ExitCode::ErrIllegalState, "failed to load verified clients")
                })?;

            let BigIntDe(vc_cap) = verified_clients
                .get(&params.address.to_bytes())
                .map_err(|e| {
                    e.downcast_default(
                        ExitCode::ErrIllegalState,
                        format!("failed to get verified client {}", &params.address),
                    )
                })?
                .ok_or_else(
                    || actor_error!(ErrNotFound; "no such verified client {}", params.address),
                )?;
            assert_ne!(vc_cap.sign(), Sign::Minus);

            if &params.deal_size > vc_cap {
                return Err(actor_error!(ErrIllegalArgument;
                        "Deal size of {} is greater than verifier_cap {} for verified client {}",
                        params.deal_size, vc_cap, params.address
                ));
            };

            let new_vc_cap = vc_cap - &params.deal_size;
            if new_vc_cap < *MINIMUM_VERIFIED_DEAL_SIZE {
                // Delete entry if remaining DataCap is less than MinVerifiedDealSize.
                // Will be restored later if the deal did not get activated with a ProvenSector.
                verified_clients
                    .delete(&params.address.to_bytes())
                    .map_err(|e| {
                        e.downcast_default(
                            ExitCode::ErrIllegalState,
                            format!("Failed to delete verified client {}", params.address),
                        )
                    })?
                    .ok_or_else(|| {
                        actor_error!(ErrIllegalState;
                            "Failed to delete verified client {}: not found",
                            params.address
                        )
                    })?;
            } else {
                verified_clients
                    .set(params.address.to_bytes().into(), BigIntDe(new_vc_cap))
                    .map_err(|e| {
                        e.downcast_default(
                            ExitCode::ErrIllegalState,
                            format!("Failed to update verified client {}", params.address),
                        )
                    })?;
            }

            st.verified_clients = verified_clients.flush().map_err(|e| {
                e.downcast_default(
                    ExitCode::ErrIllegalState,
                    "failed to flush verified clients",
                )
            })?;
            Ok(())
        })?;

        Ok(())
    }

    /// Called by HandleInitTimeoutDeals from StorageMarketActor when a VerifiedDeal fails to init.
    /// Restore allowable cap for the client, creating new entry if the client has been deleted.
    pub fn restore_bytes<BS, RT>(rt: &mut RT, params: RestoreBytesParams) -> Result<(), ActorError>
    where
        BS: BlockStore,
        RT: Runtime<BS>,
    {
        rt.validate_immediate_caller_is(std::iter::once(&*STORAGE_MARKET_ACTOR_ADDR))?;
        if params.deal_size < *MINIMUM_VERIFIED_DEAL_SIZE {
            return Err(actor_error!(ErrIllegalArgument;
                "Below minimum VerifiedDealSize requested in RestoreBytes: {}",
                params.deal_size
            ));
        }

        let st: State = rt.state()?;
        // TODO track issue https://github.com/filecoin-project/specs-actors/issues/556
        if params.address == st.root_key {
            return Err(actor_error!(ErrIllegalArgument; "Cannot restore allowance for Rootkey"));
        }

        rt.transaction(|st: &mut State, rt| {
            let verifiers =
                make_map_with_root::<_, BigIntDe>(&st.verifiers, rt.store()).map_err(|e| {
                    e.downcast_default(ExitCode::ErrIllegalState, "failed to load verified clients")
                })?;
            let mut verified_clients = make_map_with_root(&st.verified_clients, rt.store())
                .map_err(|e| {
                    e.downcast_default(ExitCode::ErrIllegalState, "failed to load verified clients")
                })?;

            // validate we are NOT attempting to do this for a verifier
            let found = verifiers
                .contains_key(&params.address.to_bytes())
                .map_err(|e| {
                    e.downcast_default(ExitCode::ErrIllegalState, "failed to get verifier")
                })?;
            if found {
                return Err(actor_error!(ErrIllegalArgument;
                    "cannot restore allowance for a verifier {}", params.address));
            }

            // Get existing cap
            let BigIntDe(vc_cap) = verified_clients
                .get(&params.address.to_bytes())
                .map_err(|e| {
                    e.downcast_default(
                        ExitCode::ErrIllegalState,
                        format!("failed to get verified client {}", &params.address),
                    )
                })?
                .cloned()
                .unwrap_or_default();

            // Update to new cap
            let new_vc_cap = vc_cap + &params.deal_size;
            verified_clients
                .set(params.address.to_bytes().into(), BigIntDe(new_vc_cap))
                .map_err(|e| {
                    e.downcast_default(
                        ExitCode::ErrIllegalState,
                        format!("Failed to put verified client {}", params.address),
                    )
                })?;

            st.verified_clients = verified_clients.flush().map_err(|e| {
                e.downcast_default(
                    ExitCode::ErrIllegalState,
                    "failed to flush verified clients",
                )
            })?;
            Ok(())
        })?;

        Ok(())
    }
}

impl ActorCode for Actor {
    fn invoke_method<BS, RT>(
        rt: &mut RT,
        method: MethodNum,
        params: &Serialized,
    ) -> Result<Serialized, ActorError>
    where
        BS: BlockStore,
        RT: Runtime<BS>,
    {
        match FromPrimitive::from_u64(method) {
            Some(Method::Constructor) => {
                Self::constructor(rt, params.deserialize()?)?;
                Ok(Serialized::default())
            }
            Some(Method::AddVerifier) => {
                Self::add_verifier(rt, params.deserialize()?)?;
                Ok(Serialized::default())
            }
            Some(Method::RemoveVerifier) => {
                Self::remove_verifier(rt, params.deserialize()?)?;
                Ok(Serialized::default())
            }
            Some(Method::AddVerifiedClient) => {
                Self::add_verified_client(rt, params.deserialize()?)?;
                Ok(Serialized::default())
            }
            Some(Method::UseBytes) => {
                Self::use_bytes(rt, params.deserialize()?)?;
                Ok(Serialized::default())
            }
            Some(Method::RestoreBytes) => {
                Self::restore_bytes(rt, params.deserialize()?)?;
                Ok(Serialized::default())
            }
            None => Err(actor_error!(SysErrInvalidMethod; "Invalid method")),
        }
    }
}
