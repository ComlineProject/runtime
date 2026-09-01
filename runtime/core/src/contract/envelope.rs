use crate::contract::BufMut;

/// A response frame body: the success payload, or a raised schema error keyed
/// by its **schema-global ordinal**. Both variants borrow the receive buffer —
/// the client decodes `Ok` as `R` or looks `id` up in its generated table.
///
/// Wire layout: `[tag] payload` for `Ok`, `[tag] id_le:u16 body` for `Err`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Envelope<'a> {
    Ok(&'a [u8]),
    Err { id: u16, body: &'a [u8] },
}

const TAG_OK: u8 = 0;
const TAG_ERR: u8 = 1;

impl<'a> Envelope<'a> {
    /// Parse a response frame. `None` if the tag is unknown or the frame is
    /// truncated.
    pub fn decode(frame: &'a [u8]) -> Option<Self> {
        match frame.split_first()? {
            (&TAG_OK, payload) => Some(Envelope::Ok(payload)),
            (&TAG_ERR, rest) => {
                let (id, body) = rest.split_at_checked(2)?;
                Some(Envelope::Err {
                    id: u16::from_le_bytes([id[0], id[1]]),
                    body,
                })
            }
            _ => None,
        }
    }

    /// Write an `Ok(payload)` frame.
    pub fn encode_ok(payload: &[u8], out: &mut dyn BufMut) {
        out.put_u8(TAG_OK);
        out.put_slice(payload);
    }

    /// Write an `Err { id, body }` frame.
    pub fn encode_err(id: u16, body: &[u8], out: &mut dyn BufMut) {
        out.put_u8(TAG_ERR);
        out.put_u16_le(id);
        out.put_slice(body);
    }
}

#[cfg(all(test, feature = "alloc"))]
mod tests {
    use super::*;
    use alloc::vec::Vec;

    #[test]
    fn ok_round_trips() {
        let mut frame = Vec::new();
        Envelope::encode_ok(b"payload", &mut frame);
        assert_eq!(Envelope::decode(&frame), Some(Envelope::Ok(b"payload")));
    }

    #[test]
    fn err_round_trips() {
        let mut frame = Vec::new();
        Envelope::encode_err(0x0102, b"fields", &mut frame);
        assert_eq!(
            Envelope::decode(&frame),
            Some(Envelope::Err {
                id: 0x0102,
                body: b"fields",
            }),
        );
    }

    #[test]
    fn rejects_empty_unknown_tag_and_truncated_err() {
        assert_eq!(Envelope::decode(&[]), None);
        assert_eq!(Envelope::decode(&[9]), None); // unknown tag
        assert_eq!(Envelope::decode(&[TAG_ERR, 0]), None); // id truncated
    }
}
