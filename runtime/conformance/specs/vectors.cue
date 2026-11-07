package conformance

// `vectors/<feature>.json`: one feature's cases.
#Vectors: close({
	$schema?: string
	feature!: string
	cases!: [...#Case]
})

// One vector: an operation's input and the exact bytes it must produce and
// decode from, or (`expect: "reject"`) bytes the operation must refuse. A
// rejection's input is only what selects the decoder, if anything. A case
// that doesn't say `expect` is exact: `*` picks that when both could hold.
#Case:
	*close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: "exact", op!: "name_hash", input!: close({name!: string})}) |
	*close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: "exact", op!: "handshake", input!: #HandshakeFrame}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect!: "reject", op!: "handshake", input?: #HandshakeFrame}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: #Expect, op!: "handshake_check", input!: #HandshakeFrame}) |
	*close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: "exact", op!: "envelope", input!: #Reply}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect!: "reject", op!: "envelope", input?: #Reply}) |
	*close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: "exact", op!: "request", input!: #RequestInput}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect!: "reject", op!: "request", input!: #SelectsFraming}) |
	*close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: "exact", op!: "response", input!: #ResponseInput}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect!: "reject", op!: "response", input!: #SelectsFraming}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: #Expect, op!: "encode", input!: #EncodeInput}) |
	*close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: "exact", op!: "decode", input!: #DecodedInput}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect!: "reject", op!: "decode", input!: #DecodeInput}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: #Expect, op!: "stream_prefix", input!: close({len!: uint, max!: uint})}) |
	*close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: "exact", op!: "stream_split", input!: #StreamSplitInput}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect!: "reject", op!: "stream_split", input!: close({max!: uint})}) |
	*close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: "exact", op!: "comline2_message", input!: #Comline2Message}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect!: "reject", op!: "comline2_message", input?: #Comline2Message}) |
	close({id!: #CaseId, bytes!: #Hex, compare?: #Compare, expect?: #Expect, op!: "comline2_negotiate", input!: #Comline2Negotiate})

// By default, exact.
#Expect: "exact" | "reject"

// How a runtime's output must match the bytes: by default, byte for byte.
#Compare: "bytes" | "json"

// names unhashed
#HandshakeFrame: close({
	ir_hash!:      #Unsigned
	wire_format!:  string
	framing!:      string
	capabilities!: uint32
})

// A response's outcome.
#Reply:
	close({kind!: "ok", payload!: #Hex}) |
	close({kind!: "err", id!: uint16, body!: #Hex}) |
	close({kind!: "fault"})

#SelectsFraming: close({framing!: string})

#RequestInput: close({
	framing!:   string
	protocol?:  uint16
	call_id!:   uint16
	method!:    string
	request_id!: #Unsigned
	one_way?:   true
	format!:    string
	params!:    #Value
})

#ResponseInput:
	close({framing!: string, protocol?: uint16, request_id!: #Unsigned, kind!: "ok", payload!: #Hex}) |
	close({framing!: string, protocol?: uint16, request_id!: #Unsigned, kind!: "err", id!: uint16, body!: #Hex}) |
	close({framing!: string, protocol?: uint16, request_id!: #Unsigned, kind!: "fault"})

#EncodeInput: close({
	format!: string

	// When the value's bounds matter; otherwise its own shape is its type
	type?:  #Type
	value!: #Value
})

// What decoding gives.
#DecodedInput: close({
	format!: string
	type!:   #Type
	value!:  #Value
})

// A rejection states no value.
#DecodeInput: close({
	format!: string
	type!:   #Type
})

#StreamSplitInput: close({
	max!: uint
	frames!: [...#Hex]
	rest!:   #Hex
	needed!: uint
})

// comline@2, as `comline_runtime::comline2` serialises it.

#Hash128: =~"^[0-9a-f]{32}$"

#Comline2Reason:
	"malformed" | "handshake_version" | "exchange_mismatch" |
	"package_mismatch" | "encoding_mismatch" | "line_mismatch" |
	"same_version_differs" | "newer_consumer" | "unknown_protocol"

#Comline2Side: "initiator" | "responder"

#Comline2Message:
	close({type!: "hello", message!: #Comline2Hello}) |
	close({type!: "answer", message!: #Comline2Answer}) |
	close({type!: "verdict", message!: #Comline2Verdict})

#Comline2Release: close({
	major!: uint32
	minor!: uint32
	patch!: uint32
})

#Comline2Version: close({
	major!: uint32
	minor!: uint32
	patch!: uint32
	pre?:   string
})

#Comline2Hello: close({
	exchange!:   "offer" | "symmetric"
	package_id!: #Hash128
	version!:    #Comline2Version
	encodings!: [...close({
		kind!:       "format" | "framing" | "handshake"
		param_hash!: #Hash128
	})]
	protocols!: [...close({
		number!:     uint16
		line!:       #Comline2Release
		shape_hash!: #Hash128
		functions!: [...uint16]
	})]
})

#Comline2Refusal: close({
	reason!:   #Comline2Reason
	side!:     #Comline2Side
	protocol?: uint16
})

#Comline2Answer:
	close({
		accept!: close({
			version!: #Comline2Version
			protocols!: [...close({
				number!: uint16
				status!:
					close({accepted!: close({shape_differs!: bool, missing!: [...uint16]})}) |
					close({refused!: close({reason!: #Comline2Reason})})
			})]
		})
	}) |
	close({refuse!: #Comline2Refusal})

#Comline2Verdict:
	close({
		accept!: close({
			refused!: [...close({number!: uint16, reason!: #Comline2Reason})]
		})
	}) |
	close({refuse!: #Comline2Refusal})

#Comline2Negotiated: close({
	peer_version!: #Comline2Version
	protocols!: [...close({
		number!: uint16
		state!:
			close({accepted!: close({peer_lacks!: [...uint16]})}) |
			close({refused!: close({reason!: #Comline2Reason})})
	})]
	warnings?: [...uint16]
})

#Comline2Negotiate: close({
	step!: "answer_offer" | "conclude_answer" | "judge_hello" | "finish"
	side?: #Comline2Side
	params?: close({
		scope!:                "protocol" | "package"
		newer_consumer!:       "degrade" | "refuse"
		same_version_differs!: "refuse" | "warn"
		exchange!:             "offer" | "symmetric"
	})
	local?:   #Comline2Hello
	pending?: #Comline2Negotiated

	// The message this end sends, if any. Left out for `finish`.
	reply?: null | #Comline2Message
	result!:
		close({accepted!: #Comline2Negotiated}) |
		close({refused!: #Comline2Refusal})
})
