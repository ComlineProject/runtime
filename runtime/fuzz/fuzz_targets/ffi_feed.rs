// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The C ABI on its own rules. An arbitrary `comline_config` (the
//! supported framings, other names, any handshake, stream, ordering, limit
//! and room), then arbitrary calls on the connection and bytes fed to it in
//! arbitrary chunks, through the `extern "C"` functions; the input grammar
//! is in `src/ffi.rs`.
//!
//! Every call is checked against what the ABI promises
//! (`runtime/ffi/tests/common/drive.rs` lists it all): no panic crosses
//! and no handle is poisoned; every status is one the function documents;
//! every span lies inside the frame and the frame inside that call's input;
//! `NEED_MORE` and `BUFFER_TOO_SMALL` report sizes a retry succeeds with;
//! refusals spend no request id; nothing allocates after
//! `comline_connection_new`, but for `jsonrpc@1`'s known cases. No
//! reference: `ffi_vs_core` adds one.

#![no_main]

use comline_runtime_fuzz::ffi::{allocations, drive, read, Counting};
use libfuzzer_sys::fuzz_target;

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fuzz_target!(|input: &[u8]| {
    let (config, ops) = read(input);
    if let Err(e) = drive::run(&config, &ops, drive::Mode::Abi, Some(allocations)) {
        panic!("{config:?}\n{e}");
    }
});
