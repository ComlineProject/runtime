// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! `comline-reference`: the Rust runtime answering encode and decode
//! questions over a line protocol, so a runtime in another language can be
//! checked against `comline-runtime` (a fuzzer, say).
//!
//! Each input line is one request object, or a JSON array of them (a batch).
//! Each gets one reply line of the same shape. Requests use the conformance
//! corpus's own encoding: any vector case's `op`, `input` and `bytes` make a
//! valid request. See README.md for the protocol.

use std::io::{self, BufRead, Write};

use serde::Deserialize;
use serde_json::{json, Map, Value as Json};

use comline_runtime::contract::{
    FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED, FRAMING_DATAGRAM, FRAMING_DATAGRAM2,
    FRAMING_JSONRPC,
};
use comline_runtime_conformance::{answer, check, Answer, Hex, Op, Type, Value};

/// The protocol's version. Ops are only ever added under it; a change to an
/// existing op's request or reply is a new version.
pub const PROTOCOL: u32 = 1;

/// The corpus's ops, as requests name them.
pub const CORPUS_OPS: &[&str] = &[
    "name_hash",
    "handshake",
    "envelope",
    "request",
    "response",
    "encode",
    "decode",
];

/// Every op this reference answers: the corpus's, then its own.
pub fn ops() -> Vec<&'static str> {
    CORPUS_OPS
        .iter()
        .copied()
        .chain(["check", "version"])
        .collect()
}

pub const FORMATS: &[&str] = &[FORMAT_JSON, FORMAT_MSGPACK, FORMAT_PACKED];
pub const FRAMINGS: &[&str] = &[FRAMING_DATAGRAM, FRAMING_DATAGRAM2, FRAMING_JSONRPC];

/// Answers one line: a request object, or an array of them. The reply is one
/// line's JSON, without the newline. A malformed line gets an `error` reply.
pub fn handle_line(line: &str) -> String {
    let reply = match serde_json::from_str::<Json>(line) {
        Ok(Json::Array(requests)) => Json::Array(requests.iter().map(handle).collect()),
        Ok(request) => handle(&request),
        Err(e) => error(Json::Null, format!("not JSON: {e}")),
    };
    reply.to_string()
}

/// Answers one request.
pub fn handle(request: &Json) -> Json {
    let Some(fields) = request.as_object() else {
        return error(Json::Null, "a request is a JSON object".into());
    };
    let id = fields.get("id").cloned().unwrap_or(Json::Null);
    match respond(fields) {
        Ok(mut reply) => {
            reply.insert("id".into(), id);
            Json::Object(reply)
        }
        Err(why) => error(id, why),
    }
}

fn respond(fields: &Map<String, Json>) -> Result<Map<String, Json>, String> {
    let op = match fields.get("op") {
        Some(Json::String(op)) => op.as_str(),
        Some(_) => return Err("`op` is a string".into()),
        None => return Err("no `op`".into()),
    };
    let input = fields.get("input").unwrap_or(&Json::Null);
    let bytes = match fields.get("bytes") {
        None | Some(Json::Null) => None,
        Some(hex) => Some(
            Hex::deserialize(hex)
                .map_err(|e| format!("`bytes` isn't lowercase hex: {e}"))?
                .0,
        ),
    };

    let reply = match op {
        "version" => json!({
            "version": {
                "protocol": PROTOCOL,
                "crate": env!("CARGO_PKG_VERSION"),
                "ops": ops(),
                "formats": FORMATS,
                "framings": FRAMINGS,
            }
        }),
        "check" => {
            if bytes.is_some() {
                return Err("`check` takes no `bytes`".into());
            }
            let ty = Type::deserialize(&input["type"]).map_err(|e| format!("bad `type`: {e}"))?;
            let value =
                Value::deserialize(&input["value"]).map_err(|e| format!("bad `value`: {e}"))?;
            match check(&value, &ty) {
                Ok(()) => json!({ "ok": true }),
                Err(why) => json!({ "ok": false, "why": why }),
            }
        }
        name => {
            let op = Op::deserialize(&Json::String(name.into()))
                .map_err(|_| format!("unknown op `{op}`"))?;
            // `answer` reports a decode it can't do as a refusal; a fuzzer
            // would count that as a real reject.
            if bytes.is_some() && matches!(op, Op::NameHash | Op::Encode) {
                return Err(format!("{name} is encode-only: send it without bytes"));
            }
            match answer(op, input, bytes.as_deref()) {
                Answer::Bytes(bytes) => json!({ "bytes": Hex(bytes) }),
                Answer::Accept(input) => json!({ "accept": true, "input": input }),
                Answer::Reject => json!({ "accept": false }),
                Answer::NotImplemented(what) => json!({ "not_implemented": what }),
                Answer::Bad(why) => return Err(why),
            }
        }
    };
    match reply {
        Json::Object(reply) => Ok(reply),
        _ => unreachable!("every reply is an object"),
    }
}

fn error(id: Json, why: String) -> Json {
    json!({ "id": id, "error": why })
}

/// Serves requests from `input` until it ends, a reply line per request
/// line, flushed after each. A line that isn't UTF-8 gets an `error` reply.
pub fn serve<R: BufRead, W: Write>(mut input: R, mut output: W) -> io::Result<()> {
    let mut line = Vec::new();
    loop {
        line.clear();
        if input.read_until(b'\n', &mut line)? == 0 {
            return Ok(());
        }
        let text = line.strip_suffix(b"\n").unwrap_or(&line);
        let text = text.strip_suffix(b"\r").unwrap_or(text);
        let reply = match std::str::from_utf8(text) {
            Ok(text) => handle_line(text),
            Err(e) => error(Json::Null, format!("not UTF-8: {e}")).to_string(),
        };
        output.write_all(reply.as_bytes())?;
        output.write_all(b"\n")?;
        output.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(line: &str) -> Json {
        serde_json::from_str(&handle_line(line)).unwrap()
    }

    #[test]
    fn encodes_without_bytes() {
        let got = reply(r#"{"id": 1, "op": "name_hash", "input": {"name": "x"}}"#);
        assert_eq!(got["id"], 1);
        assert!(got["bytes"].is_string(), "{got}");
    }

    #[test]
    fn decodes_with_bytes() {
        let input = json!({ "kind": "err", "id": 258, "body": "6669656c6473" });
        let request = json!({ "id": "e", "op": "envelope", "input": input });
        let bytes = reply(&request.to_string())["bytes"].clone();

        let mut request = request;
        request["bytes"] = bytes;
        let got = reply(&request.to_string());
        assert_eq!(got, json!({ "id": "e", "accept": true, "input": input }));
    }

    #[test]
    fn rejects() {
        let got =
            reply(r#"{"id": 2, "op": "request", "input": {"framing": "datagram@1"}, "bytes": ""}"#);
        assert_eq!(got, json!({ "id": 2, "accept": false }));
    }

    #[test]
    fn batch_keeps_order() {
        let got = reply(
            r#"[{"id": "a", "op": "version"}, {"id": "b", "op": "nope"}, 3, {"id": "d", "op": "name_hash", "input": {"name": "x"}}]"#,
        );
        let got = got.as_array().unwrap();
        assert_eq!(got.len(), 4);
        assert!(got[0]["version"].is_object());
        assert_eq!(got[1]["id"], "b");
        assert!(got[1]["error"].is_string());
        assert_eq!(got[2]["id"], Json::Null);
        assert!(got[2]["error"].is_string());
        assert_eq!(got[3]["id"], "d");
        assert!(got[3]["bytes"].is_string());
    }

    #[test]
    fn malformed() {
        for line in ["{", "", "nonsense", r#"{"id": 1}"#, r#"{"op": 5}"#] {
            let got = reply(line);
            assert!(got["error"].is_string(), "{line:?} → {got}");
        }
        assert_eq!(reply("{")["id"], Json::Null);
        assert_eq!(reply(r#"{"id": 7, "op": "encode"}"#)["id"], 7);
        assert!(reply(r#"{"id": 7, "op": "encode"}"#)["error"].is_string());
        assert!(
            reply(r#"{"id": 7, "op": "decode", "input": {}, "bytes": "zz"}"#)["error"].is_string()
        );
    }

    #[test]
    fn encode_only_ops_refuse_bytes() {
        for (op, input) in [
            ("name_hash", json!({ "name": "x" })),
            (
                "encode",
                json!({ "format": "json@1", "value": { "u8": 1 } }),
            ),
        ] {
            let got =
                reply(&json!({ "id": 9, "op": op, "input": input, "bytes": "00" }).to_string());
            assert_eq!(
                got,
                json!({ "id": 9, "error": format!("{op} is encode-only: send it without bytes") })
            );
        }
    }

    #[test]
    fn not_implemented() {
        let got = reply(
            r#"{"id": 1, "op": "encode", "input": {"format": "nope@1", "value": {"u8": 1}}}"#,
        );
        assert!(got["not_implemented"].is_string(), "{got}");
    }

    #[test]
    fn version() {
        let got = reply(r#"{"id": null, "op": "version"}"#);
        let version = &got["version"];
        assert_eq!(version["protocol"], PROTOCOL);
        assert_eq!(version["crate"], env!("CARGO_PKG_VERSION"));
        assert_eq!(version["ops"], json!(ops()));
        assert_eq!(
            version["formats"],
            json!(["json@1", "msgpack@1", "packed@1"])
        );
        assert_eq!(
            version["framings"],
            json!(["datagram@1", "datagram@2", "jsonrpc@1"])
        );
    }

    #[test]
    fn checks() {
        let ty = json!({ "struct": { "name": "P", "fields": [["x", "i32"]] } });
        let good = json!({ "struct": { "name": "P", "fields": [["x", { "i32": 1 }]] } });
        let bad = json!({ "struct": { "name": "P", "fields": [["x", { "u8": 1 }]] } });
        let ask = |value: &Json| {
            reply(
                &json!({ "id": 0, "op": "check", "input": { "type": ty, "value": value } })
                    .to_string(),
            )
        };
        assert_eq!(ask(&good), json!({ "id": 0, "ok": true }));
        let got = ask(&bad);
        assert_eq!(got["ok"], false);
        assert!(got["why"].is_string());
        assert!(ask(&json!("no"))["error"].is_string());
    }

    #[test]
    fn serve_answers_each_line() {
        let input = b"{\"id\":1,\"op\":\"version\"}\n\xff\n[]\r\n{\"id\":2,\"op\":\"version\"}";
        let mut out = Vec::new();
        serve(&input[..], &mut out).unwrap();
        let lines: Vec<Json> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0]["id"], 1);
        assert!(lines[1]["error"].is_string());
        assert_eq!(lines[2], json!([]));
        assert_eq!(lines[3]["id"], 2);
    }
}
