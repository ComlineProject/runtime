// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A random bounded type graph (a root type and up to six declarations:
//! structs, unit enums, aliases, recursion under `@max_depth`), and values of
//! it, from the bytes: `comline-core` sizes the type for `packed@1`,
//! `msgpack@1` and `json@1`, and each real encoder, through the conformance
//! crate's `Wire`, must write a length inside core's `[min, max]` (and
//! `encoded_len` must say it), while core's least and most witnesses must
//! encode to exactly `min` and `max`. See `comline_runtime_fuzz::sizes`.

#![no_main]

use comline_runtime_fuzz::sizes::check_input;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Err(why) = check_input(data) {
        panic!("{why}");
    }
});
