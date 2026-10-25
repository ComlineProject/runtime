// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The shared benchmark definitions (`runtime/conformance/benches/`), in Rust.
//!
//! The format shapes are declared as generated Rust declares types, with plain
//! `serde` derives, and each function builds the value its shape in
//! `formats.json` holds: the benches measure what generated code runs, so
//! they encode and decode these types, not the conformance crate's `Value`.
//! The constants are the framing, handshake and connection definitions.
//!
//! `runtime/conformance/tests/bench_definitions.rs` includes this file and
//! checks it against the JSON: each value must encode to exactly the bytes
//! its definition does, in every format, and each constant must match, so
//! the two can't drift.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

// ── framings.json ───────────────────────────────────────────────────────────

/// The payload sizes each framing is measured at, in bytes.
pub const PAYLOAD_SIZES: [usize; 4] = [16, 256, 4096, 65536];

/// The request id of the measured `ok` response.
pub const FRAME_REQUEST_ID: u64 = 42;

/// Its protocol, for a framing that carries one (`datagram@2`).
pub const FRAME_PROTOCOL: u16 = 1;

/// The payload of `size` bytes: a JSON string, `"` then `size - 2` copies of
/// `a` then `"`, so it is valid JSON for `jsonrpc@1` too.
pub fn payload(size: usize) -> Vec<u8> {
    let mut bytes = vec![b'a'; size];
    bytes[0] = b'"';
    bytes[size - 1] = b'"';
    bytes
}

// ── handshake.json ──────────────────────────────────────────────────────────

pub const HANDSHAKE_IR_HASH: u64 = 12_302_652_060_662_169_617;
pub const HANDSHAKE_WIRE_FORMAT: &str = "packed@1";
pub const HANDSHAKE_FRAMING: &str = "datagram@2";
pub const HANDSHAKE_CAPABILITIES: u32 = 3;

// ── connection.json ─────────────────────────────────────────────────────────
//
// One step: a fresh connection state frames a call to `CONNECTION_CALL` with
// `small-scalar` as its params, in `datagram@2` and `packed@1`, waiting on a
// reply; then it receives the `ok` reply to that call, with an empty payload.

pub const CONNECTION_FRAMING: &str = "datagram@2";
pub const CONNECTION_FORMAT: &str = "packed@1";
pub const CONNECTION_PROTOCOL: u16 = 1;
pub const CONNECTION_CALL: (u16, &str) = (1, "record");
pub const CONNECTION_PARAMS: &str = "small-scalar";

// ── formats.json ────────────────────────────────────────────────────────────

/// `small-scalar`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reading {
    pub id: u32,
    pub channel: u16,
    pub active: bool,
    pub value: f64,
    pub offset: i64,
}

pub fn small_scalar() -> Reading {
    Reading {
        id: 48_213,
        channel: 7,
        active: true,
        value: 21.5,
        offset: -1_250_000,
    }
}

/// `string-heavy`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    pub username: String,
    pub display_name: String,
    pub email: String,
    pub bio: String,
    pub locale: String,
    pub tags: Vec<String>,
}

pub fn string_heavy() -> Profile {
    Profile {
        username: "ada.lovelace".into(),
        display_name: "Ada Lovelace — \"The Enchantress of Numbers\"".into(),
        email: "ada.lovelace@analytical-engine.example".into(),
        bio: "Mathematician and writer, chiefly known for her work on Charles Babbage's \
              proposed mechanical general-purpose computer, the Analytical Engine. She was \
              the first to recognise that the machine had applications beyond pure \
              calculation.\nNotes: A–G, 1843."
            .into(),
        locale: "en-GB".into(),
        tags: vec![
            "mathematics".into(),
            "computing".into(),
            "poetical science".into(),
            "analytical engine".into(),
            "bernoulli numbers".into(),
            "née Byron".into(),
        ],
    }
}

/// `nested`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Order {
    pub id: u64,
    pub customer: Customer,
    pub shipping: Option<Address>,
    pub billing: Option<Address>,
    pub line: Line,
    pub note: Option<String>,
    pub gift: Option<Gift>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Customer {
    pub id: u32,
    pub name: String,
    pub address: Option<Address>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Address {
    pub street: String,
    pub city: String,
    pub postcode: Option<String>,
    pub country: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Line {
    pub sku: u32,
    pub quantity: u16,
    pub price: Option<Price>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Price {
    pub cents: i64,
    pub discount: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gift {
    pub from: Option<String>,
    pub wrapped: bool,
}

pub fn nested() -> Order {
    let home = Address {
        street: "12 Marylebone Road".into(),
        city: "London".into(),
        postcode: Some("NW1 5LR".into()),
        country: 826,
    };
    Order {
        id: 9_007_199_254_740_993,
        customer: Customer {
            id: 1_815,
            name: "Ada Lovelace".into(),
            address: Some(home.clone()),
        },
        shipping: Some(Address {
            street: "1 Rue de Rivoli".into(),
            city: "Paris".into(),
            postcode: None,
            country: 250,
        }),
        billing: None,
        line: Line {
            sku: 31_337,
            quantity: 3,
            price: Some(Price {
                cents: 129_900,
                discount: Some(15),
            }),
        },
        note: None,
        gift: Some(Gift {
            from: None,
            wrapped: true,
        }),
    }
}

/// An element of `large-array`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sample {
    pub x: i32,
    pub y: i32,
    pub z: i32,
    pub weight: u16,
}

/// The number of elements in `large-array`.
pub const LARGE_ARRAY_LEN: usize = 1000;

/// `large-array`: element `i` is `{x: 7i - 3500, y: 13i mod 1000, z: -i,
/// weight: 31i mod 65536}`.
pub fn large_array() -> Vec<Sample> {
    (0..LARGE_ARRAY_LEN as i64)
        .map(|i| Sample {
            x: (7 * i - 3500) as i32,
            y: (13 * i % 1000) as i32,
            z: -i as i32,
            weight: (31 * i % 65536) as u16,
        })
        .collect()
}

/// `enum-heavy`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Job {
    pub status: Status,
    pub priority: Priority,
    pub kind: Kind,
    pub previous: Option<Status>,
    pub history: Vec<Status>,
    pub kinds: Vec<Kind>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Status {
    Queued,
    Running,
    Paused,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Priority {
    Low,
    Normal,
    High,
    Critical,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Kind {
    Build,
    Test,
    Lint,
    Bench,
    Deploy,
    Release,
    Docs,
    Fuzz,
}

pub fn enum_heavy() -> Job {
    use Kind::*;
    use Status::*;
    Job {
        status: Running,
        priority: Priority::High,
        kind: Bench,
        previous: Some(Paused),
        history: vec![
            Queued, Running, Paused, Running, Failed, Queued, Running, Paused, Running, Succeeded,
            Queued, Running, Cancelled, Queued, Running, Running,
        ],
        kinds: vec![
            Build, Test, Lint, Bench, Deploy, Release, Docs, Fuzz, Test, Bench,
        ],
    }
}
