use crate::contract::{BufMut, RuntimeError, WireFormat};

/// How a call is addressed on the wire.
///
/// `Id` is the compact form — a function's position in its protocol's
/// declaration order, which is append-only. `Named` is for name-oriented
/// framings (JSON-RPC) and diagnostics; the `&'static str` comes from generated
/// `calls_names()`, never an owned `String`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Id(u16),
    Named(&'static str),
}

impl Kind {
    /// Resolve to a position in `calls` — a protocol's `calls_names()`, in
    /// declaration order. `Id(n)` is that position directly; `Named(s)` is
    /// looked up. `None` if out of range or not found — a generated dispatcher
    /// maps that to [`RuntimeError::UnknownCall`].
    pub fn resolve(&self, calls: &[&str]) -> Option<usize> {
        match self {
            Kind::Id(id) => {
                let idx = *id as usize;
                (idx < calls.len()).then_some(idx)
            }
            Kind::Named(name) => calls.iter().position(|c| c == name),
        }
    }
}

/// The provider side of a protocol: given an inbound call and its encoded
/// params, run the user's handler and write the response [`Envelope`] into
/// `out`.
///
/// Sync — no boxed futures on the `no_std` path. An async server layer wraps
/// this behind the `std` feature. The generated `<Proto>Dispatcher` implements
/// it; the call system holds one by value / `&D` (generic — no vtable, no
/// `dyn`) and calls it with the format it was configured with.
///
/// [`Envelope`]: crate::contract::Envelope
pub trait Dispatch {
    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        format: &W,
        out: &mut dyn BufMut,
    ) -> Result<(), RuntimeError>;
}

#[cfg(test)]
mod tests {
    use super::Kind;

    const CALLS: &[&str] = &["send", "history", "notify"];

    #[test]
    fn id_resolves_by_position() {
        assert_eq!(Kind::Id(0).resolve(CALLS), Some(0));
        assert_eq!(Kind::Id(2).resolve(CALLS), Some(2));
        assert_eq!(Kind::Id(3).resolve(CALLS), None);
    }

    #[test]
    fn named_resolves_by_lookup() {
        assert_eq!(Kind::Named("history").resolve(CALLS), Some(1));
        assert_eq!(Kind::Named("missing").resolve(CALLS), None);
    }
}
