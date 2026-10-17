use crate::contract::RuntimeError;

/// A call's outcome on the **client** side: either a schema-declared error `E`
/// — the generated per-function error enum — or an infrastructure failure.
///
/// The generated provider trait method returns `Result<R, Fault<E>>` — a schema
/// error, or [`Fault::Internal`] for a failure the schema does not describe; it
/// cannot fabricate the transport-level arms of a [`RuntimeError`]. The client
/// stub returns `Result<R, CallError<E>>`, and a peer's [`Fault::Internal`]
/// arrives as [`RuntimeError::Provider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallError<E> {
    /// A schema error the peer raised.
    App(E),
    /// Transport, framing, timeout, or an unrecognised remote error.
    Runtime(RuntimeError),
}

impl<E> From<RuntimeError> for CallError<E> {
    fn from(error: RuntimeError) -> Self {
        CallError::Runtime(error)
    }
}

/// A call's outcome on the **provider** side: either a schema-declared error
/// `E` — the generated per-function error enum — or a failure the schema says
/// nothing about.
///
/// The mirror of [`CallError`], and deliberately a separate type: a provider
/// has no transport, no timeout and no handshake to fail at, so it can raise
/// neither those arms of a [`RuntimeError`] nor one on the peer's behalf.
///
/// [`Internal`](Self::Internal) exists because a schema that declares no `!`
/// for a function still leaves the implementation able to fail — a dead
/// database, a poisoned lock. Before it, the only recourse was a panic. It
/// also means a generated error type is never the whole story: `E` may be
/// uninhabited (`Infallible`, for a function with no `!`) while
/// `Fault<E>` stays inhabited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault<E> {
    /// A schema error, declared with `!` and carried to the caller intact.
    App(E),
    /// A failure outside the schema. Carries nothing: there is no wire
    /// representation for arbitrary provider detail, and inventing one would
    /// leak an implementation's internals to every caller. Log it provider-side
    /// and let the caller see [`RuntimeError::Provider`].
    Internal,
}

impl<E> From<E> for Fault<E> {
    fn from(error: E) -> Self {
        Fault::App(error)
    }
}
