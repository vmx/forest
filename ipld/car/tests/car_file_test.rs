// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

use db::MemoryDB;
use forest_car::*;
use std::fs::File;
use std::io::BufReader;
use async_std::task;

#[test]
fn load_into_blockstore() {
    task::block_on(async {
        let file = File::open("tests/devnet.car").unwrap();
        let buf_reader = BufReader::new(file);
        let mut bs = MemoryDB::default();

        load_car(&mut bs, buf_reader).await.unwrap();
    });
}
