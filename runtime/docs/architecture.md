# Runtime architecture

How `comline-runtime` (`runtime/core`) is put together. The diagrams are
Mermaid, which GitHub renders in place.

## Layers

A call goes through four independent axes, each a trait with its own
implementations. Generated code picks one of each, as the schema declares.

```mermaid
flowchart TB
    gen["Generated code<br/>stubs and Dispatch impls"]

    subgraph drivers["Drivers"]
        direction LR
        lockstep["Client / Server<br/>lockstep: one asks, one answers"]
        conn["Connection<br/>blocking, calls both ways"]
        aconn["AsyncConnection<br/>any executor, calls both ways"]
    end

    state["connection::State<br/>sans-IO: request ids, waiting calls,<br/>frame to Event"]

    subgraph axes["Axes"]
        direction LR
        hs["Handshake<br/>comline@1, comline@2, none"]
        framing["Framing / ConnectionFraming<br/>datagram@1/@2/@3, jsonrpc@1"]
        format["WireFormat<br/>packed@1, json@1, msgpack@1"]
    end

    transport["Transport / AsyncTransport<br/>in-memory, TCP, tokio TCP"]

    gen --> drivers
    conn --> state
    aconn --> state
    drivers --> hs
    drivers --> framing
    framing --> format
    drivers --> transport
```

- **`WireFormat`** (`contract`, implementations in `format`) serialises a
  call's parts: params, results, errors. `packed@1` needs neither `std` nor
  `alloc`.
- **Framing** wraps the parts into a frame. `Framing` is the lockstep shape
  that `Client` and `Server` use. `ConnectionFraming` frames say whether
  they are a request or a reply, so either end can call: `datagram@2`,
  `datagram@3`, and `jsonrpc@1` (which names only its method).
- **`Transport`** moves whole frames: `transport::{InMemory, Tcp}`, and with
  `async`, `AsyncTransport` (`AsyncInMemory`, `transport::tokio::Tcp`).
- **`Handshake`** runs once when a connection opens: `comline@1` (the same
  IR on both ends), `comline@2` (compatibility by lines and shape hashes,
  `handshake::comline2`), or `none`.
- **`connection::State`** is what both connection drivers share. It does
  no I/O: it encodes requests and replies, and turns a received frame into
  an `Event` (`Request`, `Reply` or `Stale`). The C ABI
  (`comline-runtime-ffi`) is built on it too.

## One call over a connection

```mermaid
sequenceDiagram
    autonumber
    participant App as Caller
    participant A as Connection (A)
    participant SA as State (A)
    participant T as Transport
    participant SB as State (B)
    participant B as Connection (B)
    participant D as Dispatch (B)

    Note over A,B: connect / accept: the Handshake runs first
    App->>A: call(protocol, call, params)
    A->>SA: encode_request: ConnectionFraming + WireFormat
    SA-->>A: frame, request id now waiting
    A->>T: send(frame)
    T->>B: recv
    B->>SB: receive(frame)
    SB-->>B: Event::Request
    B->>D: dispatch(call, params)
    D-->>B: Reply (ok, err or fault)
    B->>SB: encode_reply
    B->>T: send(reply)
    T->>A: recv
    A->>SA: receive(reply)
    SA-->>A: Event::Reply (or Stale after abandon)
    A-->>App: Envelope, decoded by the stub
```

While A waits for its reply, it keeps serving B's requests, so calls nest:
a handler can call back the peer that called it.

## Features

```mermaid
flowchart LR
    none["no default features<br/>contract, packed@1"] --> alloc["alloc<br/>Client, Server, Connection,<br/>handshakes, validation"]
    alloc --> jsonrpc["jsonrpc<br/>jsonrpc@1 framing"]
    jsonrpc --> std["std (default)<br/>json@1, msgpack@1,<br/>in-memory and TCP transports"]
    std --> async["async<br/>AsyncConnection, AsyncTransport"]
    async --> tokio["tokio<br/>transport::tokio"]
```
