use comline_runtime_ffi::*;

/// What each function's doc comment says it returns.
pub(super) const NEW: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_UNKNOWN_FRAMING,
];
pub(super) const START: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_BUFFER_TOO_SMALL,
    COMLINE_ERR_FRAME_TOO_LARGE,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_STATE,
];
pub(super) const FEED: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_NEED_MORE,
    COMLINE_ERR_FRAMING,
    COMLINE_ERR_PEER_REFUSED,
    COMLINE_ERR_HANDSHAKE,
    COMLINE_ERR_FRAME_TOO_LARGE,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_STATE,
];
pub(super) const REQUEST: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_BUFFER_TOO_SMALL,
    COMLINE_ERR_FRAME_TOO_LARGE,
    COMLINE_ERR_BUSY,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_STATE,
];
pub(super) const REPLY: &[comline_status] = &[
    COMLINE_OK,
    COMLINE_BUFFER_TOO_SMALL,
    COMLINE_ERR_FRAME_TOO_LARGE,
    COMLINE_ERR_INVALID_ARGUMENT,
    COMLINE_ERR_STATE,
];
pub(super) const PLAIN: &[comline_status] =
    &[COMLINE_OK, COMLINE_ERR_INVALID_ARGUMENT, COMLINE_ERR_STATE];

/// A set of statuses, as bits.
pub(super) fn bit(status: comline_status) -> u32 {
    1u32.checked_shl(status as u32).unwrap_or(0)
}

pub(super) fn statuses(set: u32) -> String {
    let all: Vec<_> = (0..32).filter(|s| set & (1 << s) != 0).collect();
    format!("{all:?}")
}
