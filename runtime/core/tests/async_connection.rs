//! The async driver: calls in both directions from any task, a handler
//! calling back the peer that called it, requests in order unless
//! `concurrent`, timeouts and dropped calls, an unprovided protocol, a closed
//! transport, and an async end talking to a blocking one over TCP.
#![cfg(feature = "tokio")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use comline_runtime::async_connection::{
    async_duplex, tokio as rt_tokio, Answer, AsyncConnection, AsyncDispatch, AsyncInMemory, AsyncPeer,
    OwnedEnvelope,
};
use comline_runtime::contract::{Call, Envelope, Kind, RuntimeError, WireFormat};
use comline_runtime::format::MsgPack;
use serde::{Deserialize, Serialize};

// protocol Chat {                        // provided by the accepting side, #0
//     function send(text: str) -> u64;  // calls Events::message back
//     function count() -> u64;
//     function slow() -> u64;           // sleeps, reporting how many run at once
// }
// @provider = Client
// protocol Events {                      // provided by the dialling side, #1
//     function message(text: str) -> bool;
//     function ping();                   // one-way
// }

const CHAT: u16 = 0;
const EVENTS: u16 = 1;
const CHAT_CALLS: &[&str] = &["send", "count", "slow"];
const EVENTS_CALLS: &[&str] = &["message", "ping"];
const IR_HASH: u64 = 0x1234;

#[derive(Serialize, Deserialize)]
struct TextParams {
    text: String,
}

fn ok<W: WireFormat, T: Serialize>(fmt: &W, value: &T) -> Result<Answer, RuntimeError> {
    let mut body = Vec::new();
    fmt.encode(value, &mut body)?;
    Ok(Answer::ok(body))
}

fn decode_ok<T: for<'de> Deserialize<'de>>(reply: &OwnedEnvelope) -> Result<T, RuntimeError> {
    match reply.envelope() {
        Envelope::Ok(payload) => MsgPack.decode(payload),
        Envelope::Fault => Err(RuntimeError::Provider),
        Envelope::Err { id, .. } => Err(RuntimeError::Remote { id }),
    }
}

/// How many messages a connection's chat has sent, kept on the connection.
struct Sent(u64);

/// The accepting side's `Chat`.
#[derive(Default)]
struct ChatDispatcher {
    running: AtomicUsize,
    most_at_once: AtomicUsize,
}

impl AsyncDispatch for ChatDispatcher {
    fn calls(&self) -> &'static [&'static str] {
        CHAT_CALLS
    }

    async fn dispatch<'a, P: AsyncPeer>(
        &'a self,
        call: Kind,
        params: &'a [u8],
        peer: &'a P,
    ) -> Result<Answer, RuntimeError> {
        match call.resolve(CHAT_CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let p: TextParams = peer.format().decode(params)?;
                let shown: bool = decode_ok(&peer.call(EVENTS, Call::new(0, "message"), &p).await?)?;
                assert!(shown);
                let sent = {
                    let mut extensions = peer.extensions();
                    match extensions.get_mut::<Sent>() {
                        Some(sent) => {
                            sent.0 += 1;
                            sent.0
                        }
                        None => {
                            extensions.insert(Sent(1));
                            1
                        }
                    }
                };
                ok(peer.format(), &sent)
            }
            1 => {
                let sent = peer.extensions().get::<Sent>().map_or(0, |s| s.0);
                ok(peer.format(), &sent)
            }
            2 => {
                let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
                self.most_at_once.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
                self.running.fetch_sub(1, Ordering::SeqCst);
                ok(peer.format(), &(now as u64))
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

impl AsyncDispatch for EventsDispatcher {
    fn calls(&self) -> &'static [&'static str] {
        EVENTS_CALLS
    }

    async fn dispatch<'a, P: AsyncPeer>(
        &'a self,
        call: Kind,
        params: &'a [u8],
        peer: &'a P,
    ) -> Result<Answer, RuntimeError> {
        match call.resolve(EVENTS_CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => {
                let p: TextParams = peer.format().decode(params)?;
                if p.text == "nest" {
                    // Calls back the side whose handler is waiting on this one.
                    peer.call(CHAT, Call::new(1, "count"), &()).await?;
                }
                self.seen.lock().unwrap().push(p.text);
                ok(peer.format(), &true)
            }
            1 => {
                self.seen.lock().unwrap().push("ping".to_string());
                Ok(Answer::none())
            }
            _ => Err(RuntimeError::UnknownCall),
        }
    }
}

type Conn = AsyncConnection<AsyncInMemory, MsgPack>;

/// Both ends, handshaken, with `Chat` on the accepting one and `Events` on
/// the dialling one; neither is running yet.
async fn pair(chat: ChatDispatcher) -> (Conn, Conn, EventsDispatcher) {
    let (dialler, acceptor) = async_duplex();
    let (client, server) = tokio::join!(
        AsyncConnection::connect(dialler, MsgPack, IR_HASH),
        AsyncConnection::accept(acceptor, MsgPack, IR_HASH),
    );
    let (mut client, mut server) = (client.unwrap(), server.unwrap());
    server.provide(CHAT, chat);
    let events = EventsDispatcher::default();
    client.provide(EVENTS, events.clone());
    (client, server, events)
}

fn text(text: &str) -> TextParams {
    TextParams { text: text.to_string() }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_handler_calls_back_the_peer_that_called_it() {
    let (client, server, events) = pair(ChatDispatcher::default()).await;
    let chat = client.handle();
    let client = tokio::spawn(client.run());
    let server = tokio::spawn(server.run());

    let first: u64 = decode_ok(&chat.call(CHAT, Call::new(0, "send"), &text("hi")).await.unwrap()).unwrap();
    let second: u64 = decode_ok(&chat.call(CHAT, Call::new(0, "send"), &text("again")).await.unwrap()).unwrap();
    let count: u64 = decode_ok(&chat.call(CHAT, Call::new(1, "count"), &()).await.unwrap()).unwrap();

    assert_eq!((first, second, count), (1, 2, 2), "the count is kept on the accepting side's connection");
    assert_eq!(*events.seen.lock().unwrap(), ["hi", "again"]);
    chat.close();
    client.await.unwrap().unwrap();
    server.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_accepting_side_calls_from_another_task_while_serving() {
    let (client, server, events) = pair(ChatDispatcher::default()).await;
    let (chat, pusher) = (client.handle(), server.handle());
    tokio::spawn(client.run());
    tokio::spawn(server.run());

    // The server pushes from its own task while the client's calls are served.
    let push = tokio::spawn(async move {
        for i in 0..5 {
            let shown: bool =
                decode_ok(&pusher.call(EVENTS, Call::new(0, "message"), &text(&format!("push {i}"))).await.unwrap())
                    .unwrap();
            assert!(shown);
        }
        pusher.notify(EVENTS, Call::new(1, "ping"), &()).unwrap();
    });
    for _ in 0..5 {
        chat.call(CHAT, Call::new(0, "send"), &text("call")).await.unwrap();
    }
    push.await.unwrap();
    let count: u64 = decode_ok(&chat.call(CHAT, Call::new(1, "count"), &()).await.unwrap()).unwrap();
    assert_eq!(count, 5);

    let seen = events.seen.lock().unwrap().clone();
    assert_eq!(seen.iter().filter(|s| *s == "call").count(), 5);
    assert_eq!(seen.iter().filter(|s| s.starts_with("push")).count(), 5);
    let ping = seen.iter().position(|s| s == "ping").unwrap();
    assert!(
        seen.iter().rposition(|s| s.starts_with("push")).unwrap() < ping,
        "a one-way call after the pushes arrives after them: {seen:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nested_request_starts_while_its_handler_waits() {
    // `send` waits on `Events::message`, which calls `Chat::count` on the
    // same side: that has to start, in order or not, or both would wait
    // forever.
    let (client, server, events) = pair(ChatDispatcher::default()).await;
    let chat = client.handle();
    tokio::spawn(client.run());
    tokio::spawn(server.run());
    let sent = tokio::time::timeout(Duration::from_secs(5), chat.call(CHAT, Call::new(0, "send"), &text("nest")))
        .await
        .expect("the nested call deadlocked")
        .unwrap();
    assert_eq!(decode_ok::<u64>(&sent).unwrap(), 1);
    assert_eq!(*events.seen.lock().unwrap(), ["nest"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn requests_run_in_order_unless_concurrent() {
    for (limit, expect_overlap) in [(1, false), (4, true)] {
        let chat = ChatDispatcher::default();
        let (client, mut server, _) = pair(chat).await;
        server.concurrent(limit);
        let caller = client.handle();
        tokio::spawn(client.run());
        tokio::spawn(server.run());

        let calls: Vec<_> = (0..4)
            .map(|_| {
                let caller = caller.clone();
                tokio::spawn(async move { caller.call(CHAT, Call::new(2, "slow"), &()).await.unwrap() })
            })
            .collect();
        let mut most = 0;
        for call in calls {
            most = most.max(decode_ok::<u64>(&call.await.unwrap()).unwrap());
        }
        assert_eq!(most > 1, expect_overlap, "concurrent({limit}): at most {most} at once");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_timed_out_call_s_late_reply_is_dropped() {
    let (dialler, acceptor) = async_duplex();
    let mut client = AsyncConnection::new_unverified(dialler, MsgPack);
    client.set_timer(rt_tokio::timer());
    let caller = client.handle();
    tokio::spawn(client.run());

    let timed_out = caller
        .call_with_timeout(CHAT, Call::new(1, "count"), &(), Duration::from_millis(20))
        .await;
    assert_eq!(timed_out.err(), Some(RuntimeError::Timeout));

    // Only now does the other side start serving: the first call's reply
    // arrives first, and is skipped.
    let mut server = AsyncConnection::new_unverified(acceptor, MsgPack);
    server.provide(CHAT, ChatDispatcher::default());
    tokio::spawn(server.run());
    let count: u64 = decode_ok(&caller.call(CHAT, Call::new(1, "count"), &()).await.unwrap()).unwrap();
    assert_eq!(count, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_protocol_this_side_does_not_provide_is_a_fault() {
    let (client, server, _) = pair(ChatDispatcher::default()).await;
    let caller = client.handle();
    tokio::spawn(client.run());
    tokio::spawn(server.run());
    assert_eq!(caller.call(7, Call::new(0, "anything"), &()).await, Ok(OwnedEnvelope::Fault));
    let count: u64 = decode_ok(&caller.call(CHAT, Call::new(1, "count"), &()).await.unwrap()).unwrap();
    assert_eq!(count, 0, "the connection survives it");
}

#[tokio::test(flavor = "multi_thread")]
async fn calls_waiting_when_the_transport_closes_fail() {
    let (dialler, acceptor) = async_duplex();
    let client = AsyncConnection::new_unverified(dialler, MsgPack);
    let caller = client.handle();
    let run = tokio::spawn(client.run());
    let waiting = caller.call(CHAT, Call::new(1, "count"), &());
    drop(acceptor);
    assert_eq!(waiting.await, Err(RuntimeError::Transport));
    run.await.unwrap().unwrap();
    assert_eq!(caller.notify(CHAT, Call::new(1, "count"), &()), Err(RuntimeError::Transport));
}

#[tokio::test]
async fn a_mismatched_handshake_refuses_the_connection() {
    let (dialler, acceptor) = async_duplex();
    let (client, server) = tokio::join!(
        AsyncConnection::connect(dialler, MsgPack, 1),
        AsyncConnection::accept(acceptor, MsgPack, 2),
    );
    assert_eq!(client.err(), Some(RuntimeError::Handshake));
    assert_eq!(server.err(), Some(RuntimeError::Handshake));
}

/// An async end and a blocking one share a wire: the blocking side's `Chat`
/// calls the async side's `Events` back mid-request.
#[tokio::test(flavor = "multi_thread")]
async fn an_async_end_talks_to_a_blocking_one_over_tcp() {
    use comline_runtime::connection::Connection;
    use comline_runtime::contract::{Dispatch, Peer, Reply};
    use comline_runtime::transport::Tcp;

    struct BlockingChat;

    impl Dispatch for BlockingChat {
        fn calls(&self) -> &'static [&'static str] {
            CHAT_CALLS
        }

        fn dispatch<W: WireFormat>(&self, _: Kind, _: &[u8], _: &W, _: &mut Reply) -> Result<(), RuntimeError> {
            unreachable!("only served on a connection")
        }

        fn dispatch_with<P: Peer>(
            &self,
            _: Kind,
            params: &[u8],
            reply: &mut Reply,
            peer: &mut P,
        ) -> Result<(), RuntimeError> {
            let p: TextParams = peer.format().decode(params)?;
            let (envelope, fmt) = peer.call(EVENTS, Call::new(0, "message"), &p)?;
            let shown: bool = match envelope {
                Envelope::Ok(payload) => fmt.decode(payload)?,
                _ => return Err(RuntimeError::Provider),
            };
            let mut body = Vec::new();
            peer.format().encode(&(shown as u64 * 42), &mut body)?;
            reply.ok(&body);
            Ok(())
        }
    }

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let blocking = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut conn = Connection::accept(Tcp::new(stream), MsgPack, IR_HASH).unwrap();
        conn.provide(CHAT, BlockingChat);
        conn.serve().unwrap();
    });

    let transport = rt_tokio::Tcp::connect(addr).await.unwrap();
    let mut client = AsyncConnection::connect(transport, MsgPack, IR_HASH).await.unwrap();
    let events = EventsDispatcher::default();
    client.provide(EVENTS, events.clone());
    let caller = client.handle();
    let run = tokio::spawn(client.run());

    let answer: u64 = decode_ok(&caller.call(CHAT, Call::new(0, "send"), &text("over tcp")).await.unwrap()).unwrap();
    assert_eq!(answer, 42);
    assert_eq!(*events.seen.lock().unwrap(), ["over tcp"]);
    caller.close();
    run.await.unwrap().unwrap();
    tokio::task::spawn_blocking(move || blocking.join().unwrap()).await.unwrap();
}

#[test]
fn the_driver_is_send() {
    fn send<T: Send>(_: T) {}
    let (a, _) = async_duplex();
    send(AsyncConnection::new_unverified(a, MsgPack).run());
}
