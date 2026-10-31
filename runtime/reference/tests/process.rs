// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The built `comline-reference` binary, driven as another language would
//! drive it: every conformance vector replayed through it, a batch per file.

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use serde_json::{json, Value as Json};

use comline_runtime_conformance::{json_equal, Case, Compare, Expect, Hex, Op, Vectors};

struct Reference {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Reference {
    fn spawn() -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_comline-reference"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .expect("spawn comline-reference");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Reference {
            child,
            stdin,
            stdout,
        }
    }

    /// Sends one line and reads its reply line.
    fn ask(&mut self, line: &str) -> Json {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
        let mut reply = String::new();
        assert!(self.stdout.read_line(&mut reply).unwrap() > 0, "no reply");
        serde_json::from_str(&reply).expect("a reply is JSON")
    }

    /// Closes stdin and requires a clean exit.
    fn finish(mut self) {
        drop(self.stdin.take());
        let mut rest = String::new();
        self.stdout.read_line(&mut rest).unwrap();
        assert_eq!(rest, "", "no reply without a request");
        assert!(self.child.wait().unwrap().success());
    }
}

/// What the reference must reply to a request about a case.
struct Expected {
    request: Json,
    reply: Json,
    /// Whether the reply's `bytes` may be any JSON text equal to the
    /// expected one's, as for a case that compares as JSON.
    json_bytes: bool,
}

impl Expected {
    fn matches(&self, got: &Json) -> bool {
        if !self.json_bytes || self.reply == *got {
            return self.reply == *got;
        }
        let hex = |reply: &Json| serde_json::from_value::<Hex>(reply["bytes"].clone()).ok();
        match (hex(&self.reply), hex(got)) {
            (Some(wanted), Some(got_bytes)) => {
                got["id"] == self.reply["id"]
                    && got.as_object().map(|o| o.len()) == Some(2)
                    && json_equal(&got_bytes.0, &wanted.0)
            }
            _ => false,
        }
    }
}

/// The requests a case makes, with the replies `run` in the conformance
/// crate would accept: encoding gives `bytes` exactly, or as JSON for a case
/// that compares as JSON (except for decode-only cases), decoding `bytes` gives the input back (except for
/// encode-only ops), and a rejection's bytes are refused.
fn expectations(case: &Case) -> Vec<Expected> {
    let id = |what: &str| format!("{}/{what}", case.id);
    let bytes = serde_json::to_value(&case.bytes).unwrap();
    let mut out = Vec::new();
    if case.expect == Expect::Reject {
        out.push(Expected {
            request: json!({ "id": id("reject"), "op": case.op, "input": case.input, "bytes": bytes }),
            reply: json!({ "id": id("reject"), "accept": false }),
            json_bytes: false,
        });
        return out;
    }
    if !case.op.decode_only() {
        out.push(Expected {
            request: json!({ "id": id("encode"), "op": case.op, "input": case.input }),
            reply: json!({ "id": id("encode"), "bytes": bytes }),
            json_bytes: case.compare == Compare::Json,
        });
    }
    if !matches!(case.op, Op::NameHash | Op::Encode) {
        out.push(Expected {
            request: json!({ "id": id("decode"), "op": case.op, "input": case.input, "bytes": bytes }),
            reply: json!({ "id": id("decode"), "accept": true, "input": case.input }),
            json_bytes: false,
        });
    }
    out
}

#[test]
fn replays_every_vector() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../conformance/vectors");
    let mut files: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no vectors in {}", dir.display());

    let mut reference = Reference::spawn();
    let mut asked = 0;
    for file in &files {
        let vectors: Vectors = serde_json::from_slice(&fs::read(file).unwrap()).unwrap();
        let expected: Vec<Expected> = vectors.cases.iter().flat_map(expectations).collect();
        assert!(!expected.is_empty(), "{}: no cases", file.display());

        let batch = Json::Array(expected.iter().map(|e| e.request.clone()).collect());
        let replies = reference.ask(&batch.to_string());
        let replies = replies.as_array().expect("a batch's reply is an array");
        assert_eq!(replies.len(), expected.len(), "{}", file.display());

        let wrong: Vec<String> = expected
            .iter()
            .zip(replies)
            .filter(|(e, got)| !e.matches(got))
            .map(|(e, got)| format!("  {}: got {got}, expected {}", e.request["id"], e.reply))
            .collect();
        assert!(
            wrong.is_empty(),
            "{}:\n{}",
            file.display(),
            wrong.join("\n")
        );
        asked += expected.len();
    }
    assert!(asked > 0);
    reference.finish();
}

#[test]
fn malformed_line_does_not_stop_it() {
    let mut reference = Reference::spawn();
    let bad = reference.ask("{not json");
    assert_eq!(bad["id"], Json::Null);
    assert!(bad["error"].is_string(), "{bad}");

    let bad = reference.ask(r#"{"id": 1, "op": "no_such_op"}"#);
    assert_eq!(bad["id"], 1);
    assert!(bad["error"].is_string(), "{bad}");

    let good = reference.ask(r#"{"id": 2, "op": "name_hash", "input": {"name": "x"}}"#);
    assert_eq!(good["id"], 2);
    assert!(good["bytes"].is_string(), "{good}");

    let version = reference.ask(r#"{"id": 3, "op": "version"}"#);
    assert_eq!(version["version"]["protocol"], 1);
    reference.finish();
}

/// A `session`: one side of a connection driven through calls, frames from
/// the peer, replies and abandons, each event answered by one step.
#[test]
fn session_runs_a_connection() {
    let mut reference = Reference::spawn();

    // `datagram@2` frames, by hand. A reply: kind 1, protocol, request id
    // (u64 LE), envelope (tag 0 ok, 1 err, 2 fault).
    let reply_ok = |id: u8, payload: &str| format!("0101{id:02x}0000000000000000{payload}");
    let events = json!([
        // Two nested calls and a one-way one: ids 0, 1, 2.
        { "call": { "protocol": 1, "call_id": 2, "method": "outer", "params": "aa" } },
        { "call": { "protocol": 1, "call_id": 3, "method": "inner", "params": "bb" } },
        { "call": { "protocol": 1, "call_id": 4, "method": "note", "one_way": true, "params": "" } },
        // A reply to the outer call skips the inner one.
        { "receive": reply_ok(0, "") },
        // The peer calls this side meanwhile (protocol 1, call 5, id 7),
        // and this side answers it with an error.
        { "receive": "000105000700000000000000cc" },
        { "reply": { "protocol": 1, "request_id": "7", "envelope": { "kind": "err", "id": 3, "body": "ee" } } },
        // The inner call's reply.
        { "receive": reply_ok(1, "11") },
        // The outer call times out; its late reply is stale.
        { "abandon": "0" },
        { "receive": reply_ok(0, "22") },
        // A reply to the one-way call, and to an id never handed out.
        { "receive": reply_ok(2, "") },
        { "receive": reply_ok(9, "") },
        // Not a frame.
        { "receive": "ff" },
        // An abandon of an id nobody waits on changes nothing.
        { "abandon": "42" },
        // The next call's id follows on.
        { "call": { "protocol": 0, "call_id": 1, "method": "again", "params": "" } },
    ]);
    let got = reference.ask(
        &json!({ "id": "s", "op": "session", "input": { "framing": "datagram@2", "ordered": true, "events": events } })
            .to_string(),
    );
    assert_eq!(
        got,
        json!({ "id": "s", "steps": [
            { "frame": "000102000000000000000000aa", "request_id": "0" },
            { "frame": "000103000100000000000000bb", "request_id": "1" },
            { "frame": "000104000200000000000000", "request_id": "2" },
            { "error": "framing" },
            { "event": { "request": { "protocol": 1, "call_id": 5, "request_id": "7", "one_way": false, "params": "cc" } } },
            { "frame": "01010700000000000000010300ee" },
            { "event": { "reply": { "request_id": "1", "envelope": { "kind": "ok", "payload": "11" } } } },
            {},
            { "event": "stale" },
            { "event": "stale" },
            { "event": "stale" },
            { "error": "framing" },
            {},
            { "frame": "000001000300000000000000", "request_id": "3" },
        ] })
    );

    // `jsonrpc@1`, unordered: replies in any order; a one-way call is a
    // notification; a request names its method, not a protocol.
    let call = |method: &str, one_way: bool| json!({ "call": { "protocol": 0, "call_id": 0, "method": method, "one_way": one_way, "params": hex("[1]") } });
    let events = json!([
        call("a", false),
        call("b", false),
        call("n", true),
        { "receive": hex(r#"{"jsonrpc":"2.0","result":"two","id":1}"#) },
        { "receive": hex(r#"{"jsonrpc":"2.0","error":{"code":-32603,"message":"Internal error"},"id":0}"#) },
        { "receive": hex(r#"{"jsonrpc":"2.0","result":null,"id":0}"#) },
        { "receive": hex(r#"{"jsonrpc":"2.0","method":"chat.Chat/send","params":{"x":1}}"#) },
        { "reply": { "protocol": 0, "request_id": "5", "envelope": { "kind": "ok", "payload": hex("true") } } },
    ]);
    let got = reference.ask(
        &json!({ "id": "j", "op": "session", "input": { "framing": "jsonrpc@1", "ordered": false, "events": events } })
            .to_string(),
    );
    assert_eq!(
        got,
        json!({ "id": "j", "steps": [
            { "frame": hex(r#"{"jsonrpc":"2.0","method":"a","params":[1],"id":0}"#), "request_id": "0" },
            { "frame": hex(r#"{"jsonrpc":"2.0","method":"b","params":[1],"id":1}"#), "request_id": "1" },
            { "frame": hex(r#"{"jsonrpc":"2.0","method":"n","params":[1]}"#), "request_id": "2" },
            { "event": { "reply": { "request_id": "1", "envelope": { "kind": "ok", "payload": hex("\"two\"") } } } },
            { "event": { "reply": { "request_id": "0", "envelope": { "kind": "fault" } } } },
            { "event": "stale" },
            { "event": { "request": { "method": "chat.Chat/send", "request_id": "0", "one_way": true, "params": hex(r#"{"x":1}"#) } } },
            { "frame": hex(r#"{"jsonrpc":"2.0","result":true,"id":5}"#) },
        ] })
    );

    // A framing a connection can't use, and a malformed event.
    let got = reference.ask(
        r#"{"id": 1, "op": "session", "input": {"framing": "datagram@1", "ordered": true, "events": []}}"#,
    );
    assert!(got["not_implemented"].is_string(), "{got}");
    let got = reference.ask(
        r#"{"id": 2, "op": "session", "input": {"framing": "datagram@2", "ordered": true, "events": [{"receive": "zz"}]}}"#,
    );
    assert!(got["error"].is_string(), "{got}");
    reference.finish();
}

fn hex(text: &str) -> String {
    text.bytes().map(|b| format!("{b:02x}")).collect()
}
