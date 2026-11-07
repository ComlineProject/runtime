// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Comparing JSON texts as values, keeping numbers exact.

/// Whether two JSON texts are the same JSON value, for a case that compares
/// as JSON whose bytes aren't the reference's own spelling: whitespace and
/// member order don't matter, strings compare by the text they hold, and
/// numbers compare as exact decimals (`1` equals `1.0` and `10e-1`, but
/// `9007199254740992` doesn't equal `9007199254740993`).
pub fn json_equal(a: &[u8], b: &[u8]) -> bool {
    // serde_json decides what is valid JSON (and caps the depth); the tree
    // below only exists because it can't keep a number's exact digits.
    if serde_json::from_slice::<serde::de::IgnoredAny>(a).is_err()
        || serde_json::from_slice::<serde::de::IgnoredAny>(b).is_err()
    {
        return false;
    }
    match (exact_json(a), exact_json(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// A JSON value whose numbers keep their exact decimal value.
#[derive(PartialEq)]
enum Exact {
    Null,
    Bool(bool),
    /// Normalised: sign, significant digits without a leading or trailing
    /// zero, and the power of ten they are scaled by. Zero is `(false, "", 0)`.
    Number(bool, String, i64),
    String(String),
    Array(Vec<Exact>),
    Object(std::collections::BTreeMap<String, Exact>),
}

/// Parses JSON text already known to be valid; `None` on a number whose
/// exponent doesn't fit in an `i64`.
fn exact_json(text: &[u8]) -> Option<Exact> {
    let mut at = 0;
    let value = exact_value(text, &mut at)?;
    Some(value)
}

fn exact_skip(text: &[u8], at: &mut usize) {
    while matches!(text.get(*at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        *at += 1;
    }
}

fn exact_string(text: &[u8], at: &mut usize) -> Option<String> {
    let start = *at;
    *at += 1;
    while *text.get(*at)? != b'"' {
        *at += if text[*at] == b'\\' { 2 } else { 1 };
    }
    *at += 1;
    serde_json::from_slice(&text[start..*at]).ok()
}

fn exact_value(text: &[u8], at: &mut usize) -> Option<Exact> {
    exact_skip(text, at);
    let value = match *text.get(*at)? {
        b'n' => {
            *at += 4;
            Exact::Null
        }
        b't' => {
            *at += 4;
            Exact::Bool(true)
        }
        b'f' => {
            *at += 5;
            Exact::Bool(false)
        }
        b'"' => Exact::String(exact_string(text, at)?),
        b'[' => {
            *at += 1;
            let mut items = Vec::new();
            loop {
                exact_skip(text, at);
                if text[*at] == b']' {
                    *at += 1;
                    break;
                }
                if text[*at] == b',' {
                    *at += 1;
                }
                items.push(exact_value(text, at)?);
            }
            Exact::Array(items)
        }
        b'{' => {
            *at += 1;
            let mut members = std::collections::BTreeMap::new();
            loop {
                exact_skip(text, at);
                match text[*at] {
                    b'}' => {
                        *at += 1;
                        break;
                    }
                    b',' => {
                        *at += 1;
                        continue;
                    }
                    _ => {}
                }
                let key = exact_string(text, at)?;
                exact_skip(text, at);
                *at += 1; // ':'
                members.insert(key, exact_value(text, at)?);
            }
            Exact::Object(members)
        }
        _ => {
            let start = *at;
            while matches!(
                text.get(*at),
                Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E')
            ) {
                *at += 1;
            }
            exact_number(std::str::from_utf8(&text[start..*at]).ok()?)?
        }
    };
    Some(value)
}

fn exact_number(number: &str) -> Option<Exact> {
    let (negative, number) = match number.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, number),
    };
    let (mantissa, exponent) = match number.find(['e', 'E']) {
        Some(at) => (&number[..at], number[at + 1..].parse::<i64>().ok()?),
        None => (number, 0),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = format!("{whole}{fraction}");
    let mut scale = exponent.checked_sub(fraction.len() as i64)?;
    let digits = digits.trim_start_matches('0');
    let trimmed = digits.trim_end_matches('0');
    scale = scale.checked_add((digits.len() - trimmed.len()) as i64)?;
    if trimmed.is_empty() {
        return Some(Exact::Number(false, String::new(), 0));
    }
    Some(Exact::Number(negative, trimmed.to_string(), scale))
}

#[cfg(test)]
mod tests {
    use super::json_equal;

    fn same(a: &str, b: &str) -> bool {
        let forward = json_equal(a.as_bytes(), b.as_bytes());
        assert_eq!(forward, json_equal(b.as_bytes(), a.as_bytes()), "{a} / {b}");
        forward
    }

    #[test]
    fn numbers_compare_as_exact_decimals() {
        for (a, b) in [
            ("1", "1.0"),
            ("1", "10e-1"),
            ("1", "0.1e1"),
            ("1", "1E0"),
            ("100", "1e2"),
            ("0", "-0"),
            ("0", "0.000"),
            ("-1.50", "-15e-1"),
            ("9007199254740993", "9007199254740993.0"),
            ("[1, {\"a\": 2.0}]", "[1.0,{\"a\":2}]"),
        ] {
            assert!(same(a, b), "{a} should equal {b}");
        }
    }

    #[test]
    fn different_numbers_differ() {
        for (a, b) in [
            ("1", "2"),
            ("1", "-1"),
            ("1", "1.0001"),
            ("1", "10"),
            ("9007199254740992", "9007199254740993"),
            ("0.1", "0.10000000000000001"),
            ("1e400", "1e401"),
        ] {
            assert!(!same(a, b), "{a} should differ from {b}");
        }
    }

    #[test]
    fn structure_and_strings() {
        assert!(same(
            r#"{"a":1,"b":[true,null]}"#,
            r#" { "b" : [ true , null ] , "a" : 1.0 } "#
        ));
        assert!(same(r#""app.Chat\/count""#, r#""app.Chat/count""#));
        assert!(same(r#""é""#, "\"é\""));
        assert!(!same(r#"{"a":1}"#, r#"{"a":1,"b":1}"#));
        assert!(!same("[1,2]", "[2,1]"));
        assert!(!same("true", "\"true\""));
        assert!(!same("1", "\"1\""));
    }

    #[test]
    fn invalid_json_equals_nothing() {
        assert!(!json_equal(b"1.", b"1.0"));
        assert!(!json_equal(b"{", b"{"));
        assert!(!json_equal(b"", b""));
    }
}
