// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use async_trait::async_trait;
use cid::{multihash::MultihashDigest, Cid};
use db::{Error, MemoryDB, RocksDb, Store};
use encoding::{de::DeserializeOwned, from_slice};

/// Wrapper for database to handle inserting and retrieving data from AMT with Cids
#[async_trait]
pub trait BlockStore: Store {
    /// Get bytes from block store by Cid
    async fn get_bytes(&self, cid: &Cid) -> Result<Option<Vec<u8>>, Error> {
        Ok(self.read(cid.to_bytes()).await?)
    }

    /// Get typed object from block store by Cid
    async fn get<T>(&self, cid: &Cid) -> Result<Option<T>, Error>
    where
        T: DeserializeOwned,
    {
        match self.get_bytes(cid).await? {
            Some(bz) => Ok(Some(
                from_slice(&bz).map_err(|e| Error::new(e.to_string()))?,
            )),
            None => Ok(None),
        }
    }

    /// Put an object in the block store and return the Cid identifier
    async fn put<T>(&self, obj: &[u8], hash: T) -> Result<Cid, Error>
    where
        T: MultihashDigest + Send,
    {
        let cid = Cid::new_from_cbor(&obj, hash).map_err(|e| Error::new(e.to_string()))?;
        self.write::<&[u8], &[u8]>(cid.to_bytes().as_ref(), obj.as_ref())
            .await?;
        Ok(cid)
    }
}

impl BlockStore for MemoryDB {}
impl BlockStore for RocksDb {}
