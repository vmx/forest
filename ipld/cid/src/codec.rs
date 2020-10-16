// Copyright 2020 ChainSafe Systems
// SPDX-License-Identifier: Apache-2.0, MIT

macro_rules! build_codec_enum {
    {$( $val:expr => $var:ident, )*} => {
        #[derive(PartialEq, Eq, Hash, Clone, Copy, Debug)]
        pub enum Codec {
            $( $var, )*
            Other(u64)
        }

        use Codec::*;

        impl Codec {
            /// Convert a number to the matching codec
            pub fn from(raw: u64) -> Self {
                match raw {
                    $( $val => $var, )*
                    x => Other(x),
                }
            }
        }

        impl From<Codec> for u64 {
            /// Convert to the matching integer code
            fn from(codec: Codec) -> u64 {
                match codec {
                    $( $var => $val, )*
                    Other(val) => val,
                }
            }
        }
    }
}

build_codec_enum! {
    0x55 => Raw,
    0x70 => DagProtobuf,
    0x71 => DagCBOR,
    0x78 => GitRaw,
    0x90 => EthereumBlock,
    0x91 => EthereumBlockList,
    0x92 => EthereumTxTrie,
    0x93 => EthereumTx,
    0x94 => EthereumTxReceiptTrie,
    0x95 => EthereumTxReceipt,
    0x96 => EthereumStateTrie,
    0x97 => EthereumAccountSnapshot,
    0x98 => EthereumStorageTrie,
    0xb0 => BitcoinBlock,
    0xb1 => BitcoinTx,
    0xc0 => ZcashBlock,
    0xc1 => ZcashTx,
    0xf101 => FilCommitmentUnsealed,
    0xf102 => FilCommitmentSealed,
    0x0129 => DagJSON,
}
