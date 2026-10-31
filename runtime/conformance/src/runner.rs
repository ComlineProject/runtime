// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The conformance runner as a library: load a corpus, ask any
//! implementation the questions its cases pose, and judge the answers.
//!
//! An implementation is anything that answers the ops protocol's questions
//! (see [`Implementation`]): `comline-runtime` in process ([`InProcess`]), or
//! a process speaking the protocol on stdin and stdout ([`Lines`]), such as
//! `comline-reference` or a third party's own encoding. The questions are
//! fixed by the cases alone ([`questions`]), never by earlier answers, so a
//! caller can send all of them in one go and judge afterwards ([`judge`]):
//! `comline conformance` does, to run an untrusted process once, with a
//! time limit and an output cap.
//!
//! The case format and the ops protocol are a public spec, versioned
//! [`SPEC_VERSION`]; see the crate's README.md, "The spec".
//!
//! ```no_run
//! use comline_runtime_conformance::runner::{load_corpus, run_all, InProcess};
//!
//! let corpus = load_corpus("runtime/conformance".as_ref()).unwrap();
//! let profile = &corpus.profiles["packed@1"];
//! let cases = corpus.cases_for(profile);
//! let outcomes = run_all(&cases, &mut InProcess).unwrap();
//! for proof in corpus.prove(profile, &outcomes) {
//!     println!("{}: {} {:?}", proof.fact, proof.value, proof.result);
//! }
//! ```

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::io::{BufRead, Write};
use std::path::Path;

use serde::Deserialize;
use serde_json::{json, Value as Json};

use crate::{answer, fact_shown, json_equal, Answer, Case, Compare, Expect, Hex, Op, Outcome};
use crate::{Profile, Vectors};

/// The version of the public spec: the case format (`vectors/*.json`,
/// `profiles/*.json`) and the ops protocol. `0.x` while Comline is in its
/// early days: a minor bump may change anything, and says so in the
/// changelog. The ops protocol also carries its own number (`protocol` in
/// the `version` reply, [`OPS_PROTOCOL`]).
pub const SPEC_VERSION: &str = "0.2";

/// The ops protocol's number, as a `version` reply states it. Ops and reply
/// fields are only ever added under it.
pub const OPS_PROTOCOL: u64 = 1;

// --- Loading ---

/// A corpus on disk: `vectors/*.json` (each a [`Vectors`]) and
/// `profiles/*.json` (each a [`Profile`], by its `name`). Either directory
/// may be missing.
#[derive(Debug, Clone, Default)]
pub struct Corpus {
    pub vectors: Vec<Vectors>,
    pub profiles: BTreeMap<String, Profile>,
}

/// Loads the corpus under `dir`, as the runtime's own
/// (`runtime/conformance`) is laid out.
pub fn load_corpus(dir: &Path) -> Result<Corpus, String> {
    let mut corpus = Corpus::default();
    for path in json_files(&dir.join("vectors"))? {
        corpus.vectors.push(load_vectors(&path)?);
    }
    for path in json_files(&dir.join("profiles"))? {
        let profile = load_profile(&path)?;
        corpus.profiles.insert(profile.name.clone(), profile);
    }
    Ok(corpus)
}

fn json_files(dir: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(format!("{}: {e}", dir.display())),
    };
    let mut paths: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    paths.sort();
    Ok(paths)
}

/// One vectors file: `{"feature": ..., "cases": [...]}`.
pub fn load_vectors(path: &Path) -> Result<Vectors, String> {
    read_json(path)
}

/// One profile file: `{"name": ..., "kind": ..., "facts": {...}}`.
pub fn load_profile(path: &Path) -> Result<Profile, String> {
    read_json(path)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let text = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::from_slice(&text).map_err(|e| format!("{}: {e}", path.display()))
}

impl Corpus {
    /// Every case, in file order.
    pub fn cases(&self) -> impl Iterator<Item = &Case> {
        self.vectors.iter().flat_map(|v| &v.cases)
    }

    /// The case with this id.
    pub fn case(&self, id: &str) -> Option<&Case> {
        self.cases().find(|c| c.id == id)
    }

    /// The cases of one feature (a vectors file's `feature`).
    pub fn feature(&self, feature: &str) -> Vec<&Case> {
        self.vectors
            .iter()
            .filter(|v| v.feature == feature)
            .flat_map(|v| &v.cases)
            .collect()
    }

    /// The cases that bear on an encoding: every case of each feature its
    /// profile's facts are proven from (a case id is `<feature>/<name>`).
    /// For the official `packed@1`, all of `format-packed`.
    pub fn cases_for(&self, profile: &Profile) -> Vec<&Case> {
        let mut features: Vec<&str> = profile
            .facts
            .values()
            .flat_map(|f| &f.proven_by)
            .filter_map(|id| id.split_once('/').map(|(feature, _)| feature))
            .collect();
        features.sort();
        features.dedup();
        features.into_iter().flat_map(|f| self.feature(f)).collect()
    }

    /// Whether each of `profile`'s facts is proven by `outcomes` (from
    /// [`run_all`] or [`judge`], by case id).
    pub fn prove(&self, profile: &Profile, outcomes: &BTreeMap<String, Outcome>) -> Vec<FactProof> {
        prove(profile, |id| self.case(id), outcomes)
    }
}

/// A profile fact and whether it's proven.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FactProof {
    pub fact: String,
    pub value: String,
    /// `Ok` when every case it names exists and passed, and they show the
    /// value; else why not.
    pub result: Result<(), String>,
}

/// Whether each of `profile`'s facts is proven: the cases it names exist
/// (`case` finds them), passed (`outcomes`), and show the stated value
/// ([`fact_shown`]). A fact naming no case isn't proven.
pub fn prove<'a>(
    profile: &Profile,
    case: impl Fn(&str) -> Option<&'a Case>,
    outcomes: &BTreeMap<String, Outcome>,
) -> Vec<FactProof> {
    profile
        .facts
        .iter()
        .map(|(fact, stated)| {
            let result = (|| {
                let mut proving = Vec::new();
                for id in &stated.proven_by {
                    let c = case(id).ok_or_else(|| format!("names unknown case `{id}`"))?;
                    match outcomes.get(id) {
                        Some(Outcome::Pass) => proving.push(c),
                        Some(Outcome::Fail(_)) => return Err(format!("case `{id}` failed")),
                        Some(Outcome::NotImplemented(what)) => {
                            return Err(format!("case `{id}` not implemented: {what}"))
                        }
                        None => return Err(format!("case `{id}` wasn't run")),
                    }
                }
                fact_shown(fact, &stated.value, &proving)
            })();
            FactProof {
                fact: fact.clone(),
                value: stated.value.clone(),
                result,
            }
        })
        .collect()
}

// --- Questions and answers ---

/// One question to an implementation: an op request of the ops protocol.
/// With no `bytes`, what `input` encodes to; with `bytes`, whether they
/// decode, and to what.
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    pub op: Op,
    pub input: Json,
    pub bytes: Option<Vec<u8>>,
}

impl Question {
    /// The request line's object, with `id`.
    pub fn request(&self, id: Json) -> Json {
        let mut request = json!({ "id": id, "op": self.op, "input": self.input });
        if let Some(bytes) = &self.bytes {
            request["bytes"] = json!(Hex(bytes.clone()));
        }
        request
    }
}

/// Reads one reply object of the ops protocol: its `id`, and the answer.
/// `bytes` is [`Answer::Bytes`]; `accept` is [`Answer::Accept`] or
/// [`Answer::Reject`]; `not_implemented` is [`Answer::NotImplemented`];
/// `error` is [`Answer::Bad`].
pub fn parse_reply(reply: &Json) -> Result<(Json, Answer), String> {
    let fields = reply.as_object().ok_or("a reply is a JSON object")?;
    let id = fields.get("id").cloned().unwrap_or(Json::Null);
    let answer = if let Some(bytes) = fields.get("bytes") {
        Answer::Bytes(
            Hex::deserialize(bytes)
                .map_err(|e| format!("`bytes` isn't lowercase hex: {e}"))?
                .0,
        )
    } else if let Some(accept) = fields.get("accept") {
        match accept {
            Json::Bool(true) => Answer::Accept(fields.get("input").cloned().unwrap_or(Json::Null)),
            Json::Bool(false) => Answer::Reject,
            _ => return Err("`accept` is a boolean".into()),
        }
    } else if let Some(what) = fields.get("not_implemented") {
        Answer::NotImplemented(what.as_str().unwrap_or_default().to_string())
    } else if let Some(why) = fields.get("error") {
        Answer::Bad(why.as_str().unwrap_or_default().to_string())
    } else {
        return Err(format!("a reply with no answer: {reply}"));
    };
    Ok((id, answer))
}

/// The questions a case poses, in order. [`judge`] takes their answers in
/// the same order.
///
/// - A rejection asks to decode `bytes`; a `stream_prefix` rejection also
///   asks to encode its input, which must fail too.
/// - Otherwise, every op but `decode` and `handshake_check` asks to encode
///   the input, and every
///   op but `name_hash` and `encode` asks to decode `bytes`.
pub fn questions(case: &Case) -> Vec<Question> {
    let ask = |bytes: Option<&[u8]>| Question {
        op: case.op,
        input: case.input.clone(),
        bytes: bytes.map(<[u8]>::to_vec),
    };
    let mut asked = Vec::new();
    if case.expect == Expect::Reject {
        if case.op == Op::StreamPrefix {
            asked.push(ask(None));
        }
        asked.push(ask(Some(&case.bytes.0)));
        return asked;
    }
    if !case.op.decode_only() {
        asked.push(ask(None));
    }
    if !matches!(case.op, Op::NameHash | Op::Encode) {
        asked.push(ask(Some(&case.bytes.0)));
    }
    asked
}

/// Judges a case by the answers to its [`questions`], in their order.
///
/// - An encode must give the case's bytes (as JSON when it compares as
///   JSON); a failure shows both with [`byte_diff`].
/// - A decode must accept and give back the case's input exactly, or, for a
///   rejection, refuse.
/// - A `stream_prefix` rejection's encode must not give bytes.
/// - Any `not_implemented` answer makes the case not implemented, and an
///   `error` answer fails it.
pub fn judge(case: &Case, answers: &[Answer]) -> Outcome {
    let asked = questions(case);
    if answers.len() != asked.len() {
        return Outcome::Fail(format!(
            "{} answers to {} questions",
            answers.len(),
            asked.len()
        ));
    }
    if let Some(what) = answers.iter().find_map(|a| match a {
        Answer::NotImplemented(what) => Some(what),
        _ => None,
    }) {
        return Outcome::NotImplemented(what.clone());
    }
    for (question, answer) in asked.iter().zip(answers) {
        let verdict = match (&question.bytes, answer) {
            (_, Answer::NotImplemented(_)) => unreachable!("handled above"),
            // A refused encode of an over-limit prefix: an error is right.
            (None, Answer::Bad(_)) if case.expect == Expect::Reject => Ok(()),
            (_, Answer::Bad(why)) => Err(format!("answered with an error: {why}")),
            (None, Answer::Bytes(got)) if case.expect == Expect::Reject => {
                Err(format!("prefixed a frame over the limit, as {}", hex(got)))
            }
            (None, Answer::Bytes(got)) => {
                let want = &case.bytes.0;
                if got == want || (case.compare == Compare::Json && json_equal(got, want)) {
                    Ok(())
                } else {
                    Err(format!("encodes wrong:\n{}", byte_diff(got, want)))
                }
            }
            (None, other) => Err(format!("answered an encode with {}", describe(other))),
            (Some(_), Answer::Reject) if case.expect == Expect::Reject => Ok(()),
            (Some(_), Answer::Reject) => Err("refused bytes it must decode".into()),
            (Some(_), Answer::Accept(got)) if case.expect == Expect::Reject => {
                Err(format!("accepted bytes it must reject, as {got}"))
            }
            (Some(_), Answer::Accept(got)) if *got == case.input => Ok(()),
            (Some(_), Answer::Accept(got)) => {
                Err(format!("decodes to {got}, expected {}", case.input))
            }
            (Some(_), Answer::Bytes(_)) => Err("answered a decode with bytes".into()),
        };
        if let Err(why) = verdict {
            return Outcome::Fail(why);
        }
    }
    Outcome::Pass
}

fn describe(answer: &Answer) -> &'static str {
    match answer {
        Answer::Bytes(_) => "bytes",
        Answer::Accept(_) => "an accept",
        Answer::Reject => "a refusal",
        Answer::NotImplemented(_) => "not_implemented",
        Answer::Bad(_) => "an error",
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// Two byte strings side by side, in hex, with the first difference marked:
///
/// ```text
///   expected: 0102016669656c6473
///        got: 0102006669656c6473
///                 ^^ byte 2 differs
/// ```
///
/// Long strings are cut to a window around the difference.
pub fn byte_diff(got: &[u8], want: &[u8]) -> String {
    const WINDOW: usize = 24;
    let first = got
        .iter()
        .zip(want)
        .position(|(a, b)| a != b)
        .unwrap_or(got.len().min(want.len()));
    let start = first.saturating_sub(WINDOW / 2);
    let cut = |b: &[u8]| {
        let end = b.len().min(start + WINDOW);
        let mut s = String::new();
        if start > 0 {
            s.push('…');
        }
        s.push_str(&hex(b.get(start..end).unwrap_or_default()));
        if end < b.len() {
            s.push('…');
        }
        s
    };
    let lead = if start > 0 { 1 } else { 0 };
    let marker = " ".repeat(12 + lead + 2 * (first - start));
    let what = if first < got.len().min(want.len()) {
        format!("byte {first} differs")
    } else {
        format!(
            "lengths differ ({} bytes, expected {})",
            got.len(),
            want.len()
        )
    };
    format!(
        "  expected: {}\n       got: {}\n{marker}^^ {what}",
        cut(want),
        cut(got)
    )
}

// --- Implementations ---

/// Something that answers the ops protocol's questions.
pub trait Implementation {
    /// Answers each question, in order. `Err` when the implementation
    /// can't answer at all (it died, or replied out of protocol).
    fn answer(&mut self, questions: &[Question]) -> Result<Vec<Answer>, String>;
}

/// `comline-runtime` itself, in process: the reference.
pub struct InProcess;

impl Implementation for InProcess {
    fn answer(&mut self, questions: &[Question]) -> Result<Vec<Answer>, String> {
        Ok(questions
            .iter()
            .map(|q| answer(q.op, &q.input, q.bytes.as_deref()))
            .collect())
    }
}

/// An implementation over the ops protocol's lines: requests written to
/// `input`, a reply line read from `output` for each. For a process, its
/// stdin and stdout. Each call sends its questions as one batch line.
pub struct Lines<W, R> {
    pub input: W,
    pub output: R,
    next_id: u64,
}

impl<W: Write, R: BufRead> Lines<W, R> {
    pub fn new(input: W, output: R) -> Self {
        Lines {
            input,
            output,
            next_id: 0,
        }
    }

    /// Asks `version`: the reply's `version` object (`protocol`, `ops`,
    /// `formats`, `framings`).
    pub fn version(&mut self) -> Result<Json, String> {
        let reply = self.round_trip(&json!({ "id": "version", "op": "version" }))?;
        reply
            .get("version")
            .cloned()
            .ok_or_else(|| format!("`version` got {reply}"))
    }

    fn round_trip(&mut self, request: &Json) -> Result<Json, String> {
        let io = |e: std::io::Error| format!("the implementation's pipe: {e}");
        writeln!(self.input, "{request}").map_err(io)?;
        self.input.flush().map_err(io)?;
        let mut line = String::new();
        if self.output.read_line(&mut line).map_err(io)? == 0 {
            return Err("the implementation closed its output".into());
        }
        serde_json::from_str(&line).map_err(|e| format!("a reply that isn't JSON ({e}): {line}"))
    }
}

impl<W: Write, R: BufRead> Implementation for Lines<W, R> {
    fn answer(&mut self, questions: &[Question]) -> Result<Vec<Answer>, String> {
        let first = self.next_id;
        self.next_id += questions.len() as u64;
        let batch: Vec<Json> = questions
            .iter()
            .enumerate()
            .map(|(i, q)| q.request(json!(first + i as u64)))
            .collect();
        let replies = self.round_trip(&Json::Array(batch))?;
        let replies = replies
            .as_array()
            .ok_or_else(|| format!("a batch got {replies}"))?;
        if replies.len() != questions.len() {
            return Err(format!(
                "a batch of {} got {} replies",
                questions.len(),
                replies.len()
            ));
        }
        replies
            .iter()
            .enumerate()
            .map(|(i, reply)| {
                let (id, answer) = parse_reply(reply)?;
                if id != json!(first + i as u64) {
                    return Err(format!("reply {i} has id {id}"));
                }
                Ok(answer)
            })
            .collect()
    }
}

/// Runs each case against `implementation`, one batch per case: every
/// outcome, by case id.
pub fn run_all(
    cases: &[&Case],
    implementation: &mut impl Implementation,
) -> Result<BTreeMap<String, Outcome>, String> {
    cases
        .iter()
        .map(|case| {
            let answers = implementation.answer(&questions(case))?;
            Ok((case.id.clone(), judge(case, &answers)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_marks_the_first_difference() {
        let diff = byte_diff(&[1, 2, 0, 3], &[1, 2, 1, 3]);
        assert_eq!(
            diff,
            "  expected: 01020103\n       got: 01020003\n                ^^ byte 2 differs"
        );
        assert!(byte_diff(&[1], &[1, 2]).contains("lengths differ"));
    }

    #[test]
    fn a_wrong_answer_fails_with_a_diff() {
        let case = Case {
            id: "x/y".into(),
            op: Op::NameHash,
            input: json!({ "name": "x" }),
            bytes: Hex(vec![1, 2]),
            expect: Expect::Exact,
            compare: Compare::Bytes,
        };
        assert_eq!(judge(&case, &[Answer::Bytes(vec![1, 2])]), Outcome::Pass);
        match judge(&case, &[Answer::Bytes(vec![1, 3])]) {
            Outcome::Fail(why) => assert!(why.contains("byte 1 differs"), "{why}"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            judge(&case, &[Answer::NotImplemented("x".into())]),
            Outcome::NotImplemented(_)
        ));
    }

    #[test]
    fn replies_parse() {
        assert_eq!(
            parse_reply(&json!({ "id": 1, "bytes": "00ff" })).unwrap(),
            (json!(1), Answer::Bytes(vec![0, 255]))
        );
        assert_eq!(
            parse_reply(&json!({ "id": 1, "accept": false })).unwrap().1,
            Answer::Reject
        );
        assert!(parse_reply(&json!({ "id": 1 })).is_err());
    }
}
