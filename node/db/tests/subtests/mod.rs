// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use db::{DatabaseService, Store};

pub fn open<DB>(db: &mut DB)
where
    DB: DatabaseService,
{
    db.open().unwrap();
}

pub async fn write<DB>(db: &DB)
where
    DB: Store,
{
    let key = [1];
    let value = [1];
    db.write(key, value).await.unwrap();
}

pub async fn read<DB>(db: &DB)
where
    DB: Store,
{
    let key = [0];
    let value = [1];
    db.write(key.clone(), value.clone()).await.unwrap();
    let res = db.read(key).await.unwrap().unwrap();
    assert_eq!(value.to_vec(), res);
}

pub async fn exists<DB>(db: &DB)
where
    DB: Store,
{
    let key = [0];
    let value = [1];
    db.write(key.clone(), value.clone()).await.unwrap();
    let res = db.exists(key).await.unwrap();
    assert_eq!(res, true);
}

pub async fn does_not_exist<DB>(db: &DB)
where
    DB: Store,
{
    let key = [0];
    let res = db.exists(key).await.unwrap();
    assert_eq!(res, false);
}

pub async fn delete<DB>(db: &DB)
where
    DB: Store,
{
    let key = [0];
    let value = [1];
    db.write(key.clone(), value.clone()).await.unwrap();
    let res = db.exists(key.clone()).await.unwrap();
    assert_eq!(res, true);
    db.delete(key.clone()).await.unwrap();
    let res = db.exists(key.clone()).await.unwrap();
    assert_eq!(res, false);
}

pub async fn bulk_write<DB>(db: &DB)
where
    DB: Store,
{
    let keys = [[0], [1], [2]];
    let values = [[0], [1], [2]];
    db.bulk_write(&keys, &values).await.unwrap();
    for k in keys.iter() {
        let res = db.exists(k.clone()).await.unwrap();
        assert_eq!(res, true);
    }
}

pub async fn bulk_read<DB>(db: &DB)
where
    DB: Store,
{
    let keys = [[0], [1], [2]];
    let values = [[0], [1], [2]];
    db.bulk_write(&keys, &values).await.unwrap();
    let results = db.bulk_read(&keys).await.unwrap();
    for (result, value) in results.iter().zip(values.iter()) {
        match result {
            Some(v) => assert_eq!(v, value),
            None => panic!("No values found!"),
        }
    }
}

pub async fn bulk_delete<DB>(db: &DB)
where
    DB: Store,
{
    let keys = [[0], [1], [2]];
    let values = [[0], [1], [2]];
    db.bulk_write(&keys, &values).await.unwrap();
    db.bulk_delete(&keys).await.unwrap();
    for k in keys.iter() {
        let res = db.exists(k.clone()).await.unwrap();
        assert_eq!(res, false);
    }
}
