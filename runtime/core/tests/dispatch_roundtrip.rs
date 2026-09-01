//! End to end, no network: a hand-written stand-in for what `comline-rust`
//! generates — a client stub and a `Dispatch` impl — driven directly with
//! `MsgPack`, no framing. Proves the `contract` surface (`Kind`, `WireFormat`,
//! `Dispatch`, `Reply`, `Outcome`, `CallError`) fits together.
#![cfg(feature = "std")]

use comline_runtime::contract::{
    CallError, Dispatch, Kind, Outcome, Reply, RuntimeError, WireFormat,
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

// ── provider: the user trait + the generated dispatcher ─────────────────────

trait Echo {
    fn say(&self, msg: &str) -> Result<String, SayError>;
    fn bump(&self, n: u32) -> Result<u32, core::convert::Infallible>;
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
                    Err(SayError::TooLong(e)) => {
                        let mut body = Vec::new();
                        fmt.encode(&e, &mut body)?;
                        reply.err(ERR_TOO_LONG, &body);
                    }
                }
                Ok(())
            }
            1 => {
                let p: BumpParams = fmt.decode(params)?;
                let r = self.0.bump(p.n).unwrap();
                let mut body = Vec::new();
                fmt.encode(&r, &mut body)?;
                reply.ok(&body);
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
        }
    }

    fn bump(&self, n: u32) -> Result<u32, CallError<core::convert::Infallible>> {
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
        }
    }
}

// ── the service and the assertions ────────────────────────────────────────

struct Service;

impl Echo for Service {
    fn say(&self, msg: &str) -> Result<String, SayError> {
        if msg.len() > 8 {
            return Err(SayError::TooLong(TooLong { limit: 8 }));
        }
        Ok(format!("echo: {msg}"))
    }

    fn bump(&self, n: u32) -> Result<u32, core::convert::Infallible> {
        Ok(n + 1)
    }
}

fn client() -> (EchoDispatcher<Service>, MsgPack) {
    (EchoDispatcher(Service), MsgPack)
}

#[test]
fn ok_path_round_trips() {
    let (d, fmt) = client();
    let c = EchoClient { dispatcher: &d, fmt };
    assert_eq!(c.say("hi").unwrap(), "echo: hi");
    assert_eq!(c.bump(41).unwrap(), 42);
}

#[test]
fn a_raised_error_reaches_the_client_typed() {
    let (d, fmt) = client();
    let c = EchoClient { dispatcher: &d, fmt };
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
