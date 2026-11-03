//! JSON-RPC on a `Connection`: calls in both directions, with qualified
//! method names telling the protocols apart; a one-way call as a
//! notification, which gets no reply even when the function returns one; and
//! the async driver doing the same.
#![cfg(feature = "std")]

use std::sync::{Arc, Mutex};
use std::thread;

use comline_runtime::connection::Connection;
use comline_runtime::contract::{Call, Dispatch, Envelope, Kind, Peer, Reply, RuntimeError, WireFormat};
use comline_runtime::format::Json;
use comline_runtime::framing::JsonRpcFraming;
use comline_runtime::handshake::ComlineHandshake;
use comline_runtime::transport::{duplex, InMemory, Transport};
use serde::{Deserialize, Serialize};

// @framing = { name = "jsonrpc", method_names = qualified }
// protocol Chat {                        // provided by the accepting side, #0
//     function send(text: str) -> u64;  // calls Events::message back
//     function count() -> u64;
// }
// @framing = { name = "jsonrpc", method_names = qualified }
// @provider = Client
// protocol Events {                      // provided by the dialling side, #1
//     function message(text: str) -> bool;
//     function ping();                   // one-way
// }

const CHAT: u16 = 0;
const EVENTS: u16 = 1;
const CHAT_CALLS: &[&str] = &["app.Chat/send", "app.Chat/count"];
const EVENTS_CALLS: &[&str] = &["app.Events/message", "app.Events/ping"];
const IR_HASH: u64 = 0x1234;

#[derive(Serialize, Deserialize)]
struct TextParams {
    text: String,
}

fn ok<W: WireFormat, T: Serialize>(fmt: &W, value: &T, reply: &mut Reply) -> Result<(), RuntimeError> {
    let mut body = Vec::new();
    fmt.encode(value, &mut body)?;
    reply.ok(&body);
    Ok(())
}

fn decode_ok<'a, T: Deserialize<'a>, W: WireFormat>((envelope, fmt): (Envelope<'a>, &W)) -> Result<T, RuntimeError> {
    match envelope {
        Envelope::Ok(payload) => fmt.decode(payload),
        Envelope::Fault => Err(RuntimeError::Provider),
        Envelope::Err { id, .. } => Err(RuntimeError::Remote { id }),
    }
}

/// The accepting side's `Chat`: `send` shows the message on the caller's
/// `Events` first, and counts it.
#[derive(Clone, Default)]
struct ChatDispatcher {
    sent: Arc<Mutex<u64>>,
}

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
                let p: TextParams = peer.format().decode(params)?;
                let shown: bool = decode_ok(peer.call(EVENTS, Call::new(0, EVENTS_CALLS[0]), &p)?)?;
                assert!(shown);
                let mut sent = self.sent.lock().unwrap();
                *sent += 1;
                ok(peer.format(), &*sent, reply)
            }
            1 => ok(peer.format(), &*self.sent.lock().unwrap(), reply),
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

    fn dispatch<W: WireFormat>(&self, call: Kind, params: &[u8], fmt: &W, reply: &mut Reply) -> Result<(), RuntimeError> {
        match call.resolve(EVENTS_CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let p: TextParams = fmt.decode(params)?;
                self.seen.lock().unwrap().push(p.text);
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

type JsonRpcConnection = Connection<InMemory, Json, JsonRpcFraming>;

#[test]
fn calls_go_both_ways_named_by_protocol() {
    let (dialler, acceptor) = duplex();
    let chat = ChatDispatcher::default();
    let server = {
        let chat = chat.clone();
        thread::spawn(move || {
            let mut conn: JsonRpcConnection =
                Connection::accept_with(acceptor, Json, JsonRpcFraming, &mut ComlineHandshake, IR_HASH).unwrap();
            conn.provide(CHAT, chat);
            // A call of its own, then a notification, then serve.
            let shown: bool = decode_ok(
                conn.call(EVENTS, Call::new(0, EVENTS_CALLS[0]), &TextParams { text: "pushed".into() })
                    .unwrap(),
            )
            .unwrap();
            assert!(shown);
            conn.notify(EVENTS, Call::new(1, EVENTS_CALLS[1]), &()).unwrap();
            conn.serve().unwrap();
        })
    };
    let mut client: JsonRpcConnection =
        Connection::connect_with(dialler, Json, JsonRpcFraming, &mut ComlineHandshake, IR_HASH).unwrap();
    let events = EventsDispatcher::default();
    client.provide(EVENTS, events.clone());
    // The push and the ping first: a blocking side's calls can't cross.
    assert!(client.serve_one().unwrap() && client.serve_one().unwrap());

    let first: u64 =
        decode_ok(client.call(CHAT, Call::new(0, CHAT_CALLS[0]), &TextParams { text: "hi".into() }).unwrap()).unwrap();
    let count: u64 = decode_ok(client.call(CHAT, Call::new(1, CHAT_CALLS[1]), &()).unwrap()).unwrap();
    assert_eq!((first, count), (1, 1));
    assert_eq!(*events.seen.lock().unwrap(), ["pushed", "ping", "hi"]);

    // A method no protocol here has is a fault, and the connection survives.
    let (envelope, _) = client.call(CHAT, Call::new(0, "app.Chat/nope"), &()).unwrap();
    assert_eq!(envelope, Envelope::Fault);
    let count: u64 = decode_ok(client.call(CHAT, Call::new(1, CHAT_CALLS[1]), &()).unwrap()).unwrap();
    assert_eq!(count, 1);

    drop(client);
    server.join().unwrap();
}

fn text(frame: &[u8]) -> &str {
    std::str::from_utf8(frame).unwrap()
}

#[test]
fn a_notification_gets_no_reply() {
    let (mut raw, conn_side) = duplex();
    let server = thread::spawn(move || {
        let mut conn: JsonRpcConnection = Connection::with_framing_unverified(conn_side, Json, JsonRpcFraming);
        conn.provide(CHAT, ChatDispatcher::default());
        conn.notify(EVENTS, Call::new(1, EVENTS_CALLS[1]), &()).unwrap();
        conn.serve().unwrap();
    });

    let mut frame = Vec::new();
    raw.recv(&mut frame).unwrap();
    assert_eq!(text(&frame), r#"{"jsonrpc":"2.0","method":"app.Events/ping","params":null}"#);

    // `count` returns a value, but asked without an id it answers nothing:
    // the next frame back is the reply to the call after it.
    raw.send(br#"{"jsonrpc":"2.0","method":"app.Chat/count","params":null}"#).unwrap();
    raw.send(br#"{"jsonrpc":"2.0","method":"app.Chat/count","params":null,"id":3}"#).unwrap();
    raw.recv(&mut frame).unwrap();
    assert_eq!(text(&frame), r#"{"jsonrpc":"2.0","result":0,"id":3}"#);

    drop(raw);
    server.join().unwrap();
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn the_async_driver_finds_protocols_by_method_and_skips_notifications() {
    use comline_runtime::async_connection::{async_duplex, Answer, AsyncConnection, AsyncDispatch, AsyncPeer};
    use futures_util::{SinkExt, StreamExt};

    struct Count;

    impl AsyncDispatch for Count {
        fn calls(&self) -> &'static [&'static str] {
            CHAT_CALLS
        }

        async fn dispatch<'a, P: AsyncPeer>(
            &'a self,
            call: Kind,
            _: &'a [u8],
            peer: &'a P,
        ) -> Result<Answer, RuntimeError> {
            assert_eq!(call.resolve(CHAT_CALLS), Some(1));
            let mut body = Vec::new();
            peer.format().encode(&7u64, &mut body)?;
            Ok(Answer::ok(body))
        }
    }

    let (mut raw, conn_side) = async_duplex();
    let mut conn = AsyncConnection::with_framing_unverified(conn_side, Json, JsonRpcFraming);
    conn.provide(CHAT, Count);
    tokio::spawn(conn.run());

    raw.send(br#"{"jsonrpc":"2.0","method":"app.Chat/count","params":null}"#.to_vec()).await.unwrap();
    raw.send(br#"{"jsonrpc":"2.0","method":"app.Chat/count","params":null,"id":3}"#.to_vec()).await.unwrap();
    let reply = raw.next().await.unwrap().unwrap();
    assert_eq!(text(&reply), r#"{"jsonrpc":"2.0","result":7,"id":3}"#);
}
