// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

mod subtests;

use async_std::task;
use db::MemoryDB;

#[test]
fn mem_db_open() {
    task::block_on(async {
        let mut db = MemoryDB::default();
        subtests::open(&mut db);
        // Calling open on opened db should not error
        subtests::open(&mut db);
    });
}

#[test]
fn mem_db_write() {
    task::block_on(async {
        let db = MemoryDB::default();
        subtests::write(&db).await;
    });
}

#[test]
fn mem_db_read() {
    task::block_on(async {
        let db = MemoryDB::default();
        subtests::read(&db).await;
    });
}

#[test]
fn mem_db_exists() {
    task::block_on(async {
        let db = MemoryDB::default();
        subtests::exists(&db).await;
    });
}

#[test]
fn mem_db_does_not_exist() {
    task::block_on(async {
        let db = MemoryDB::default();
        subtests::does_not_exist(&db).await;
    });
}

#[test]
fn mem_db_delete() {
    task::block_on(async {
        let db = MemoryDB::default();
        subtests::delete(&db).await;
    });
}

#[test]
fn mem_db_bulk_write() {
    task::block_on(async {
        let db = MemoryDB::default();
        subtests::bulk_write(&db).await;
    });
}

#[test]
fn mem_db_bulk_read() {
    task::block_on(async {
        let db = MemoryDB::default();
        subtests::bulk_read(&db).await;
    });
}

#[test]
fn mem_db_bulk_delete() {
    task::block_on(async {
        let db = MemoryDB::default();
        subtests::bulk_delete(&db).await;
    });
}
