package catalog

encodings: {
	"packed@1": {
		kind:        "format"
		title:       "packed"
		description: "Fixed-width fields back to back, varint lengths, no tags."
		feature:     "format-packed"
		tags: ["official"]
	}
	"json@1": {
		kind:        "format"
		title:       "JSON"
		description: "Values as UTF-8 JSON: a struct as an object."
		feature:     "format-json"
		tags: ["official"]
	}
	"msgpack@1": {
		kind:        "format"
		title:       "MessagePack"
		description: "Values as MessagePack: a struct as an array."
		feature:     "format-msgpack"
		tags: ["official"]
	}
	"jsonrpc@1": {
		kind:        "framing"
		title:       "JSON-RPC"
		description: "JSON-RPC 2.0 requests and responses."
		aliases: ["jsonrpc", "json-rpc", "jsonrpc-2.0"]
		frozen_as: "jsonrpc"
		feature:   "framing-jsonrpc"
		tags: ["official"]
	}
	"datagram@1": {
		kind:        "framing"
		title:       "datagram"
		description: "Comline's own binary framing of requests and responses."
		aliases: ["datagram", "comline.datagram"]
		frozen_as: "datagram"
		feature:   "framing-datagram"
		tags: ["official"]
	}
	"datagram@2": {
		kind:        "framing"
		title:       "datagram@2"
		description: "The datagram framing with a request/reply kind and a protocol number, for calls both ways and several protocols on one connection."
		feature:     "framing-datagram2"
		tags: ["official"]
	}
	"datagram@3": {
		kind:        "framing"
		title:       "datagram@3"
		description: "The datagram framing that addresses a call by its protocol's and function's wire numbers."
		feature:     "framing-datagram3"
		since:       "0.4"
		tags: ["official"]
	}
	"comline@1": {
		kind:        "handshake"
		title:       "comline@1"
		description: "The fixed frame each end sends first: magic, version, IR hash, wire format, framing and capabilities."
		feature:     "handshake"
		tags: ["official"]
	}
	"comline@2": {
		kind:        "handshake"
		title:       "comline@2"
		description: "The compatibility-checking handshake: lines, shape hashes and wire numbers instead of an exact IR match."
		feature:     "handshake-comline2"
		since:       "0.4"
		tags: ["official"]
	}
	none: {
		kind:        "handshake"
		title:       "no handshake"
		description: "No frame and nothing checked. Unversioned, since there is nothing to evolve; it establishes neither agreement."
		profile:     false
		tags: ["official"]
	}
}
