// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `TypeSeed` decodes exactly as a generated type does.
//!
//! Each mirror (in `common/mod.rs`) is declared as the Rust generator declares
//! types: plain `serde` derives, `serde_big_array` past 32 elements. For every
//! format, the bytes of sample values, every single-byte mutation of them,
//! every truncation and a trailing byte go through both the derive and the
//! seed. Both must accept or both refuse, and what they accept must be the
//! same value. Two decodes are compared by their `packed@1` encoding, which is
//! positional and deterministic. `tests/properties.rs` does the same with
//! proptest's mutations and random bytes.

mod common;

use common::{mirror, FORMATS};

/// The inputs a decode is tried on: each sample's bytes, every single-byte
/// mutation, every truncation, and a byte appended.
fn inputs(valid: &[Vec<u8>]) -> Vec<Vec<u8>> {
    let mut all = Vec::new();
    for bytes in valid {
        all.push(bytes.clone());
        for i in 0..bytes.len() {
            for flip in [0x01, 0x20, 0x80, 0xff] {
                let mut mutated = bytes.clone();
                mutated[i] ^= flip;
                all.push(mutated);
            }
            all.push(bytes[..i].to_vec());
        }
        for extra in [0x00, 0x01, b' ', b'0'] {
            let mut longer = bytes.clone();
            longer.push(extra);
            all.push(longer);
        }
    }
    all
}

/// Holds the seed for a mirror to its derive, on its samples' mutations and
/// on its hand-written inputs.
fn agrees(name: &str) {
    let m = mirror(name);
    for format in FORMATS {
        let mut tried = inputs(&m.valid(format));
        tried.extend(
            m.extra
                .iter()
                .filter(|(f, _)| *f == format)
                .map(|(_, b)| b.to_vec()),
        );
        for bytes in tried {
            if let Err(why) = m.agrees_on(format, &bytes) {
                panic!("{why}");
            }
        }
    }
}

#[test]
fn point() {
    agrees("Point");
}

#[test]
fn tagged() {
    agrees("Tagged");
}

#[test]
fn color() {
    agrees("Color");
}

#[test]
fn scalars() {
    agrees("Scalars");
}

#[test]
fn bag() {
    agrees("Bag");
}

#[test]
fn sample_with_fixed_arrays() {
    agrees("Sample");
}

#[test]
fn errors_and_unit() {
    agrees("TooLong");
    agrees("Denied");
    agrees("()");
}
