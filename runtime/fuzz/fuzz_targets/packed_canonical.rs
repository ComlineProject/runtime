// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A type and bytes: if the bytes decode as the type in `packed@1`,
//! encoding what they decoded to gives the same bytes. The format promises
//! one encoding per value.

#![no_main]

use arbitrary::Unstructured;
use comline_runtime::contract::FORMAT_PACKED;
use comline_runtime_conformance::decode_typed;
use comline_runtime_fuzz::{encode, read_type};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let ty = read_type(&mut u);
    let bytes = u.take_rest();
    if let Some(value) = decode_typed(FORMAT_PACKED, &ty, bytes).expect("a known format") {
        let again = encode(FORMAT_PACKED, &value);
        assert_eq!(again, bytes, "{value:?} as {ty:?} has a second encoding");
    }
});
