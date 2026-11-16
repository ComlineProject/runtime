//! A Hello for a subset of the package's protocols (named connections,
//! runtime#124): building one from per-protocol entries, what a connection
//! that ran it lets its end provide, and which calls fail locally.
#![cfg(feature = "std")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;

use comline_runtime::connection::Connection;
use comline_runtime::contract::{
    Call, Datagram2Framing, Dispatch, Kind, Peer, Reason, Reply, RuntimeError, WireFormat,
};
use comline_runtime::format::MsgPack;
use comline_runtime::handshake::comline2::*;
use comline_runtime::transport::{duplex, InMemory, Transport};

const IR_HASH: u64 = 0x1234;

/// The `hello` case of the specification's `handshake-comline2` vectors:
/// protocols 0 and 1 of the package.
const HELLO: &str = "434f0201000123456789abcdef0123456789abcdef00030000030011111111111111111111111111111111012222222222222222222222222222222202333333333333333333333333333333330200000200a0a0a0a0a0a0a0a0a0a0a0a0a0a0a0a00300010201000100b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0020001";

/// Its `hello-subset` case: the same Hello with protocol 0's entry gone and
/// the count one.
const HELLO_SUBSET: &str = "434f0201000123456789abcdef0123456789abcdef00030000030011111111111111111111111111111111012222222222222222222222222222222202333333333333333333333333333333330101000100b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0b0020001";

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
        .collect()
}

fn hash(b: u8) -> Hash128 {
    Hash128([b; 16])
}

const ENCODINGS: [Encoding; 3] = [
    Encoding { kind: EncodingKind::Format, param_hash: Hash128([0x11; 16]) },
    Encoding { kind: EncodingKind::Framing, param_hash: Hash128([0x22; 16]) },
    Encoding { kind: EncodingKind::Handshake, param_hash: Hash128([0x33; 16]) },
];

fn header(encodings: &[Encoding]) -> HelloHeader<'_> {
    HelloHeader {
        exchange: Exchange::Offer,
        package_id: Hash128::from_hex("0123456789abcdef0123456789abcdef").unwrap(),
        version: Release::new(0, 3, 0),
        pre: "",
        encodings,
    }
}

/// Protocols 0, 1 and 2 of the package; the first two are the vectors'.
fn zero() -> ProtocolEntry<'static> {
    ProtocolEntry { number: 0, line: Release::new(0, 2, 0), shape_hash: hash(0xa0), functions: &[0, 1, 2] }
}

fn one() -> ProtocolEntry<'static> {
    ProtocolEntry { number: 1, line: Release::new(0, 1, 0), shape_hash: hash(0xb0), functions: &[0, 1] }
}

fn two() -> ProtocolEntry<'static> {
    ProtocolEntry { number: 2, line: Release::new(0, 1, 0), shape_hash: hash(0xc0), functions: &[0] }
}

fn encoded(hello: Hello) -> Vec<u8> {
    Message::Hello(hello).to_bytes()
}

// ── building a Hello ───────────────────────────────────────────────────────

#[test]
fn every_protocol_builds_the_whole_hello_byte_for_byte() {
    let header = header(&ENCODINGS);
    let whole = bytes(HELLO);
    // However the protocols are split between provided and called, in
    // whatever order, listed twice or not.
    let scrambled = ProtocolEntry { functions: &[2, 0, 1, 1], ..zero() };
    let splits: [(&[ProtocolEntry], &[ProtocolEntry]); 6] = [
        (&[zero(), one()], &[]),
        (&[], &[zero(), one()]),
        (&[zero()], &[one()]),
        (&[one()], &[zero()]),
        (&[one(), scrambled], &[zero(), one()]),
        (&[zero(), one()], &[one(), zero()]),
    ];
    for (provided, called) in splits {
        let built = encoded(Hello::build(&header, provided, called));
        assert_eq!(built, whole, "provided {provided:?}, called {called:?}");
    }
    // And it is the Hello that bytes decode to.
    assert_eq!(
        Message::decode(&whole),
        Ok(Message::Hello(Hello::build(&header, &[zero(), one()], &[])))
    );
}

#[test]
fn a_subset_builds_a_hello_of_those_protocols_alone() {
    let header = header(&ENCODINGS);
    let subset = bytes(HELLO_SUBSET);
    // Protocol 1 alone, provided, called, or both.
    let splits: [(&[ProtocolEntry], &[ProtocolEntry]); 3] =
        [(&[one()], &[]), (&[], &[one()]), (&[one()], &[one()])];
    for (provided, called) in splits {
        assert_eq!(encoded(Hello::build(&header, provided, called)), subset);
    }

    let hello = Hello::build(&header, &[zero()], &[]);
    assert_eq!(hello.protocols.iter().map(|p| p.number).collect::<Vec<_>>(), [0]);
    assert_eq!(hello.protocols[0].functions, [0, 1, 2]);

    // What it calls is narrowed to what's passed: the other side's protocol 2
    // isn't listed when the end doesn't call it.
    let hello = Hello::build(&header, &[zero()], &[one()]);
    assert_eq!(hello.protocols.iter().map(|p| p.number).collect::<Vec<_>>(), [0, 1]);
    let none = Hello::build(&header, &[], &[]);
    assert!(none.protocols.is_empty());
    assert_eq!(none.encodings, ENCODINGS);
}

#[test]
fn the_headers_encodings_are_put_in_wire_order() {
    let reversed = [ENCODINGS[2], ENCODINGS[0], ENCODINGS[1], ENCODINGS[0]];
    let hello = Hello::build(&header(&reversed), &[zero(), one()], &[]);
    assert_eq!(encoded(hello), bytes(HELLO));
}

#[test]
fn the_headers_version_and_exchange_are_the_hellos() {
    let header = HelloHeader {
        exchange: Exchange::Symmetric,
        pre: "dev.2",
        ..header(&ENCODINGS)
    };
    let hello = Hello::build(&header, &[one()], &[]);
    assert_eq!(hello.exchange, Exchange::Symmetric);
    assert_eq!(
        hello.version,
        PackageVersion { release: Release::new(0, 3, 0), pre: "dev.2".into() }
    );
    // `Comline2` sets the exchange from its parameters, as for any Hello.
    let params = Params { exchange: Exchange::Offer, ..Params::default() };
    assert_eq!(Comline2::new(params, hello).hello.exchange, Exchange::Offer);
}

// ── what a connection that ran a Hello lets its end provide ────────────────

/// A protocol with no calls, to register.
struct Nop;

impl Dispatch for Nop {
    fn calls(&self) -> &'static [&'static str] {
        &[]
    }

    fn dispatch<W: WireFormat>(
        &self,
        _: Kind,
        _: &[u8],
        _: &W,
        _: &mut Reply,
    ) -> Result<(), RuntimeError> {
        Err(RuntimeError::UnknownCall)
    }
}

/// Every protocol number tried in these tests: the package's, and one it
/// doesn't have.
const NUMBERS: [u16; 4] = [0, 1, 2, 7];

type Provided = Vec<Result<(), RuntimeError>>;

fn provide_all<T: Transport, F: comline_runtime::contract::ConnectionFraming>(
    conn: &mut Connection<T, MsgPack, F>,
) -> Provided {
    NUMBERS.iter().map(|n| conn.provide(*n, Nop)).collect()
}

/// Runs `comline@2` between `client` (which dials) and `server` over a
/// connection each, and tries to provide every one of [`NUMBERS`] on each:
/// what the dialling and the accepting end got.
fn provided_after(params: Params, client: Hello, server: Hello) -> (Provided, Provided) {
    let (dialler, acceptor) = duplex();
    let accepting = thread::spawn(move || {
        let mut handshake = Comline2Handshake::new(params, server);
        let mut conn = Connection::accept_with(acceptor, MsgPack, Datagram2Framing, &mut handshake, IR_HASH).unwrap();
        provide_all(&mut conn)
    });
    let mut handshake = Comline2Handshake::new(params, client);
    let mut conn = Connection::connect_with(dialler, MsgPack, Datagram2Framing, &mut handshake, IR_HASH).unwrap();
    let dialling = provide_all(&mut conn);
    (dialling, accepting.join().unwrap())
}

const OK: Result<(), RuntimeError> = Ok(());
const NO: Result<(), RuntimeError> = Err(RuntimeError::Unavailable);

#[test]
fn providing_a_protocol_the_hello_didnt_list_or_the_peer_refused_is_refused() {
    let header = header(&ENCODINGS);
    for exchange in [Exchange::Offer, Exchange::Symmetric] {
        let params = Params { exchange, ..Params::default() };

        // The dialling end lists protocols 0 and 1; the accepting one has 2
        // as well. For the dialling end, 2 is a protocol its Hello didn't
        // list; for the accepting end, one the peer refused
        // (`unknown_protocol`). Protocol 7 is in no Hello.
        let (dialling, accepting) = provided_after(
            params,
            Hello::build(&header, &[zero()], &[one()]),
            Hello::build(&header, &[zero(), one(), two()], &[]),
        );
        assert_eq!(dialling, [OK, OK, NO, NO], "dialling, {exchange:?}");
        assert_eq!(accepting, [OK, OK, NO, NO], "accepting, {exchange:?}");

        // Both ends list protocol 1, but the lines differ: each refuses it
        // on its own, and the others go through.
        let moved = ProtocolEntry { line: Release::new(0, 4, 0), ..one() };
        let (dialling, accepting) = provided_after(
            params,
            Hello::build(&header, &[zero(), two()], &[moved]),
            Hello::build(&header, &[zero(), one(), two()], &[]),
        );
        assert_eq!(dialling, [OK, NO, OK, NO], "dialling, line mismatch, {exchange:?}");
        assert_eq!(accepting, [OK, NO, OK, NO], "accepting, line mismatch, {exchange:?}");

        // Every protocol on both ends: everything the Hello lists.
        let all = |header: &HelloHeader<'_>| Hello::build(header, &[zero(), one(), two()], &[]);
        let (dialling, accepting) = provided_after(params, all(&header), all(&header));
        assert_eq!(dialling, [OK, OK, OK, NO], "dialling, whole, {exchange:?}");
        assert_eq!(accepting, [OK, OK, OK, NO], "accepting, whole, {exchange:?}");
    }
}

#[test]
fn without_a_hello_nothing_is_checked() {
    // Opened by hand, with no handshake at all.
    let (a, _b) = duplex();
    let mut conn = Connection::new_unverified(a, MsgPack);
    assert_eq!(provide_all(&mut conn), [OK, OK, OK, OK]);

    // `comline@1` has no Hello: it checks the schema's hash, not protocols.
    let (dialler, acceptor) = duplex();
    let accepting = thread::spawn(move || {
        let mut conn = Connection::accept(acceptor, MsgPack, IR_HASH).unwrap();
        provide_all(&mut conn)
    });
    let mut conn = Connection::connect(dialler, MsgPack, IR_HASH).unwrap();
    assert_eq!(provide_all(&mut conn), [OK, OK, OK, OK]);
    assert_eq!(accepting.join().unwrap(), [OK, OK, OK, OK]);
}

// ── calls outside the negotiated result ────────────────────────────────────

/// A transport that counts the frames written to it.
struct Counting {
    inner: InMemory,
    sent: Arc<AtomicUsize>,
}

impl Transport for Counting {
    fn send(&mut self, frame: &[u8]) -> Result<(), RuntimeError> {
        self.sent.fetch_add(1, Ordering::SeqCst);
        self.inner.send(frame)
    }

    fn recv(&mut self, buf: &mut Vec<u8>) -> Result<(), RuntimeError> {
        self.inner.recv(buf)
    }
}

#[test]
fn a_call_outside_the_negotiated_result_fails_locally_and_sends_nothing() {
    let header = header(&ENCODINGS);
    for exchange in [Exchange::Offer, Exchange::Symmetric] {
        let params = Params { exchange, ..Params::default() };
        // The dialling end lists protocol 0 alone: it provides it, and
        // narrowed what it calls to nothing. The accepting end has all three.
        let client = Hello::build(&header, &[zero()], &[]);
        let server = Hello::build(&header, &[zero(), one(), two()], &[]);

        let (dialler, acceptor) = duplex();
        // The accepting end's result, and its transport, kept open here.
        let accepting = thread::spawn(move || {
            let mut handshake = Comline2Handshake::new(params, server);
            let conn = Connection::accept_with(acceptor, MsgPack, Datagram2Framing, &mut handshake, IR_HASH).unwrap();
            (handshake.negotiated().cloned().unwrap(), conn.into_transport())
        });
        let sent = Arc::new(AtomicUsize::new(0));
        let transport = Counting { inner: dialler, sent: Arc::clone(&sent) };
        let mut handshake = Comline2Handshake::new(params, client);
        let mut conn = Connection::connect_with(transport, MsgPack, Datagram2Framing, &mut handshake, IR_HASH).unwrap();
        let (accepted, _accepting_end) = accepting.join().unwrap();
        let negotiated = handshake.negotiated().unwrap();

        // What a generated sender runs before it writes a request.
        let mut call = |protocol: u16, function: u16| -> Result<(), RuntimeError> {
            negotiated.check_call(protocol, function)?;
            conn.notify(protocol, Call::new(function, "f"), &())
        };

        let before = sent.load(Ordering::SeqCst);
        // Protocols the dialling end's Hello didn't list, and one in no
        // Hello: local, and nothing reached the transport.
        for protocol in [1, 2, 7] {
            assert_eq!(call(protocol, 0), Err(RuntimeError::Unavailable), "{protocol}, {exchange:?}");
        }
        assert_eq!(sent.load(Ordering::SeqCst), before, "{exchange:?}");
        // The accepting end holds them as refused (`unknown_protocol`): the
        // same error for it, for the same reason.
        for protocol in [1, 2] {
            assert_eq!(accepted.check_call(protocol, 0), Err(RuntimeError::Unavailable));
            assert_eq!(
                accepted.protocols.iter().find(|p| p.number == protocol).map(|p| &p.state),
                Some(&ProtocolState::Refused { reason: Reason::UnknownProtocol })
            );
        }
        for protocol in NUMBERS {
            assert_eq!(negotiated.accepts(protocol), negotiated.check_provide(protocol).is_ok());
            assert_eq!(negotiated.accepts(protocol), negotiated.check_call(protocol, 0).is_ok());
        }
        // A protocol the Hello listed goes through, and is written.
        assert_eq!(call(0, 1), Ok(()), "{exchange:?}");
        assert_eq!(sent.load(Ordering::SeqCst), before + 1, "{exchange:?}");
    }
}

// ── the same on an async connection ────────────────────────────────────────

#[cfg(feature = "async")]
mod asynchronous {
    use super::*;
    use comline_runtime::connection::{Answer, AsyncConnection, AsyncDispatch, AsyncPeer};
    use comline_runtime::transport::async_duplex;

    struct AsyncNop;

    impl AsyncDispatch for AsyncNop {
        fn calls(&self) -> &'static [&'static str] {
            &[]
        }

        async fn dispatch<'a, P: AsyncPeer>(
            &'a self,
            _: Kind,
            _: &'a [u8],
            _: &'a P,
        ) -> Result<Answer, RuntimeError> {
            Err(RuntimeError::UnknownCall)
        }
    }

    fn provide_all<T, F>(conn: &mut AsyncConnection<T, MsgPack, F>) -> Provided
    where
        T: comline_runtime::transport::AsyncTransport,
        F: comline_runtime::contract::ConnectionFraming + Send + Sync + 'static,
    {
        NUMBERS.iter().map(|n| conn.provide(*n, AsyncNop)).collect()
    }

    #[tokio::test]
    async fn providing_a_protocol_the_hello_didnt_list_or_the_peer_refused_is_refused() {
        let header = header(&ENCODINGS);
        for exchange in [Exchange::Offer, Exchange::Symmetric] {
            let params = Params { exchange, ..Params::default() };
            let (dialler, acceptor) = async_duplex();
            let mut ours = Comline2Handshake::new(params, Hello::build(&header, &[zero()], &[one()]));
            let mut theirs = Comline2Handshake::new(params, Hello::build(&header, &[zero(), one(), two()], &[]));
            let (client, server) = tokio::join!(
                AsyncConnection::connect_with(dialler, MsgPack, Datagram2Framing, &mut ours, IR_HASH),
                AsyncConnection::accept_with(acceptor, MsgPack, Datagram2Framing, &mut theirs, IR_HASH),
            );
            let (mut client, mut server) = (client.unwrap(), server.unwrap());
            assert_eq!(provide_all(&mut client), [OK, OK, NO, NO], "dialling, {exchange:?}");
            assert_eq!(provide_all(&mut server), [OK, OK, NO, NO], "accepting, {exchange:?}");
        }
    }

    #[tokio::test]
    async fn without_a_hello_nothing_is_checked() {
        let (a, _b) = async_duplex();
        let mut conn = AsyncConnection::new_unverified(a, MsgPack);
        assert_eq!(provide_all(&mut conn), [OK, OK, OK, OK]);

        let (dialler, acceptor) = async_duplex();
        let (client, server) = tokio::join!(
            AsyncConnection::connect(dialler, MsgPack, IR_HASH),
            AsyncConnection::accept(acceptor, MsgPack, IR_HASH),
        );
        assert_eq!(provide_all(&mut client.unwrap()), [OK, OK, OK, OK]);
        assert_eq!(provide_all(&mut server.unwrap()), [OK, OK, OK, OK]);
    }
}
