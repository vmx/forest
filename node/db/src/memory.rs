// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use super::Error;
use super::{DatabaseService, Store};
use async_std::sync::RwLock;
use async_trait::async_trait;
use std::collections::{hash_map::DefaultHasher, HashMap};
use std::hash::{Hash, Hasher};

/// A thread-safe `HashMap` wrapper.
#[derive(Debug)]
pub struct MemoryDB {
    db: RwLock<HashMap<u64, Vec<u8>>>,
}

impl MemoryDB {
    fn db_index<K>(key: &K) -> u64
    where
        K: AsRef<[u8]>,
    {
        let mut hasher = DefaultHasher::new();
        key.as_ref().hash::<DefaultHasher>(&mut hasher);
        hasher.finish()
    }

    async fn clone(&self) -> Self {
        Self {
            db: RwLock::new(self.db.read().await.clone()),
        }
    }
}

impl Default for MemoryDB {
    fn default() -> Self {
        Self {
            db: RwLock::new(HashMap::new()),
        }
    }
}

impl DatabaseService for MemoryDB {}

#[async_trait]
impl Store for MemoryDB {
    async fn write<K, V>(&self, key: K, value: V) -> Result<(), Error>
    where
        K: AsRef<[u8]> + Send,
        V: AsRef<[u8]> + Send,
    {
        self.db
            .write()
            .await
            .insert(Self::db_index(&key), value.as_ref().to_vec());
        Ok(())
    }

    async fn delete<K>(&self, key: K) -> Result<(), Error>
    where
        K: AsRef<[u8]> + Send,
    {
        self.db.write().await.remove(&Self::db_index(&key));
        Ok(())
    }

    async fn bulk_write<K, V>(&self, keys: &[K], values: &[V]) -> Result<(), Error>
    where
        K: AsRef<[u8]> + Send + Sync,
        V: AsRef<[u8]> + Send + Sync,
    {
        for (k, v) in keys.iter().zip(values.iter()) {
            self.db
                .write()
                .await
                .insert(Self::db_index(&k), v.as_ref().to_vec());
        }
        Ok(())
    }

    async fn bulk_delete<K>(&self, keys: &[K]) -> Result<(), Error>
    where
        K: AsRef<[u8]> + Send + Sync,
    {
        for k in keys.iter() {
            self.db.write().await.remove(&Self::db_index(&k));
        }
        Ok(())
    }

    async fn read<K>(&self, key: K) -> Result<Option<Vec<u8>>, Error>
    where
        K: AsRef<[u8]> + Send,
    {
        Ok(self.db.read().await.get(&Self::db_index(&key)).cloned())
    }

    async fn exists<K>(&self, key: K) -> Result<bool, Error>
    where
        K: AsRef<[u8]> + Send,
    {
        Ok(self.db.read().await.contains_key(&Self::db_index(&key)))
    }

    async fn bulk_read<K>(&self, keys: &[K]) -> Result<Vec<Option<Vec<u8>>>, Error>
    where
        K: AsRef<[u8]> + Send + Sync,
    {
        let mut v = Vec::with_capacity(keys.len());
        for k in keys.iter() {
            v.push(self.db.read().await.get(&Self::db_index(&k)).cloned())
        }
        Ok(v)
    }
}
