//! End to end, no network: a hand-written stand-in for what `comline-rust`
//! generates — a client stub and a `Dispatch` impl — driven directly with
//! `MsgPack`, no framing. Proves the `contract` surface (`Kind`, `WireFormat`,
//! `Dispatch`, `Reply`, `Outcome`, `CallError`) fits together.
#![cfg(feature = "std")]

use comline_runtime::contract::{
    CallError, Dispatch, Fault, Kind, Outcome, Reply, RuntimeError, WireFormat,
};
use comline_runtime::format::MsgPack;
use serde::{Deserialize, Serialize};

// The "schema":
//   protocol Echo {
//       function say(msg: str) -> str ! TooLong;
//       function bump(n: u32) -> u32;
//   }

#[derive(Serialize, Deserialize)]
struct SayParams<'a> {
    #[serde(borrow)]
    msg: &'a str,
}

#[derive(Serialize, Deserialize)]
struct BumpParams {
    n: u32,
}

#[derive(Serialize, Deserialize, PartialEq, Eq, Debug)]
struct TooLong {
    limit: u32,
}

const CALLS: &[&str] = &["say", "bump"];
const ERR_TOO_LONG: u16 = 0; // schema-global error ordinal

/// The generated per-function error enum for `say`.
#[derive(Debug, PartialEq, Eq)]
enum SayError {
    TooLong(TooLong),
}

/// The generated per-function error type for `bump`, which declares no `!`.
/// An alias to the never type, not a bespoke empty enum — nothing can be
/// constructed either way, but this one is `Display`, `Copy` and recognisable.
type BumpError = core::convert::Infallible;

// ── provider: the user trait + the generated dispatcher ─────────────────────

trait Echo {
    fn say(&self, msg: &str) -> Result<String, Fault<SayError>>;
    // Declares no `!`, so `BumpError` is uninhabited — and `Fault` is still
    // inhabited, so the implementation can fail without panicking.
    fn bump(&self, n: u32) -> Result<u32, Fault<BumpError>>;
}

struct EchoDispatcher<T>(T);

impl<T: Echo> Dispatch for EchoDispatcher<T> {
    fn calls(&self) -> &'static [&'static str] {
        CALLS
    }

    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        fmt: &W,
        reply: &mut Reply,
    ) -> Result<(), RuntimeError> {
        match call.resolve(CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let p: SayParams = fmt.decode(params)?;
                match self.0.say(p.msg) {
                    Ok(r) => {
                        let mut body = Vec::new();
                        fmt.encode(&r, &mut body)?;
                        reply.ok(&body);
                    }
                    Err(Fault::App(SayError::TooLong(e))) => {
                        let mut body = Vec::new();
                        fmt.encode(&e, &mut body)?;
                        reply.err(ERR_TOO_LONG, &body);
                    }
                    Err(Fault::Internal) => reply.fault(),
                }
                Ok(())
            }
            1 => {
                let p: BumpParams = fmt.decode(params)?;
                match self.0.bump(p.n) {
                    Ok(r) => {
                        let mut body = Vec::new();
                        fmt.encode(&r, &mut body)?;
                        reply.ok(&body);
                    }
                    // `BumpError` is uninhabited, so this arm is unreachable —
                    // it is what keeps the `match` exhaustive.
                    Err(Fault::App(never)) => match never {},
                    Err(Fault::Internal) => reply.fault(),
                }
                Ok(())
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

// ── consumer: the generated client stub (no framing — talks to the dispatcher) ─

struct EchoClient<'d, D> {
    dispatcher: &'d D,
    fmt: MsgPack,
}

impl<D: Dispatch> EchoClient<'_, D> {
    fn say(&self, msg: &str) -> Result<String, CallError<SayError>> {
        let mut params = Vec::new();
        self.fmt.encode(&SayParams { msg }, &mut params)?;

        let mut body = Vec::new();
        let mut reply = Reply::new(&mut body);
        self.dispatcher
            .dispatch(Kind::Id(0), &params, &self.fmt, &mut reply)?;

        match reply.outcome() {
            Outcome::Ok => self.fmt.decode(&body).map_err(CallError::Runtime),
            Outcome::Err(ERR_TOO_LONG) => {
                let e: TooLong = self.fmt.decode(&body)?;
                Err(CallError::App(SayError::TooLong(e)))
            }
            Outcome::Err(id) => Err(CallError::Runtime(RuntimeError::Remote { id })),
            Outcome::None => Err(CallError::Runtime(RuntimeError::Framing)),
            Outcome::Fault => Err(CallError::Runtime(RuntimeError::Provider)),
        }
    }

    fn bump(&self, n: u32) -> Result<u32, CallError<BumpError>> {
        let mut params = Vec::new();
        self.fmt.encode(&BumpParams { n }, &mut params)?;

        let mut body = Vec::new();
        let mut reply = Reply::new(&mut body);
        self.dispatcher
            .dispatch(Kind::Id(1), &params, &self.fmt, &mut reply)?;

        match reply.outcome() {
            Outcome::Ok => self.fmt.decode(&body).map_err(CallError::Runtime),
            Outcome::Err(id) => Err(CallError::Runtime(RuntimeError::Remote { id })),
            Outcome::None => Err(CallError::Runtime(RuntimeError::Framing)),
            Outcome::Fault => Err(CallError::Runtime(RuntimeError::Provider)),
        }
    }
}

// ── the service and the assertions ────────────────────────────────────────

struct Service;

impl Echo for Service {
    fn say(&self, msg: &str) -> Result<String, Fault<SayError>> {
        if msg.len() > 8 {
            return Err(Fault::App(SayError::TooLong(TooLong { limit: 8 })));
        }
        Ok(format!("echo: {msg}"))
    }

    fn bump(&self, n: u32) -> Result<u32, Fault<BumpError>> {
        // The schema promises `bump` cannot raise anything, and it still has a
        // way to fail that the schema says nothing about. Before `Fault`, the
        // only option here was a panic inside the dispatcher.
        n.checked_add(1).ok_or(Fault::Internal)
    }
}

fn client() -> (EchoDispatcher<Service>, MsgPack) {
    (EchoDispatcher(Service), MsgPack)
}

#[test]
fn ok_path_round_trips() {
    let (d, fmt) = client();
    let c = EchoClient {
        dispatcher: &d,
        fmt,
    };
    assert_eq!(c.say("hi").unwrap(), "echo: hi");
    assert_eq!(c.bump(41).unwrap(), 42);
}

#[test]
fn a_raised_error_reaches_the_client_typed() {
    let (d, fmt) = client();
    let c = EchoClient {
        dispatcher: &d,
        fmt,
    };
    let err = c.say("this is far too long").unwrap_err();
    assert_eq!(err, CallError::App(SayError::TooLong(TooLong { limit: 8 })));
}

#[test]
fn an_unknown_call_ordinal_is_a_runtime_error() {
    let (d, _) = client();
    let mut body = Vec::new();
    let mut reply = Reply::new(&mut body);
    let err = d
        .dispatch(Kind::Id(9), &[], &MsgPack, &mut reply)
        .unwrap_err();
    assert_eq!(err, RuntimeError::UnknownCall);
}

#[test]
fn a_provider_fault_reaches_the_client_as_a_runtime_error() {
    let (d, fmt) = client();
    let c = EchoClient {
        dispatcher: &d,
        fmt,
    };
    // `bump` declares no `!`, so its error type is uninhabited — and it can
    // still fail. The caller hears about it instead of seeing a panic.
    let err = c.bump(u32::MAX).unwrap_err();
    assert_eq!(err, CallError::Runtime(RuntimeError::Provider));
}

#[test]
fn a_fault_is_distinguishable_from_a_raised_schema_error() {
    let (d, fmt) = client();
    let c = EchoClient {
        dispatcher: &d,
        fmt,
    };
    assert!(matches!(
        c.say("this is far too long").unwrap_err(),
        CallError::App(_)
    ));
    assert!(matches!(
        c.bump(u32::MAX).unwrap_err(),
        CallError::Runtime(RuntimeError::Provider)
    ));
}
