// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The C ABI against the core it wraps. The same input as `ffi_feed`, each
//! call made through the `extern "C"` functions and again through
//! `comline-runtime`'s public API (`State`, the connection framings,
//! `HandshakeFrame`), as a Rust host would. Both must give the same status,
//! bytes, `*written`, request id, `*consumed` and event: kind, flags, ids,
//! envelope, and spans at the same offsets. `ffi_feed`'s checks run too.
//!
//! `runtime/ffi/tests/differential.rs` is the same comparison on proptest's
//! sequences, which mirror the core's `State` model test.

#![no_main]

use comline_runtime_fuzz::ffi::{allocations, drive, read, Counting};
use libfuzzer_sys::fuzz_target;

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fuzz_target!(|input: &[u8]| {
    let (config, ops) = read(input);
    if let Err(e) = drive::run(&config, &ops, drive::Mode::Differential, Some(allocations)) {
        panic!("{config:?}\n{e}");
    }
});
