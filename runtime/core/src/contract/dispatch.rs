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

/// What a dispatched call produced. Framing-agnostic: the dispatcher calls
/// [`ok`](Reply::ok) or [`err`](Reply::err) exactly once, or neither for a
/// one-way call. The framing then wraps the accumulated body.
pub struct Reply<'a> {
    body: &'a mut dyn BufMut,
    outcome: Outcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// One-way — nothing written, no response frame.
    None,
    /// A success payload was written.
    Ok,
    /// A schema error `body` was written, keyed by ordinal `id`.
    Err(u16),
}

impl<'a> Reply<'a> {
    pub fn new(body: &'a mut dyn BufMut) -> Self {
        Self {
            body,
            outcome: Outcome::None,
        }
    }

    /// Record a success: `payload` is the serialised return value.
    pub fn ok(&mut self, payload: &[u8]) {
        self.body.put_slice(payload);
        self.outcome = Outcome::Ok;
    }

    /// Record a raised schema error: `body` is the serialised error struct,
    /// `id` its schema-global ordinal.
    pub fn err(&mut self, id: u16, body: &[u8]) {
        self.body.put_slice(body);
        self.outcome = Outcome::Err(id);
    }

    pub fn outcome(&self) -> Outcome {
        self.outcome
    }
}

/// The provider side of a protocol: given an inbound call and its encoded
/// params, run the user's handler and record the outcome on `reply`.
///
/// Sync — no boxed futures on the `no_std` path. An async server layer wraps
/// this behind the `std` feature. The generated `<Proto>Dispatcher` implements
/// it; [`Server`](crate::serve::Server) holds one and routes inbound frames
/// to it.
pub trait Dispatch {
    /// The protocol's function names, in declaration order — so a name-oriented
    /// framing's method name can be resolved to an ordinal. The generated
    /// dispatcher returns its `<PROTO>_CALLS` constant.
    fn calls(&self) -> &'static [&'static str];

    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        format: &W,
        reply: &mut Reply,
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
