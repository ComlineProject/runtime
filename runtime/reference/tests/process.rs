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

use comline_runtime_conformance::{Case, Expect, Op, Vectors};

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
}

/// The requests a case makes, with the replies `run` in the conformance
/// crate would accept: encoding gives `bytes` exactly (except for
/// decode-only cases), decoding `bytes` gives the input back (except for
/// encode-only ops), and a rejection's bytes are refused.
fn expectations(case: &Case) -> Vec<Expected> {
    let id = |what: &str| format!("{}/{what}", case.id);
    let bytes = serde_json::to_value(&case.bytes).unwrap();
    let mut out = Vec::new();
    if case.expect == Expect::Reject {
        out.push(Expected {
            request: json!({ "id": id("reject"), "op": case.op, "input": case.input, "bytes": bytes }),
            reply: json!({ "id": id("reject"), "accept": false }),
        });
        return out;
    }
    if case.op != Op::Decode {
        out.push(Expected {
            request: json!({ "id": id("encode"), "op": case.op, "input": case.input }),
            reply: json!({ "id": id("encode"), "bytes": bytes }),
        });
    }
    if !matches!(case.op, Op::NameHash | Op::Encode) {
        out.push(Expected {
            request: json!({ "id": id("decode"), "op": case.op, "input": case.input, "bytes": bytes }),
            reply: json!({ "id": id("decode"), "accept": true, "input": case.input }),
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
            .filter(|(e, got)| e.reply != **got)
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
