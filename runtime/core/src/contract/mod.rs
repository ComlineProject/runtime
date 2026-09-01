//! The `core ↔ target` contract — runtime side.
//!
//! `no_std`, allocation-free: the shapes every generated protocol and every
//! `WireFormat` build against. Nothing here spends the call path on a heap
//! allocation or a payload copy.
//!
//! See `ComlineProject/docs` → Design → *The `core` ↔ target contract*, §4.

mod buf;
mod call;
mod dispatch;
mod envelope;
mod error;
mod wire;

pub use buf::{BufMut, SliceBuf};
pub use call::CallError;
pub use dispatch::{Dispatch, Kind};
pub use envelope::Envelope;
pub use error::RuntimeError;
pub use wire::WireFormat;
