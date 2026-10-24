// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// `no_std`-first: the crate is `#![no_std]` unless the `std` feature is on
// (it is, by default). `--no-default-features` builds `contract` + `wire`.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

// The `core ↔ target` contract — `no_std`, allocation-free. The Comline
// datagram `Framing` lives here (`contract::DatagramFraming`).
pub mod contract;

// `WireFormat` implementations. `packed@1` needs nothing; JSON and
// MessagePack need `std`.
pub mod format;

// Alternative `Framing` implementations that need `std` / extra deps
// (JSON-RPC). The datagram framing is in `contract`.
#[cfg(feature = "std")]
pub mod framing;

// The frame transport, the connection handshake, the provider serve loop, the
// consumer call side, and connections carrying calls both ways.
#[cfg(feature = "alloc")]
pub mod client;
#[cfg(feature = "alloc")]
pub mod connection;
// The async driver for connections, on any executor.
#[cfg(feature = "async")]
pub mod async_connection;
#[cfg(feature = "alloc")]
pub mod handshake;
#[cfg(feature = "alloc")]
pub mod serve;
#[cfg(feature = "alloc")]
pub mod transport;

// What a generated `validate()` reports, and the `Validate` trait it implements.
#[cfg(feature = "alloc")]
pub mod validation;

// The Comline-package dynamic-library ABI.
#[cfg(feature = "std")]
pub mod package_abi;
