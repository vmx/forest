// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

#[cfg(feature = "buffered")]
mod buffered;
#[cfg(feature = "resolve")]
pub mod resolve;
#[cfg(feature = "tracking")]
mod tracking;

#[cfg(feature = "buffered")]
pub use self::buffered::BufferedBlockStore;

#[cfg(feature = "tracking")]
pub use self::tracking::{BSStats, TrackingBlockStore};

use cid::{
    multihash::{MultihashDigest, U32},
    Cid,
};
use db::{MemoryDB, Store};
use encoding::{de::DeserializeOwned, from_slice, ser::Serialize, to_vec};
use std::error::Error as StdError;

#[cfg(feature = "rocksdb")]
use db::{RocksDb, WriteBatch};

/// Wrapper for database to handle inserting and retrieving ipld data with Cids
pub trait BlockStore: Store {
    /// Get bytes from block store by Cid.
    fn get_bytes(&self, cid: &Cid) -> Result<Option<Vec<u8>>, Box<dyn StdError>> {
        Ok(self.read(cid.to_bytes())?)
    }

    /// Get typed object from block store by Cid.
    fn get<T>(&self, cid: &Cid) -> Result<Option<T>, Box<dyn StdError>>
    where
        T: DeserializeOwned,
    {
        match self.get_bytes(cid)? {
            Some(bz) => Ok(Some(from_slice(&bz)?)),
            None => Ok(None),
        }
    }

    /// Put an object in the block store and return the Cid identifier.
    fn put<S, T>(&self, obj: &S, hash: T) -> Result<Cid, Box<dyn StdError>>
    where
        S: Serialize,
        T: MultihashDigest<AllocSize = U32>,
    {
        let bytes = to_vec(obj)?;
        self.put_raw(bytes, hash)
    }

    /// Put raw bytes in the block store and return the Cid identifier.
    fn put_raw<T>(&self, bytes: Vec<u8>, hash: T) -> Result<Cid, Box<dyn StdError>>
    where
        T: MultihashDigest<AllocSize = U32>,
    {
        let cid = Cid::new_from_cbor(&bytes, hash);
        self.write(cid.to_bytes(), bytes)?;
        Ok(cid)
    }

    /// Batch put cbor objects into blockstore and returns vector of Cids
    fn bulk_put<'a, S, T, V>(&self, values: V, hash: T) -> Result<Vec<Cid>, Box<dyn StdError>>
    where
        S: Serialize + 'a,
        T: MultihashDigest<AllocSize = U32>,
        V: IntoIterator<Item = &'a S>,
    {
        values
            .into_iter()
            .map(|value| self.put(value, hash))
            .collect()
    }
}

impl BlockStore for MemoryDB {}

#[cfg(feature = "rocksdb")]
impl BlockStore for RocksDb {
    fn bulk_put<'a, S, T, V>(&self, values: V, hash: T) -> Result<Vec<Cid>, Box<dyn StdError>>
    where
        S: Serialize + 'a,
        T: MultihashDigest<AllocSize = U32>,
        V: IntoIterator<Item = &'a S>,
    {
        let mut batch = WriteBatch::default();
        let cids: Vec<Cid> = values
            .into_iter()
            .map(|v| {
                let bz = to_vec(v)?;
                let cid = Cid::new_from_cbor(&bz, hash);
                batch.put(cid.to_bytes(), bz);
                Ok(cid)
            })
            .collect::<Result<_, Box<dyn StdError>>>()?;
        self.db()?.write(batch)?;

        Ok(cids)
    }
}
