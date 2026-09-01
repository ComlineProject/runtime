use crate::contract::RuntimeError;

/// A call's outcome on the **client** side: either a schema-declared error `E`
/// — the generated per-function error enum — or an infrastructure failure.
///
/// The generated provider trait method returns `Result<R, E>` (schema errors
/// only, it cannot fabricate a [`RuntimeError`]); the client stub returns
/// `Result<R, CallError<E>>`.
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
