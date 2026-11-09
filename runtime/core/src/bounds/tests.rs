use super::*;

#[test]
fn a_message_reads_back() {
    for measure in [Measure::Chars, Measure::Items, Measure::Depth] {
        let b = BoundExceeded::new(measure, 32);
        let mut head = MessageHead::default();
        fmt::write(&mut head, format_args!("{b} at line 1 column 9")).unwrap();
        assert_eq!(MessageHead::of(&b), Some(b));
        assert_eq!(
            BoundExceeded::from_message(core::str::from_utf8(&head.buf[..head.len]).unwrap()),
            Some(b)
        );
    }
    assert_eq!(BoundExceeded::from_message("more than x items"), None);
    assert_eq!(BoundExceeded::from_message("invalid type"), None);
}

#[test]
fn characters_not_bytes() {
    assert!(check_chars("abcd", 4).is_ok());
    assert!(check_chars("abcde", 4).is_err());
    // 4 characters, 16 bytes: the most 4 characters can take.
    assert!(check_chars("😀😀😀😀", 4).is_ok());
    assert!(check_chars("😀😀😀😀a", 4).is_err());
    assert!(check_chars("ééééé", 4).is_err());
    assert!(check_chars("", 0).is_ok());
    assert!(check_chars("a", 0).is_err());
}

#[test]
fn utf8_is_checked_and_counted() {
    assert_eq!(chars_from_utf8("é€😀".as_bytes(), 3), Ok("é€😀"));
    assert_eq!(
        chars_from_utf8(&[b'a'; 13], 3),
        Err(RuntimeError::BoundExceeded {
            measure: Measure::Chars
        })
    );
    assert_eq!(
        chars_from_utf8(&[0xff], 3),
        Err(RuntimeError::Serialization)
    );
}

#[test]
fn a_single_char_is_strict_utf8() {
    assert_eq!(char_from_utf8(b"a"), Ok(('a', 1)));
    assert_eq!(char_from_utf8("é".as_bytes()), Ok(('é', 2)));
    assert_eq!(char_from_utf8("€".as_bytes()), Ok(('€', 3)));
    assert_eq!(char_from_utf8("😀x".as_bytes()), Ok(('😀', 4)));
    for bad in [
        &[][..],
        &[0x80],                   // a continuation byte
        &[0xc3],                   // truncated
        &[0xc0, 0x80],             // overlong
        &[0xed, 0xa0, 0x80],       // a surrogate
        &[0xf4, 0x90, 0x80, 0x80], // past U+10FFFF
        &[0xff],
    ] {
        assert_eq!(
            char_from_utf8(bad),
            Err(RuntimeError::Serialization),
            "{bad:x?}"
        );
    }
}

#[test]
fn depth_counts_levels() {
    let mut depth = Depth::new(2);
    assert!(depth.enter().is_ok());
    assert!(depth.enter().is_ok());
    assert_eq!(depth.enter(), Err(BoundExceeded::new(Measure::Depth, 2)));
    depth.leave();
    assert!(depth.enter().is_ok());
}

/// Every format reports a bound as `BoundExceeded`, on decode and encode.
#[cfg(feature = "std")]
#[test]
fn every_format_reports_the_bound() {
    use crate::contract::WireFormat;
    use crate::format::{Json, MsgPack, Packed};
    use alloc::string::String;
    use alloc::vec::Vec;
    use serde::Deserialize;

    #[derive(Serialize, Deserialize, Debug, PartialEq)]
    struct Named {
        #[serde(
            deserialize_with = "decode::string::<_, 3>",
            serialize_with = "encode::string::<_, 3>"
        )]
        name: String,
        #[serde(
            deserialize_with = "decode::vec::<_, u8, 2>",
            serialize_with = "encode::vec::<_, u8, 2>"
        )]
        tags: Vec<u8>,
    }
    #[derive(Serialize)]
    struct Unbounded<'a> {
        name: &'a str,
        tags: &'a [u8],
    }

    fn check<F: WireFormat>(f: F) {
        let chars = RuntimeError::BoundExceeded {
            measure: Measure::Chars,
        };
        let items = RuntimeError::BoundExceeded {
            measure: Measure::Items,
        };
        let encode = |name: &str, tags: &[u8]| {
            let mut out = Vec::new();
            f.encode(&Unbounded { name, tags }, &mut out).map(|()| out)
        };
        let fits = encode("é😀a", &[1, 2]).unwrap();
        assert_eq!(
            f.decode::<Named>(&fits),
            Ok(Named {
                name: "é😀a".into(),
                tags: vec![1, 2]
            })
        );
        assert_eq!(f.decode::<Named>(&encode("abcd", &[]).unwrap()), Err(chars));
        assert_eq!(
            f.decode::<Named>(&encode("", &[1, 2, 3]).unwrap()),
            Err(items)
        );
        let mut out = Vec::new();
        let over = Named {
            name: "ééée".into(),
            tags: vec![],
        };
        assert_eq!(f.encode(&over, &mut out), Err(chars));
        let over = Named {
            name: "".into(),
            tags: vec![1, 2, 3],
        };
        assert_eq!(f.encode(&over, &mut out), Err(items));
    }
    check(Packed);
    check(MsgPack);
    check(Json);
}

#[cfg(feature = "std")]
#[test]
fn a_single_char_on_each_format() {
    use crate::contract::WireFormat;
    use crate::format::{Json, MsgPack, Packed};
    use alloc::vec::Vec;

    let bytes = |f: &dyn Fn(&mut Vec<u8>)| {
        let mut out = Vec::new();
        f(&mut out);
        out
    };
    for c in ['a', 'é', '€', '😀'] {
        let mut utf8 = [0; 4];
        let utf8 = c.encode_utf8(&mut utf8).as_bytes();
        let packed = bytes(&|o| Packed.encode(&c, o).unwrap());
        assert_eq!(packed, utf8, "packed@1: raw UTF-8, no prefix");
        assert_eq!(Packed.decode::<char>(&packed), Ok(c));
        let msgpack = bytes(&|o| MsgPack.encode(&c, o).unwrap());
        assert_eq!(msgpack[0], 0xa0 | utf8.len() as u8, "msgpack@1: a fixstr");
        assert_eq!(MsgPack.decode::<char>(&msgpack), Ok(c));
        let json = bytes(&|o| Json.encode(&c, o).unwrap());
        assert_eq!(Json.decode::<char>(&json), Ok(c));
    }
    assert!(Packed.decode::<char>(b"ab").is_err());
    assert!(Packed.decode::<char>(&[0xc3]).is_err());
    assert!(MsgPack.decode::<char>(&[0xa2, b'a', b'b']).is_err());
    assert!(Json.decode::<char>(b"\"ab\"").is_err());
    assert!(Json.decode::<char>(b"\"\"").is_err());
}
