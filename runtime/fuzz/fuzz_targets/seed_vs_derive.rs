// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A format, one of the fixed mirrors (`conformance/tests/common`) and
//! bytes: `TypeSeed` accepts and refuses exactly what the mirror's
//! `serde` derive does, and reads the same value.

#![no_main]

#[path = "../../conformance/tests/common/mod.rs"]
mod common;

use std::sync::OnceLock;

use arbitrary::Unstructured;
use comline_runtime_fuzz::read_format;
use libfuzzer_sys::fuzz_target;

fn mirrors() -> &'static [common::Mirror] {
    static MIRRORS: OnceLock<Vec<common::Mirror>> = OnceLock::new();
    MIRRORS.get_or_init(common::mirrors)
}

fuzz_target!(|data: &[u8]| {
    let mut u = Unstructured::new(data);
    let format = read_format(&mut u);
    let m = &mirrors()[u.arbitrary::<u8>().unwrap_or(0) as usize % mirrors().len()];
    let bytes = u.take_rest();
    if let Err(why) = m.agrees_on(format, bytes) {
        panic!("{why}");
    }
});
