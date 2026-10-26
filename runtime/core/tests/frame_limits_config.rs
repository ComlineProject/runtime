//! A frame limit other than the default: lower and higher, on each
//! transport (`with_max_frame`) and on the async connection
//! (`set_max_frame`), and a limit past what the `u32` prefix can declare,
//! which is refused.
#![cfg(feature = "std")]

use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use comline_runtime::connection::Connection;
use comline_runtime::contract::stream::{DEFAULT_MAX_FRAME, PREFIX_CEILING};
use comline_runtime::contract::{
    Call, Dispatch, Envelope, Kind, Peer, Reply, RuntimeError, WireFormat,
};
use comline_runtime::format::MsgPack;
use comline_runtime::transport::{duplex, Tcp, Transport};

const ECHO: u16 = 0;
const ECHO_CALLS: &[&str] = &["echo", "big"];
const PATIENCE: Duration = Duration::from_secs(10);

/// A limit well under the default.
const LOW: usize = 64;
/// A limit over the default.
const HIGH: usize = 2 * DEFAULT_MAX_FRAME;

/// `echo(text: str) -> str`, and `big(len: u64) -> str`, a reply of `len`
/// bytes.
struct EchoDispatcher;

impl Dispatch for EchoDispatcher {
    fn calls(&self) -> &'static [&'static str] {
        ECHO_CALLS
    }

    fn dispatch<W: WireFormat>(
        &self,
        call: Kind,
        params: &[u8],
        format: &W,
        reply: &mut Reply,
    ) -> Result<(), RuntimeError> {
        let text = match call.resolve(ECHO_CALLS).ok_or(RuntimeError::UnknownCall)? {
            0 => format.decode::<String>(params)?,
            1 => "x".repeat(format.decode::<u64>(params)? as usize),
            _ => return Err(RuntimeError::UnknownCall),
        };
        let mut body = Vec::new();
        format.encode(&text, &mut body)?;
        reply.ok(&body);
        Ok(())
    }
}

fn decode_text<W: WireFormat>(
    (envelope, format): (Envelope<'_>, &W),
) -> Result<String, RuntimeError> {
    match envelope {
        Envelope::Ok(payload) => format.decode(payload),
        Envelope::Fault => Err(RuntimeError::Provider),
        Envelope::Err { id, .. } => Err(RuntimeError::Remote { id }),
    }
}

fn sockets() -> (TcpStream, TcpStream) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let dialled = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (accepted, _) = listener.accept().unwrap();
    for s in [&dialled, &accepted] {
        s.set_read_timeout(Some(PATIENCE)).unwrap();
        s.set_write_timeout(Some(PATIENCE)).unwrap();
    }
    (dialled, accepted)
}

#[test]
fn the_default_limit_is_the_transports_unless_configured() {
    let (dialled, _) = sockets();
    assert_eq!(Tcp::new(dialled).max_frame(), DEFAULT_MAX_FRAME);
    let (a, _) = duplex();
    assert_eq!(a.max_frame(), DEFAULT_MAX_FRAME);

    let (dialled, _) = sockets();
    assert_eq!(
        Tcp::new(dialled).with_max_frame(LOW).unwrap().max_frame(),
        LOW
    );
    let (a, _) = duplex();
    assert_eq!(a.with_max_frame(HIGH).max_frame(), HIGH);
}

#[test]
fn a_tcp_limit_above_the_prefix_ceiling_is_refused() {
    let (dialled, _) = sockets();
    let tcp = Tcp::new(dialled).with_max_frame(PREFIX_CEILING).unwrap();
    assert_eq!(tcp.max_frame(), u32::MAX as usize);
    if let Some(over) = PREFIX_CEILING.checked_add(1) {
        let (dialled, _) = sockets();
        assert_eq!(
            Tcp::new(dialled).with_max_frame(over).err(),
            Some(RuntimeError::FrameTooLarge)
        );
    }
}

#[test]
fn a_lower_tcp_limit_holds_both_ways() {
    let (dialled, accepted) = sockets();
    let mut low = Tcp::new(dialled).with_max_frame(LOW).unwrap();
    let mut default = Tcp::new(accepted);
    assert_eq!(low.send(&[1; LOW + 1]), Err(RuntimeError::FrameTooLarge));
    low.send(&[2; LOW]).unwrap();
    let mut buf = Vec::new();
    default.recv(&mut buf).unwrap();
    assert_eq!(buf, [2; LOW], "only the frame within the limit went out");

    // The default side sends what the low side refuses: it closes.
    let (dialled, accepted) = sockets();
    let mut low = Tcp::new(dialled).with_max_frame(LOW).unwrap();
    let mut default = Tcp::new(accepted);
    default.send(&[3; LOW + 1]).unwrap();
    assert_eq!(low.recv(&mut buf), Err(RuntimeError::FrameTooLarge));
    assert_eq!(low.recv(&mut buf), Err(RuntimeError::Transport));
}

#[test]
fn a_higher_tcp_limit_carries_frames_the_default_refuses() {
    let big = DEFAULT_MAX_FRAME + 1;
    let (dialled, accepted) = sockets();
    let high = Tcp::new(dialled).with_max_frame(HIGH).unwrap();
    let mut other = Tcp::new(accepted).with_max_frame(HIGH).unwrap();
    let sender = thread::spawn(move || {
        let mut high = high;
        high.send(&vec![7; big]).unwrap();
        high.send(&vec![8; HIGH]).unwrap();
        high.send(&vec![9; HIGH + 1])
    });
    let mut buf = Vec::new();
    other.recv(&mut buf).unwrap();
    assert_eq!(buf.len(), big);
    other.recv(&mut buf).unwrap();
    assert_eq!(buf.len(), HIGH);
    assert_eq!(sender.join().unwrap(), Err(RuntimeError::FrameTooLarge));

    // A peer at the default refuses what the higher limit allowed.
    let (dialled, accepted) = sockets();
    let high = Tcp::new(dialled).with_max_frame(HIGH).unwrap();
    let mut default = Tcp::new(accepted);
    let sender = thread::spawn(move || {
        let mut high = high;
        let _ = high.send(&vec![7; big]);
    });
    assert_eq!(default.recv(&mut buf), Err(RuntimeError::FrameTooLarge));
    drop(default);
    sender.join().unwrap();
}

#[test]
fn in_memory_holds_frames_to_its_limit() {
    let (a, b) = duplex();
    let (mut a, mut b) = (a.with_max_frame(LOW), b.with_max_frame(HIGH));
    let mut buf = Vec::new();

    assert_eq!(a.send(&[0; LOW + 1]), Err(RuntimeError::FrameTooLarge));
    a.send(&[1; LOW]).unwrap();
    b.recv(&mut buf).unwrap();
    assert_eq!(buf, [1; LOW]);

    // `b` may send more than `a` takes: `a` refuses it, and closes.
    b.send(&[2; LOW + 1]).unwrap();
    b.send(&[3; 1]).unwrap();
    assert_eq!(a.recv(&mut buf), Err(RuntimeError::FrameTooLarge));
    assert_eq!(a.recv(&mut buf), Err(RuntimeError::Transport));
    assert_eq!(a.send(&[4]), Err(RuntimeError::Transport));
}

#[test]
fn a_connection_checks_against_its_transports_configured_limit() {
    let (a, b) = duplex();
    let server = thread::spawn(move || {
        let mut conn = Connection::new_unverified(b.with_max_frame(LOW), MsgPack);
        conn.provide(ECHO, EchoDispatcher);
        conn.serve()
    });
    let mut conn = Connection::new_unverified(a.with_max_frame(LOW), MsgPack);

    let over = "x".repeat(LOW);
    let refused = conn
        .call(ECHO, Call::new(0, "echo"), &over)
        .and_then(decode_text);
    let notified = conn.notify(ECHO, Call::new(0, "echo"), &over);
    let big_reply = conn
        .call(ECHO, Call::new(1, "big"), &(LOW as u64))
        .and_then(decode_text);
    let next = conn
        .call(ECHO, Call::new(0, "echo"), &"hi")
        .and_then(decode_text);
    assert_eq!(
        (refused, notified, big_reply, next),
        (
            Err(RuntimeError::FrameTooLarge),
            Err(RuntimeError::FrameTooLarge),
            Err(RuntimeError::Provider),
            Ok("hi".into())
        )
    );

    drop(conn);
    server.join().unwrap().unwrap();
}

#[cfg(feature = "tokio")]
mod tokio_tcp {
    use super::*;

    use comline_runtime::async_connection::{
        async_duplex, tokio as rt_tokio, Answer, AsyncConnection, AsyncDispatch, AsyncPeer,
        AsyncTransport, OwnedEnvelope,
    };
    use futures_util::{SinkExt, StreamExt};

    struct AsyncEcho;

    impl AsyncDispatch for AsyncEcho {
        fn calls(&self) -> &'static [&'static str] {
            ECHO_CALLS
        }

        async fn dispatch<'a, P: AsyncPeer>(
            &'a self,
            call: Kind,
            params: &'a [u8],
            peer: &'a P,
        ) -> Result<Answer, RuntimeError> {
            let mut body = Vec::new();
            let mut reply = Reply::new(&mut body);
            EchoDispatcher.dispatch(call, params, peer.format(), &mut reply)?;
            Ok(Answer::ok(body))
        }
    }

    fn decode_owned(reply: Result<OwnedEnvelope, RuntimeError>) -> Result<String, RuntimeError> {
        decode_text((reply?.envelope(), &MsgPack))
    }

    /// Calls over a connection whose ends both hold frames to `limit`: one
    /// over it fails alone, a reply over it is a fault, and one at the limit
    /// crosses.
    async fn calls_hold_to<T: AsyncTransport>(limit: usize, a: T, b: T) {
        let mut client = AsyncConnection::new_unverified(a, MsgPack);
        let mut server = AsyncConnection::new_unverified(b, MsgPack);
        client.set_max_frame(limit);
        server.set_max_frame(limit);
        server.provide(ECHO, AsyncEcho);
        let caller = client.handle();
        let client = tokio::spawn(client.run());
        let server = tokio::spawn(server.run());

        let over = "x".repeat(limit);
        let refused = decode_owned(caller.call(ECHO, Call::new(0, "echo"), &over).await);
        let big_reply = decode_owned(
            caller
                .call(ECHO, Call::new(1, "big"), &(limit as u64))
                .await,
        );
        // MsgPack's str header and `datagram@2`'s head: what fills the
        // frame up to the limit exactly.
        let fits = "x".repeat(limit - 12 - if limit > 0xffff { 5 } else { 2 });
        let at_limit = decode_owned(caller.call(ECHO, Call::new(0, "echo"), &fits).await);
        assert_eq!(
            (refused, big_reply, at_limit.map(|s| s.len())),
            (
                Err(RuntimeError::FrameTooLarge),
                Err(RuntimeError::Provider),
                Ok(fits.len())
            )
        );

        caller.close();
        client.await.unwrap().unwrap();
        server.await.unwrap().unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_lower_limit_on_an_async_connection_in_memory() {
        let (a, b) = async_duplex();
        calls_hold_to(
            LOW * 4,
            a.with_max_frame(LOW * 4),
            b.with_max_frame(LOW * 4),
        )
        .await;
    }

    async fn tokio_pair(limit: usize) -> (rt_tokio::Tcp, rt_tokio::Tcp) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (dialled, accepted) =
            tokio::join!(tokio::net::TcpStream::connect(addr), listener.accept());
        (
            rt_tokio::Tcp::new(dialled.unwrap())
                .with_max_frame(limit)
                .unwrap(),
            rt_tokio::Tcp::new(accepted.unwrap().0)
                .with_max_frame(limit)
                .unwrap(),
        )
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_lower_limit_on_an_async_connection_over_tcp() {
        let (a, b) = tokio_pair(LOW * 4).await;
        calls_hold_to(LOW * 4, a, b).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_higher_limit_on_an_async_connection_over_tcp() {
        let (a, b) = tokio_pair(HIGH).await;
        calls_hold_to(HIGH, a, b).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_tokio_limit_above_the_prefix_ceiling_is_refused() {
        let (a, _) = tokio_pair(PREFIX_CEILING).await;
        assert_eq!(a.max_frame(), u32::MAX as usize);
        if let Some(over) = PREFIX_CEILING.checked_add(1) {
            let (a, _) = tokio_pair(LOW).await;
            assert_eq!(
                a.with_max_frame(over).err(),
                Some(RuntimeError::FrameTooLarge)
            );
        }
    }

    /// A lower limit on the tokio side refuses what a blocking peer at the
    /// default sends, and closes; its own sends stop at its limit.
    #[tokio::test(flavor = "multi_thread")]
    async fn a_lower_tokio_limit_against_a_blocking_peer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let blocking = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(PATIENCE)).unwrap();
            let mut tcp = Tcp::new(stream);
            let mut buf = Vec::new();
            tcp.recv(&mut buf).unwrap();
            assert_eq!(buf.len(), LOW);
            tcp.send(&[1; LOW + 1]).unwrap();
        });

        let mut tcp = rt_tokio::Tcp::connect(addr)
            .await
            .unwrap()
            .with_max_frame(LOW)
            .unwrap();
        assert_eq!(
            tcp.send(vec![0; LOW + 1]).await,
            Err(RuntimeError::FrameTooLarge)
        );
        tcp.send(vec![0; LOW]).await.unwrap();
        assert_eq!(tcp.next().await, Some(Err(RuntimeError::FrameTooLarge)));
        assert_eq!(tcp.next().await, Some(Err(RuntimeError::Transport)));
        tokio::task::spawn_blocking(move || blocking.join().unwrap())
            .await
            .unwrap();
    }
}
