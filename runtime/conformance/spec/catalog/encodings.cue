package catalog

encodings: {
	"packed@1": {
		kind:        "format"
		title:       "packed"
		description: "Fixed-width fields back to back, varint lengths, no tags."
		feature:     "format-packed"
		sizes: {
			feature: "sizes-packed"
			scalars: {
				"bool": {min: 1, max: 1, proven_by: ["sizes-packed/bool"]}
				u8: {min: 1, max: 1, proven_by: ["sizes-packed/u8"]}
				u16: {min: 2, max: 2, proven_by: ["sizes-packed/u16"]}
				u32: {min: 4, max: 4, proven_by: ["sizes-packed/u32"]}
				u64: {min: 8, max: 8, proven_by: ["sizes-packed/u64"]}
				u128: {min: 16, max: 16, proven_by: ["sizes-packed/u128"]}
				s8: {min: 1, max: 1, proven_by: ["sizes-packed/i8"]}
				s16: {min: 2, max: 2, proven_by: ["sizes-packed/i16"]}
				s32: {min: 4, max: 4, proven_by: ["sizes-packed/i32"]}
				s64: {min: 8, max: 8, proven_by: ["sizes-packed/i64"]}
				s128: {min: 16, max: 16, proven_by: ["sizes-packed/i128"]}
				f32: {min: 4, max: 4, proven_by: ["sizes-packed/f32"]}
				f64: {min: 8, max: 8, proven_by: ["sizes-packed/f64"]}
				char: {min: 1, max: 4, proven_by: ["sizes-packed/char"]}
				unit: {min: 0, max: 0, proven_by: ["sizes-packed/unit"]}
			}
			"string": {header: {kind: "varint_leb128"}, body: "utf8", proven_by: [
				"sizes-packed/string-unbounded",
				"sizes-packed/string-max-0",
				"sizes-packed/string-max-1",
				"sizes-packed/string-max-3",
				"sizes-packed/string-max-7",
				"sizes-packed/string-max-8",
				"sizes-packed/string-max-31",
				"sizes-packed/string-max-32",
				"sizes-packed/string-max-63",
				"sizes-packed/string-max-64",
				"sizes-packed/string-max-4095",
				"sizes-packed/string-max-4096",
				"sizes-packed/string-fixed-3",
				"sizes-packed/string-fixed-8",
				"sizes-packed/string-fixed-32",
			]}
			array: {header: {kind: "varint_leb128"}, proven_by: [
				"sizes-packed/array-unbounded",
				"sizes-packed/array-unbounded-strings",
				"sizes-packed/array-max-3",
				"sizes-packed/array-max-127",
				"sizes-packed/array-max-128",
				"sizes-packed/array-max-2-of-strings",
				"sizes-packed/array-max-3-of-options",
				"sizes-packed/recursive-tree-2",
			]}
			fixed_array: {header: {kind: "none"}, proven_by: [
				"sizes-packed/fixed-4",
				"sizes-packed/fixed-1",
				"sizes-packed/fixed-2-of-strings",
				"sizes-packed/fixed-3-of-options",
			]}
			option: {none: 1, some: 1, proven_by: [
				"sizes-packed/option-u8",
				"sizes-packed/option-bool",
				"sizes-packed/option-string",
				"sizes-packed/option-max-3-chars",
				"sizes-packed/option-struct",
				"sizes-packed/recursive-chain-3",
			]}
			"struct": {header: {kind: "none"}, proven_by: [
				"sizes-packed/struct-point",
				"sizes-packed/struct-empty",
				"sizes-packed/struct-mixed",
				"sizes-packed/struct-nested",
				"sizes-packed/struct-15-fields",
				"sizes-packed/struct-16-fields",
				"sizes-packed/struct-long-names",
				"sizes-packed/recursive-chain-3",
				"sizes-packed/recursive-tree-2",
			]}
			"enum": {tag: "varint_index", proven_by: [
				"sizes-packed/enum-color",
				"sizes-packed/enum-single",
				"sizes-packed/enum-129-variants",
				"sizes-packed/enum-long-names",
			]}
		}
		tags: ["official"]
	}
	"json@1": {
		kind:        "format"
		title:       "JSON"
		description: "Values as UTF-8 JSON: a struct as an object."
		feature:     "format-json"
		sizes: {
			feature: "sizes-json"
			scalars: {
				"bool": {min: 4, max: 5, proven_by: ["sizes-json/bool"]}
				u8: {min: 1, max: 3, proven_by: ["sizes-json/u8"]}
				u16: {min: 1, max: 5, proven_by: ["sizes-json/u16"]}
				u32: {min: 1, max: 10, proven_by: ["sizes-json/u32"]}
				u64: {min: 1, max: 20, proven_by: ["sizes-json/u64"]}
				u128: {min: 1, max: 39, proven_by: ["sizes-json/u128"]}
				s8: {min: 1, max: 4, proven_by: ["sizes-json/i8"]}
				s16: {min: 1, max: 6, proven_by: ["sizes-json/i16"]}
				s32: {min: 1, max: 11, proven_by: ["sizes-json/i32"]}
				s64: {min: 1, max: 20, proven_by: ["sizes-json/i64"]}
				s128: {min: 1, max: 40, proven_by: ["sizes-json/i128"]}
				f32: {min: 3, max: 16, proven_by: ["sizes-json/f32"]}
				f64: {min: 3, max: 24, proven_by: ["sizes-json/f64"]}
				char: {min: 3, max: 8, proven_by: ["sizes-json/char"]}
				unit: {min: 4, max: 4, proven_by: ["sizes-json/unit"]}
			}
			"string": {header: {kind: "delimited", before: 1, after: 1, between: 0}, body: "json_escaped", proven_by: [
				"sizes-json/string-unbounded",
				"sizes-json/string-max-0",
				"sizes-json/string-max-1",
				"sizes-json/string-max-3",
				"sizes-json/string-max-7",
				"sizes-json/string-max-8",
				"sizes-json/string-max-31",
				"sizes-json/string-max-32",
				"sizes-json/string-max-63",
				"sizes-json/string-max-64",
				"sizes-json/string-fixed-3",
				"sizes-json/string-fixed-8",
				"sizes-json/string-fixed-32",
			]}
			array: {header: {kind: "delimited", before: 1, after: 1, between: 1}, proven_by: [
				"sizes-json/array-unbounded",
				"sizes-json/array-unbounded-strings",
				"sizes-json/array-max-0",
				"sizes-json/array-max-1",
				"sizes-json/array-max-3",
				"sizes-json/array-max-2-of-strings",
				"sizes-json/array-max-3-of-options",
				"sizes-json/recursive-tree-2",
			]}
			fixed_array: {header: {kind: "delimited", before: 1, after: 1, between: 1}, proven_by: [
				"sizes-json/fixed-4",
				"sizes-json/fixed-1",
				"sizes-json/fixed-2-of-strings",
				"sizes-json/fixed-3-of-options",
			]}
			option: {none: 4, some: 0, proven_by: [
				"sizes-json/option-u8",
				"sizes-json/option-bool",
				"sizes-json/option-string",
				"sizes-json/option-max-3-chars",
				"sizes-json/option-struct",
				"sizes-json/recursive-chain-3",
			]}
			"struct": {header: {kind: "delimited", before: 1, after: 1, between: 1}, key_extra: 3, proven_by: [
				"sizes-json/struct-point",
				"sizes-json/struct-empty",
				"sizes-json/struct-mixed",
				"sizes-json/struct-nested",
				"sizes-json/struct-15-fields",
				"sizes-json/struct-16-fields",
				"sizes-json/struct-long-names",
				"sizes-json/recursive-chain-3",
				"sizes-json/recursive-tree-2",
			]}
			"enum": {tag: "name", header: {kind: "delimited", before: 1, after: 1, between: 0}, proven_by: [
				"sizes-json/enum-color",
				"sizes-json/enum-single",
				"sizes-json/enum-129-variants",
				"sizes-json/enum-long-names",
			]}
		}
		tags: ["official"]
	}
	"msgpack@1": {
		kind:        "format"
		title:       "MessagePack"
		description: "Values as MessagePack: a struct as an array."
		feature:     "format-msgpack"
		sizes: {
			feature: "sizes-msgpack"
			scalars: {
				"bool": {min: 1, max: 1, proven_by: ["sizes-msgpack/bool"]}
				u8: {min: 1, max: 2, proven_by: ["sizes-msgpack/u8"]}
				u16: {min: 1, max: 3, proven_by: ["sizes-msgpack/u16"]}
				u32: {min: 1, max: 5, proven_by: ["sizes-msgpack/u32"]}
				u64: {min: 1, max: 9, proven_by: ["sizes-msgpack/u64"]}
				u128: {min: 18, max: 18, proven_by: ["sizes-msgpack/u128"]}
				s8: {min: 1, max: 2, proven_by: ["sizes-msgpack/i8"]}
				s16: {min: 1, max: 3, proven_by: ["sizes-msgpack/i16"]}
				s32: {min: 1, max: 5, proven_by: ["sizes-msgpack/i32"]}
				s64: {min: 1, max: 9, proven_by: ["sizes-msgpack/i64"]}
				s128: {min: 18, max: 18, proven_by: ["sizes-msgpack/i128"]}
				f32: {min: 5, max: 5, proven_by: ["sizes-msgpack/f32"]}
				f64: {min: 9, max: 9, proven_by: ["sizes-msgpack/f64"]}
				char: {min: 2, max: 5, proven_by: ["sizes-msgpack/char"]}
				unit: {min: 1, max: 1, proven_by: ["sizes-msgpack/unit"]}
			}
			"string": {header: {kind: "msgpack_str"}, body: "utf8", proven_by: [
				"sizes-msgpack/string-unbounded",
				"sizes-msgpack/string-max-0",
				"sizes-msgpack/string-max-1",
				"sizes-msgpack/string-max-3",
				"sizes-msgpack/string-max-7",
				"sizes-msgpack/string-max-8",
				"sizes-msgpack/string-max-31",
				"sizes-msgpack/string-max-32",
				"sizes-msgpack/string-max-63",
				"sizes-msgpack/string-max-64",
				"sizes-msgpack/string-max-16384",
				"sizes-msgpack/string-fixed-3",
				"sizes-msgpack/string-fixed-8",
				"sizes-msgpack/string-fixed-32",
			]}
			array: {header: {kind: "msgpack_array"}, proven_by: [
				"sizes-msgpack/array-unbounded",
				"sizes-msgpack/array-unbounded-strings",
				"sizes-msgpack/array-max-3",
				"sizes-msgpack/array-max-15",
				"sizes-msgpack/array-max-16",
				"sizes-msgpack/array-max-2-of-strings",
				"sizes-msgpack/array-max-3-of-options",
				"sizes-msgpack/recursive-tree-2",
			]}
			fixed_array: {header: {kind: "msgpack_array"}, proven_by: [
				"sizes-msgpack/fixed-4",
				"sizes-msgpack/fixed-15",
				"sizes-msgpack/fixed-16",
				"sizes-msgpack/fixed-1",
				"sizes-msgpack/fixed-2-of-strings",
				"sizes-msgpack/fixed-3-of-options",
			]}
			option: {none: 1, some: 0, proven_by: [
				"sizes-msgpack/option-u8",
				"sizes-msgpack/option-bool",
				"sizes-msgpack/option-string",
				"sizes-msgpack/option-max-3-chars",
				"sizes-msgpack/option-struct",
				"sizes-msgpack/recursive-chain-3",
			]}
			"struct": {header: {kind: "msgpack_array"}, proven_by: [
				"sizes-msgpack/struct-point",
				"sizes-msgpack/struct-empty",
				"sizes-msgpack/struct-mixed",
				"sizes-msgpack/struct-nested",
				"sizes-msgpack/struct-15-fields",
				"sizes-msgpack/struct-16-fields",
				"sizes-msgpack/struct-long-names",
				"sizes-msgpack/recursive-chain-3",
				"sizes-msgpack/recursive-tree-2",
			]}
			"enum": {tag: "name", header: {kind: "msgpack_str"}, proven_by: [
				"sizes-msgpack/enum-color",
				"sizes-msgpack/enum-single",
				"sizes-msgpack/enum-129-variants",
				"sizes-msgpack/enum-long-names",
			]}
		}
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
