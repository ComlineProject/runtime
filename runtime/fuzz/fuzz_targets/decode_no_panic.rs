// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A format, a type and bytes: decoding never panics, never allocates more
//! than the input justifies (run with `-rss_limit_mb` and
//! `-malloc_limit_mb`), and whatever it accepts is a value of the type.

#![no_main]

use arbitrary::Unstructured;
use comline_runtime_conformance::{check, decode_typed};
use comline_runtime_fuzz::{read_format, read_type};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let format = read_format(&mut u);
    let ty = read_type(&mut u);
    let bytes = u.take_rest();
    if let Some(value) = decode_typed(format, &ty, bytes).expect("a known format") {
        if let Err(why) = check(&value, &ty) {
            panic!("{format} decoded {bytes:02x?} to something else: {why}");
        }
    }
});
