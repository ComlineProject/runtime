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

// The frame transport and the provider serve loop.
#[cfg(feature = "alloc")]
pub mod serve;
#[cfg(feature = "alloc")]
pub mod transport;

// The Comline-package dynamic-library ABI.
#[cfg(feature = "std")]
pub mod package_abi;
