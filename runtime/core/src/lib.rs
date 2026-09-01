// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// `no_std`-first: the crate is `#![no_std]` unless the `std` feature is on
// (it is, by default). `--no-default-features` builds `contract` + `wire`.
#![cfg_attr(not(feature = "std"), no_std)]

#[cfg(feature = "alloc")]
extern crate alloc;

// The `core ↔ target` contract — `no_std`, allocation-free.
pub mod contract;

// Request / response framing — `no_std`, allocation-free.
pub mod wire;

// `WireFormat` implementations (`std`-gated for now — see the module).
#[cfg(feature = "std")]
pub mod format;

// The frame transport, the provider serve loop, and the consumer call side.
#[cfg(feature = "alloc")]
pub mod client;
#[cfg(feature = "alloc")]
pub mod serve;
#[cfg(feature = "alloc")]
pub mod transport;

// The Comline-package dynamic-library ABI.
#[cfg(feature = "std")]
pub mod package_abi;
