//! The method a decoded request names, as its frame spells it.

use core::fmt::{self, Write as _};
use core::marker::PhantomData;
use core::ptr::NonNull;

/// The method a decoded request names ([`RequestCall::Name`](crate::contract::RequestCall::Name)).
///
/// It borrows the frame. A framing that carries names as JSON strings
/// (JSON-RPC) can find one written with escapes, `"app.Chat\/count"` or
/// `"\u0067reet"` for instance, which any JSON writer may do. Rather than
/// copy the name out to unescape it, a `MethodName` keeps it as written and
/// unescapes as it's read: compare it with `==`, or read it with
/// [`chars`](Self::chars) or `Display`. [`as_str`](Self::as_str) borrows it
/// when it has no escapes, which is how every Comline runtime writes a
/// generated name.
///
/// It's the size of a `&str`, so a [`RequestCall`](crate::contract::RequestCall)
/// and every frame that holds one are no bigger than when a name was a
/// `&str`: whether the text is escaped is the top bit of its length, which a
/// `str`'s length never sets (no allocation exceeds `isize::MAX` bytes).
#[derive(Clone, Copy)]
pub struct MethodName<'f> {
    /// The text's first byte.
    ptr: NonNull<u8>,
    /// The text's length in bytes, with [`ESCAPED`] set when it's JSON
    /// string text with escapes in it.
    len: usize,
    text: PhantomData<&'f str>,
}

/// The bit of [`MethodName::len`] that says its text is escaped.
const ESCAPED: usize = 1 << (usize::BITS - 1);

// SAFETY: a `MethodName` is a `&'f str` and a flag, and `&str` is both.
unsafe impl Send for MethodName<'_> {}
unsafe impl Sync for MethodName<'_> {}

#[derive(Clone, Copy)]
enum Spelling<'f> {
    /// The name itself.
    Plain(&'f str),
    /// A valid JSON string's text between its quotes, with at least one
    /// escape in it.
    Json(&'f str),
}

impl<'f> MethodName<'f> {
    /// The name `name`, as it is.
    pub const fn new(name: &'f str) -> Self {
        Self::from_spelling(name, false)
    }

    const fn from_spelling(text: &'f str, escaped: bool) -> Self {
        // A `str` is never longer than `isize::MAX` bytes, so `ESCAPED` is
        // free.
        debug_assert!(text.len() & ESCAPED == 0);
        Self {
            // SAFETY: a reference's pointer is never null.
            ptr: unsafe { NonNull::new_unchecked(text.as_ptr().cast_mut()) },
            len: text.len() | if escaped { ESCAPED } else { 0 },
            text: PhantomData,
        }
    }

    fn spelling(&self) -> Spelling<'f> {
        let len = self.len & !ESCAPED;
        // SAFETY: `ptr` and `len` are a `&'f str`'s, as `from_spelling`
        // took them apart, and `PhantomData` holds the borrow for `'f`.
        let text = unsafe {
            core::str::from_utf8_unchecked(core::slice::from_raw_parts(self.ptr.as_ptr(), len))
        };
        if self.len & ESCAPED == 0 {
            Spelling::Plain(text)
        } else {
            Spelling::Json(text)
        }
    }

    /// The name a JSON string spells, from its text between the quotes as
    /// written (`app.Chat\/count`). `None` unless that text is a valid JSON
    /// string's (RFC 8259 §7) that names a Unicode string: no unescaped `"`,
    /// `\` or control character, only JSON's escapes, and every `\u`
    /// surrogate paired, as serde_json requires of a `String`.
    pub fn from_json(text: &'f str) -> Option<Self> {
        if !text.bytes().any(|b| b == b'\\' || b == b'"' || b < 0x20) {
            return Some(Self::new(text));
        }
        let mut rest = text;
        while unescape_one(&mut rest)?.is_some() {}
        Some(Self::from_spelling(text, true))
    }

    /// The name, borrowed from the frame, when the frame wrote it without
    /// escapes.
    pub fn as_str(&self) -> Option<&'f str> {
        match self.spelling() {
            Spelling::Plain(name) => Some(name),
            Spelling::Json(_) => None,
        }
    }

    /// The name as the frame wrote it, escapes and all, and whether it has
    /// any. For a caller that hands the text on as it is, such as a binding
    /// through the C ABI pointing into the frame, and unescapes it only if it
    /// must. A name made by [`new`](Self::new) has none.
    pub fn as_written(&self) -> (&'f str, bool) {
        match self.spelling() {
            Spelling::Plain(name) => (name, false),
            Spelling::Json(text) => (text, true),
        }
    }

    /// The name's characters, unescaped.
    pub fn chars(&self) -> Chars<'f> {
        match self.spelling() {
            Spelling::Plain(name) => Chars(CharsInner::Plain(name.chars())),
            Spelling::Json(text) => Chars(CharsInner::Json(text)),
        }
    }
}

impl<'f> From<&'f str> for MethodName<'f> {
    fn from(name: &'f str) -> Self {
        Self::new(name)
    }
}

impl PartialEq<str> for MethodName<'_> {
    fn eq(&self, other: &str) -> bool {
        match self.spelling() {
            Spelling::Plain(name) => name == other,
            Spelling::Json(_) => self.chars().eq(other.chars()),
        }
    }
}

impl PartialEq<&str> for MethodName<'_> {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

impl PartialEq<MethodName<'_>> for str {
    fn eq(&self, other: &MethodName<'_>) -> bool {
        *other == *self
    }
}

impl PartialEq<MethodName<'_>> for &str {
    fn eq(&self, other: &MethodName<'_>) -> bool {
        *other == **self
    }
}

#[cfg(feature = "alloc")]
impl PartialEq<MethodName<'_>> for alloc::string::String {
    fn eq(&self, other: &MethodName<'_>) -> bool {
        *other == **self
    }
}

impl PartialEq for MethodName<'_> {
    fn eq(&self, other: &Self) -> bool {
        match (self.spelling(), other.spelling()) {
            (Spelling::Plain(a), Spelling::Plain(b)) => a == b,
            _ => self.chars().eq(other.chars()),
        }
    }
}

impl Eq for MethodName<'_> {}

impl fmt::Display for MethodName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.spelling() {
            Spelling::Plain(name) => f.write_str(name),
            Spelling::Json(_) => self.chars().try_for_each(|c| f.write_char(c)),
        }
    }
}

impl fmt::Debug for MethodName<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_char('"')?;
        for c in self.chars() {
            for e in c.escape_debug() {
                f.write_char(e)?;
            }
        }
        f.write_char('"')
    }
}

/// A [`MethodName`]'s characters, unescaped.
#[derive(Clone)]
pub struct Chars<'f>(CharsInner<'f>);

#[derive(Clone)]
enum CharsInner<'f> {
    Plain(core::str::Chars<'f>),
    /// What's left of the escaped text, which `from_json` checked.
    Json(&'f str),
}

impl Iterator for Chars<'_> {
    type Item = char;

    fn next(&mut self) -> Option<char> {
        match &mut self.0 {
            CharsInner::Plain(chars) => chars.next(),
            // `from_json` checked the whole text, so this never fails.
            CharsInner::Json(rest) => unescape_one(rest).flatten(),
        }
    }
}

/// Reads the next character from a JSON string's text, unescaping it, and
/// moves `rest` past it: `Some(None)` at the end, `None` when the text isn't
/// a valid JSON string's.
fn unescape_one(rest: &mut &str) -> Option<Option<char>> {
    let mut chars = rest.chars();
    let Some(c) = chars.next() else {
        return Some(None);
    };
    let c = match c {
        '\\' => match chars.next()? {
            '"' => '"',
            '\\' => '\\',
            '/' => '/',
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            'u' => match hex4(&mut chars)? {
                high @ 0xD800..=0xDBFF => {
                    if chars.next()? != '\\' || chars.next()? != 'u' {
                        return None;
                    }
                    let low = hex4(&mut chars)?;
                    if !(0xDC00..=0xDFFF).contains(&low) {
                        return None;
                    }
                    char::from_u32(0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00))?
                }
                // A low surrogate with no high one before it.
                0xDC00..=0xDFFF => return None,
                code => char::from_u32(code)?,
            },
            _ => return None,
        },
        '"' | '\u{0}'..='\u{1f}' => return None,
        c => c,
    };
    *rest = chars.as_str();
    Some(Some(c))
}

/// Four hex digits, either case.
fn hex4(chars: &mut core::str::Chars<'_>) -> Option<u32> {
    let mut code = 0;
    for _ in 0..4 {
        code = code * 16 + chars.next()?.to_digit(16)?;
    }
    Some(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_no_bigger_than_a_str() {
        use core::mem::size_of;
        assert_eq!(size_of::<MethodName<'_>>(), size_of::<&str>());
        assert_eq!(
            size_of::<crate::contract::RequestCall<'_>>(),
            size_of::<&str>()
        );
    }

    #[test]
    fn as_written_is_the_frame_s_text_and_whether_it_has_escapes() {
        let frame = r#"app.Chat\/count"#;
        let escaped = MethodName::from_json(frame).unwrap();
        assert_eq!(escaped.as_written(), (frame, true));
        assert!(core::ptr::eq(escaped.as_written().0, frame));

        let plain = MethodName::from_json("greet").unwrap();
        assert_eq!(plain.as_written(), ("greet", false));
        assert_eq!(MethodName::new("a\\b").as_written(), ("a\\b", false));
    }

    #[test]
    fn plain_text_is_borrowed_as_it_is() {
        let name = MethodName::from_json("app.Chat/count").unwrap();
        assert_eq!(name.as_str(), Some("app.Chat/count"));
        assert_eq!(name, "app.Chat/count");
    }

    #[test]
    fn escaped_text_compares_and_reads_unescaped() {
        for (text, name) in [
            (r"app.Chat\/count", "app.Chat/count"),
            (r"\u0067reet", "greet"),
            (r"\u00E9\u00e9", "\u{e9}\u{e9}"),
            (r"\ud83d\ude00!", "\u{1f600}!"),
            (r#"\"\\\b\f\n\r\t"#, "\"\\\u{8}\u{c}\n\r\t"),
        ] {
            let read = MethodName::from_json(text).unwrap();
            assert_eq!(read.as_str(), None, "{text}");
            assert_eq!(read, name, "{text}");
            assert_eq!(read, MethodName::new(name), "{text}");
            assert!(read.chars().eq(name.chars()), "{text}");
        }
        let slash = MethodName::from_json(r"app.Chat\/count").unwrap();
        assert!("app.Chat/count" == slash);
        #[cfg(feature = "alloc")]
        {
            let owned = alloc::string::String::from("app.Chat/count");
            assert!(owned == slash);
        }
        assert_ne!(MethodName::from_json(r"gr\u0065et").unwrap(), "greets");
        assert_ne!(MethodName::from_json(r"gr\u0065et").unwrap(), "gree");
    }

    #[test]
    fn text_that_is_not_a_json_string_is_refused() {
        for text in [
            r"\",
            r"\x",
            r"\u12",
            r"\u12g4",
            r"\ud800",
            r"\ud800\u0041",
            r"\udc00",
            r"\ud800x",
            "a\"b",
            "a\u{1f}b",
        ] {
            assert!(MethodName::from_json(text).is_none(), "{text}");
        }
    }
}
