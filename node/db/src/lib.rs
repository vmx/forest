// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

mod errors;
mod memory;
mod rocks;

pub use errors::Error;
pub use memory::*;
pub use rocks::*;

use async_trait::async_trait;

pub trait DatabaseService {
    fn open(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

#[async_trait]
pub trait Store {
    /// Read single value from data store and return `None` if key doesn't exist.
    async fn read<K>(&self, key: K) -> Result<Option<Vec<u8>>, Error>
    where
        K: AsRef<[u8]> + Send;

    /// Write a single value to the data store.
    async fn write<K, V>(&self, key: K, value: V) -> Result<(), Error>
    where
        K: AsRef<[u8]> + Send,
        V: AsRef<[u8]> + Send;

    /// Delete value at key.
    async fn delete<K>(&self, key: K) -> Result<(), Error>
    where
        K: AsRef<[u8]> + Send;

    /// Returns `Ok(true)` if key exists in store
    async fn exists<K>(&self, key: K) -> Result<bool, Error>
    where
        K: AsRef<[u8]> + Send;

    /// Read slice of keys and return a vector of optional values.
    async fn bulk_read<K>(&self, keys: &[K]) -> Result<Vec<Option<Vec<u8>>>, Error>
    where
        K: AsRef<[u8]> + Send + Sync;

    /// Write slice of KV pairs.
    // TODO maybe change API to slice of tuple KV
    async fn bulk_write<K, V>(&self, keys: &[K], values: &[V]) -> Result<(), Error>
    where
        K: AsRef<[u8]> + Send + Sync,
        V: AsRef<[u8]> + Send + Sync;

    /// Bulk delete keys from the data store.
    async fn bulk_delete<K>(&self, keys: &[K]) -> Result<(), Error>
    where
        K: AsRef<[u8]> + Send + Sync;
}
