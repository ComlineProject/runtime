use super::*;
use alloc::vec::Vec;

use serde::Serialize;

use crate::contract::{
    Call, ConnectionFraming, Datagram2Framing, Envelope, RuntimeError, WireFormat,
};

#[test]
fn a_reply_must_answer_the_innermost_waiting_call() {
    let framing = Datagram2Framing;
    let mut state = State::default();
    let mut out = Vec::new();
    let outer = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
        .unwrap();
    let inner = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
        .unwrap();

    let reply = |id| {
        let mut frame = Vec::new();
        framing.encode_reply(0, id, &Envelope::Ok(b""), &mut frame);
        frame
    };
    assert_eq!(
        state.receive(&framing, &reply(outer)),
        Err(RuntimeError::Framing)
    );
    assert!(matches!(
        state.receive(&framing, &reply(inner)),
        Ok(Event::Reply { .. })
    ));
    assert!(matches!(
        state.receive(&framing, &reply(outer)),
        Ok(Event::Reply { .. })
    ));
    assert!(!state.is_waiting());
    assert_eq!(state.receive(&framing, &reply(99)), Ok(Event::Stale));
}

#[test]
fn an_unordered_state_takes_replies_in_any_order() {
    let framing = Datagram2Framing;
    let mut state = State::unordered();
    let mut out = Vec::new();
    let first = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
        .unwrap();
    let second = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
        .unwrap();
    let reply = |id| {
        let mut frame = Vec::new();
        framing.encode_reply(0, id, &Envelope::Ok(b""), &mut frame);
        frame
    };
    assert!(
        matches!(state.receive(&framing, &reply(first)), Ok(Event::Reply { request_id, .. }) if request_id == first)
    );
    assert!(matches!(
        state.receive(&framing, &reply(second)),
        Ok(Event::Reply { .. })
    ));
    assert_eq!(state.receive(&framing, &reply(first)), Ok(Event::Stale));
    assert!(!state.is_waiting());
}

#[test]
fn an_abandoned_call_s_late_reply_is_stale() {
    let framing = Datagram2Framing;
    let mut state = State::default();
    let mut out = Vec::new();
    let id = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
        .unwrap();
    state.abandon(id);
    let mut frame = Vec::new();
    framing.encode_reply(0, id, &Envelope::Fault, &mut frame);
    assert_eq!(state.receive(&framing, &frame), Ok(Event::Stale));
}

#[test]
fn a_retracted_call_spends_nothing() {
    let framing = Datagram2Framing;
    let mut state = State::default();
    let mut out = Vec::new();
    let id = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
        .unwrap();
    state.retract(id);
    assert!(!state.is_waiting());
    let again = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
        .unwrap();
    assert_eq!(again, id, "the id is handed out again");
}

#[test]
fn retract_after_a_later_call_does_not_rewind_the_ids() {
    let framing = Datagram2Framing;
    let mut state = State::default();
    let mut out = Vec::new();
    let first = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, true, &mut out)
        .unwrap();
    let later = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, false, &mut out)
        .unwrap();
    state.retract(first);
    assert!(!state.is_waiting());
    let next = state
        .encode_request(&framing, 0, Call::from(0), &(), &Unit, false, &mut out)
        .unwrap();
    assert_eq!(next, later + 1, "neither id is handed out again");
}

#[test]
fn extensions_hold_one_value_per_type() {
    struct Session(u32);
    let mut extensions = Extensions::default();
    assert!(extensions.insert(Session(1)).is_none());
    assert_eq!(extensions.insert(Session(2)).map(|s| s.0), Some(1));
    extensions.get_mut::<Session>().unwrap().0 += 1;
    assert_eq!(extensions.get::<Session>().map(|s| s.0), Some(3));
    assert_eq!(extensions.remove::<Session>().map(|s| s.0), Some(3));
    assert!(extensions.get::<Session>().is_none());
}

/// A format that writes nothing, for framing-only tests.
struct Unit;

impl WireFormat for Unit {
    fn name(&self) -> &'static str {
        "unit"
    }
    fn encode<T: Serialize + ?Sized>(
        &self,
        _: &T,
        _: &mut dyn crate::contract::BufMut,
    ) -> Result<(), RuntimeError> {
        Ok(())
    }
    fn decode<'de, T: serde::Deserialize<'de>>(&self, _: &'de [u8]) -> Result<T, RuntimeError> {
        Err(RuntimeError::Serialization)
    }
}
