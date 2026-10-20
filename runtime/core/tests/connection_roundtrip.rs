//! Calls in both directions over one `Connection` (`datagram@2`): ordinary
//! calls, a handler calling back the peer that called it, a call the
//! accepting side starts on its own, one-way calls, an unprovided protocol,
//! per-connection extensions, and a timed-out call whose reply comes late.
#![cfg(feature = "std")]

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use comline_runtime::connection::Connection;
use comline_runtime::contract::{
    Call, Dispatch, Envelope, Kind, Peer, Reply, RuntimeError, WireFormat,
};
use comline_runtime::format::MsgPack;
use comline_runtime::transport::duplex;
use serde::{Deserialize, Serialize};

// protocol Chat {                        // provided by the accepting side, #0
//     function send(text: str) -> u64;  // calls Events::message back
//     function count() -> u64;
// }
// @provider = Client
// protocol Events {                      // provided by the dialling side, #1
//     function message(text: str) -> bool;
//     function ping();                   // one-way
// }

const CHAT: u16 = 0;
const EVENTS: u16 = 1;
const CHAT_CALLS: &[&str] = &["send", "count"];
const EVENTS_CALLS: &[&str] = &["message", "ping"];
const IR_HASH: u64 = 0x1234;

#[derive(Serialize, Deserialize)]
struct TextParams<'a> {
    #[serde(borrow)]
    text: &'a str,
}

fn ok<W: WireFormat, T: Serialize>(fmt: &W, value: &T, reply: &mut Reply) -> Result<(), RuntimeError> {
    let mut body = Vec::new();
    fmt.encode(value, &mut body)?;
    reply.ok(&body);
    Ok(())
}

fn decode_ok<'a, T: Deserialize<'a>, W: WireFormat>(
    (envelope, fmt): (Envelope<'a>, &W),
) -> Result<T, RuntimeError> {
    match envelope {
        Envelope::Ok(payload) => fmt.decode(payload),
        Envelope::Fault => Err(RuntimeError::Provider),
        Envelope::Err { id, .. } => Err(RuntimeError::Remote { id }),
    }
}

/// How many messages a connection's chat has sent, kept on the connection.
struct Sent(u64);

/// The accepting side's `Chat`: `send` asks the caller's `Events` to show the
/// message before answering.
struct ChatDispatcher;

impl Dispatch for ChatDispatcher {
    fn calls(&self) -> &'static [&'static str] {
        CHAT_CALLS
    }

    fn dispatch<W: WireFormat>(&self, _: Kind, _: &[u8], _: &W, _: &mut Reply) -> Result<(), RuntimeError> {
        unreachable!("only served on a connection")
    }

    fn dispatch_with<P: Peer>(
        &self,
        call: Kind,
        params: &[u8],
        reply: &mut Reply,
        peer: &mut P,
    ) -> Result<(), RuntimeError> {
        match call.resolve(CHAT_CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let text = peer.format().decode::<TextParams>(params)?.text.to_string();
                let shown: bool = decode_ok(peer.call(EVENTS, Call::new(0, "message"), &TextParams { text: &text })?)?;
                assert!(shown);
                let sent = match peer.extensions().get_mut::<Sent>() {
                    Some(sent) => {
                        sent.0 += 1;
                        sent.0
                    }
                    None => {
                        peer.extensions().insert(Sent(1));
                        1
                    }
                };
                ok(peer.format(), &sent, reply)
            }
            1 => {
                let sent = peer.extensions().get::<Sent>().map_or(0, |s| s.0);
                ok(peer.format(), &sent, reply)
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

/// The dialling side's `Events`: records what it was shown.
#[derive(Clone, Default)]
struct EventsDispatcher {
    seen: Arc<Mutex<Vec<String>>>,
}

impl Dispatch for EventsDispatcher {
    fn calls(&self) -> &'static [&'static str] {
        EVENTS_CALLS
    }

    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        fmt: &W,
        reply: &mut Reply,
    ) -> Result<(), RuntimeError> {
        match call.resolve(EVENTS_CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let p: TextParams = fmt.decode(params)?;
                self.seen.lock().unwrap().push(p.text.to_string());
                ok(fmt, &true, reply)
            }
            1 => {
                self.seen.lock().unwrap().push("ping".to_string());
                Ok(())
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

/// A dialling connection providing `Events`, and the accepting one on
/// another thread, handed to `serve`.
fn pair(
    serve: impl FnOnce(&mut Connection<comline_runtime::transport::InMemory, MsgPack>) + Send + 'static,
) -> (
    Connection<comline_runtime::transport::InMemory, MsgPack>,
    EventsDispatcher,
    thread::JoinHandle<()>,
) {
    let (dialler, acceptor) = duplex();
    let server = thread::spawn(move || {
        let mut conn = Connection::accept(acceptor, MsgPack, IR_HASH).unwrap();
        conn.provide(CHAT, ChatDispatcher);
        serve(&mut conn);
    });
    let mut client = Connection::connect(dialler, MsgPack, IR_HASH).unwrap();
    let events = EventsDispatcher::default();
    client.provide(EVENTS, events.clone());
    (client, events, server)
}

#[test]
fn a_handler_calls_back_the_peer_that_called_it() {
    let (mut client, events, server) = pair(|conn| conn.serve().unwrap());

    let first: u64 = decode_ok(client.call(CHAT, Call::new(0, "send"), &TextParams { text: "hi" }).unwrap()).unwrap();
    let second: u64 = decode_ok(client.call(CHAT, Call::new(0, "send"), &TextParams { text: "again" }).unwrap()).unwrap();
    let count: u64 = decode_ok(client.call(CHAT, Call::new(1, "count"), &()).unwrap()).unwrap();

    assert_eq!((first, second, count), (1, 2, 2), "the count is kept on the accepting side's connection");
    assert_eq!(*events.seen.lock().unwrap(), ["hi", "again"]);
    drop(client);
    server.join().unwrap();
}

#[test]
fn the_accepting_side_starts_calls_of_its_own() {
    let (mut client, events, server) = pair(|conn| {
        let shown: bool = decode_ok(conn.call(EVENTS, Call::new(0, "message"), &TextParams { text: "pushed" }).unwrap()).unwrap();
        assert!(shown);
        conn.notify(EVENTS, Call::new(1, "ping"), &()).unwrap();
    });

    // Served until the accepting side hangs up.
    client.serve().unwrap();
    server.join().unwrap();
    assert_eq!(*events.seen.lock().unwrap(), ["pushed", "ping"]);
}

#[test]
fn a_protocol_this_side_does_not_provide_is_a_fault() {
    let (mut client, _, server) = pair(|conn| conn.serve().unwrap());

    let (envelope, _) = client.call(7, Call::new(0, "anything"), &()).unwrap();
    assert_eq!(envelope, Envelope::Fault);
    // The connection survives it.
    let count: u64 = decode_ok(client.call(CHAT, Call::new(1, "count"), &()).unwrap()).unwrap();
    assert_eq!(count, 0);
    drop(client);
    server.join().unwrap();
}

#[test]
fn a_late_reply_to_a_timed_out_call_is_dropped() {
    let (mut client, _, server) = pair(|conn| {
        thread::sleep(Duration::from_millis(200));
        conn.serve().unwrap();
    });

    let timed_out = client.call_with_timeout(CHAT, Call::new(1, "count"), &(), Duration::from_millis(20));
    assert_eq!(timed_out.err(), Some(RuntimeError::Timeout));
    // The first call's reply arrives first, and is skipped.
    let count: u64 = decode_ok(client.call(CHAT, Call::new(1, "count"), &()).unwrap()).unwrap();
    assert_eq!(count, 0);
    drop(client);
    server.join().unwrap();
}

#[test]
fn connections_get_distinct_ids() {
    let (a, _) = duplex();
    let (b, _) = duplex();
    let a = Connection::new_unverified(a, MsgPack);
    let b = Connection::new_unverified(b, MsgPack);
    assert_ne!(a.id(), b.id());
}

#[test]
fn a_mismatched_handshake_refuses_the_connection() {
    let (dialler, acceptor) = duplex();
    let server = thread::spawn(move || Connection::accept(acceptor, MsgPack, 1).err());
    let client = Connection::connect(dialler, MsgPack, 2).err();
    assert_eq!(client, Some(RuntimeError::Handshake));
    assert_eq!(server.join().unwrap(), Some(RuntimeError::Handshake));
}
