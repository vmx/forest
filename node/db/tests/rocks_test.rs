// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

mod db_utils;
mod subtests;

use async_std::task;
use db::RocksDb;
use db_utils::DBPath;

#[test]
fn rocks_db_open() {
    let path = DBPath::new("start_rocks_test");
    let mut db = RocksDb::new(path.as_ref());
    subtests::open(&mut db);
    // Calling open on opened db should not error
    subtests::open(&mut db);
}

#[test]
fn rocks_db_write() {
    task::block_on(async {
        let path = DBPath::new("write_rocks_test");
        let mut db = RocksDb::new(path.as_ref());
        subtests::open(&mut db);
        subtests::write(&db).await;
    });
}

#[test]
fn rocks_db_read() {
    task::block_on(async {
        let path = DBPath::new("read_rocks_test");
        let mut db = RocksDb::new(path.as_ref());
        subtests::open(&mut db);
        subtests::read(&db).await;
    });
}

#[test]
fn rocks_db_exists() {
    task::block_on(async {
        let path = DBPath::new("exists_rocks_test");
        let mut db = RocksDb::new(path.as_ref());
        subtests::open(&mut db);
        subtests::exists(&db).await;
    });
}

#[test]
fn rocks_db_does_not_exist() {
    task::block_on(async {
        let path = DBPath::new("does_not_exists_rocks_test");
        let mut db = RocksDb::new(path.as_ref());
        subtests::open(&mut db);
        subtests::does_not_exist(&db).await;
    });
}

#[test]
fn rocks_db_delete() {
    task::block_on(async {
        let path = DBPath::new("delete_rocks_test");
        let mut db = RocksDb::new(path.as_ref());
        subtests::open(&mut db);
        subtests::delete(&db).await;
    });
}

#[test]
fn rocks_db_bulk_write() {
    task::block_on(async {
        let path = DBPath::new("bulk_write_rocks_test");
        let mut db = RocksDb::new(path.as_ref());
        subtests::open(&mut db);
        subtests::bulk_write(&db).await;
    });
}

#[test]
fn rocks_db_bulk_read() {
    task::block_on(async {
        let path = DBPath::new("bulk_read_rocks_test");
        let mut db = RocksDb::new(path.as_ref());
        subtests::open(&mut db);
        subtests::bulk_read(&db).await;
    });
}

#[test]
fn rocks_db_bulk_delete() {
    task::block_on(async {
        let path = DBPath::new("bulk_delete_rocks_test");
        let mut db = RocksDb::new(path.as_ref());
        subtests::open(&mut db);
        subtests::bulk_delete(&db).await;
    });
}
