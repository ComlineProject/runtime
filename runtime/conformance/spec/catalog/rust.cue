package catalog

import (
	"list"
	"regexp"
	"strings"
)

// `rust/src/catalog.rs`: the catalog as Rust data, for the `comline-catalog`
// crate. `rust/src/lib.rs` defines the types it fills. Strings are raw
// literals, so no text may hold `"#`.

_rust: {
	// A Rust raw string literal.
	str: {
		#in: string
		out: "r#\"\(#in)\"#"
	}
	opt: {
		#in: string | *null
		let v = #in
		out: [if v != null {"Some(\((str & {#in: v}).out))"}, "None"][0]
	}
	list: {
		#in: [...string]
		out: "&[" + strings.Join([for s in #in {(str & {#in: s}).out}], ", ") + "]"
	}
	// A constant's name: `packed@1` -> `PACKED_1`.
	const: {
		#in: string
		out: strings.ToUpper(regexp.ReplaceAll("[^a-zA-Z0-9]+", #in, "_"))
	}
	kind: {
		#in: "format" | "framing" | "handshake"
		out: {format: "Kind::Format", framing: "Kind::Framing", handshake: "Kind::Handshake"}[#in]
	}
	scope: {
		#in: #Scope
		out: {package: "Scope::Package", protocol: "Scope::Protocol", function: "Scope::Function", argument: "Scope::Argument", field: "Scope::Field", type: "Scope::Type"}[#in]
	}
	scopes: {
		#in: [...#Scope]
		out: "&[" + strings.Join([for s in #in {(scope & {#in: s}).out}], ", ") + "]"
	}
	header: {
		#in: {...}
		let h = #in
		out: [
			if h.kind == "none" {"Header::None"},
			if h.kind == "varint_leb128" {"Header::VarintLeb128"},
			if h.kind == "msgpack_str" {"Header::MsgpackStr"},
			if h.kind == "msgpack_array" {"Header::MsgpackArray"},
			if h.kind == "delimited" {"Header::Delimited { before: \(h.before), after: \(h.after), between: \(h.between) }"},
		][0]
	}
	width: {
		#in: {...}
		let w = #in
		out: "Width { min: \(w.min), max: \(w.max), upper_bound: \([if w.upper_bound != _|_ {"true"}, "false"][0]), proven_by: \((list & {#in: w.proven_by}).out) }"
	}
	// An encoding's size rules, as a `Sizes` constant.
	sizes: {
		#name: string
		#in: {...}
		let s = #in
		let enum_tag = [
			if s.enum.tag == "varint_index" {"EnumTag::VarintIndex"},
			if s.enum.tag == "name" {"EnumTag::Name(\((header & {#in: s.enum.header}).out))"},
		][0]
		let key_extra = [if s.struct.key_extra != _|_ {"Some(\(s.struct.key_extra))"}, "None"][0]
		let proofs = {
			#of: [...string]
			out: (list & {#in: #of}).out
		}
		out: strings.Join([
			"const \(#name): Sizes = Sizes {",
			"    feature: \((str & {#in: s.feature}).out),",
			"    scalars: Scalars {",
			for n in ["bool", "u8", "u16", "u32", "u64", "u128", "s8", "s16", "s32", "s64", "s128", "f32", "f64", "char", "unit"] {
				"        \(n): \((width & {#in: s.scalars[n]}).out),"
			},
			"    },",
			"    string: StringSize {",
			"        header: \((header & {#in: s."string".header}).out),",
			"        body: \({utf8: "Body::Utf8", json_escaped: "Body::JsonEscaped"}[s."string".body]),",
			"        proven_by: \((proofs & {#of: s."string".proven_by}).out),",
			"    },",
			"    array: SeqSize { header: \((header & {#in: s.array.header}).out), proven_by: \((proofs & {#of: s.array.proven_by}).out) },",
			"    fixed_array: SeqSize { header: \((header & {#in: s.fixed_array.header}).out), proven_by: \((proofs & {#of: s.fixed_array.proven_by}).out) },",
			"    option: OptionSize { none: \(s.option.none), some: \(s.option.some), proven_by: \((proofs & {#of: s.option.proven_by}).out) },",
			"    struct_: StructSize { header: \((header & {#in: s."struct".header}).out), key_extra: \(key_extra), proven_by: \((proofs & {#of: s."struct".proven_by}).out) },",
			"    enum_: EnumSize { tag: \(enum_tag), proven_by: \((proofs & {#of: s."enum".proven_by}).out) },",
			"};",
		], "\n")
	}
	status: {
		#in: "stable" | "experimental" | "deprecated"
		out: {stable: "Status::Stable", experimental: "Status::Experimental", deprecated: "Status::Deprecated"}[#in]
	}
	lifecycle: {
		#in: {...}
		let e = #in
		out: """
					status: \((status & {#in: e.status}).out),
					replaced_by: \((opt & {#in: [if e.replaced_by != _|_ {e.replaced_by}, null][0]}).out),
					since: \((opt & {#in: [if e.since != _|_ {e.since}, null][0]}).out),
			"""
	}
}

// No text may end a raw string early.
_raw_safe: [
	for k, entries in {"guarantees": guarantees, "facts": facts, "encodings": encodings, "features": features}
	for id, e in entries
	for f, v in e
	if (v & string) != _|_
	if strings.Contains(v, "\"#") {"\(k).\(id).\(f)"},
] & []

// Constant names stay distinct within each kind.
_unique_consts: {
	for k, entries in {"guarantees": guarantees, "facts": facts, "encodings": encodings, "features": features} {
		(k): list.UniqueItems() & [for id, _ in entries {(_rust.const & {#in: id}).out}]
	}
}

_ids: {
	#in: {...}
	out: strings.Join([for id, _ in #in {"    pub const \((_rust.const & {#in: id}).out): &str = \((_rust.str & {#in: id}).out);"}], "\n")
}

_set: {
	#name: string
	#in: [...string]
	let items = #in
	out: "    pub const \(#name): &[&str] = \((_rust.list & {#in: items}).out);"
}

rust_catalog: """
	// Generated from the catalog's CUE by `specs/check.sh`: don't edit by hand.

	use crate::{
	    Body, Encoding, EnumSize, EnumTag, Fact, Feature, Guarantee, Header, Kind, Origin, OptionSize, Scalars, Scope,
	    SeqSize, Sizes, Status, StringSize, StructSize, Value, Width,
	};

	pub const SPEC_VERSION: &str = \((_rust.str & {#in: _spec_version}).out);

	/// Every guarantee, in the catalog's order.
	pub const GUARANTEES: &[Guarantee] = &[
	\(strings.Join([for id, g in guarantees {"""
			Guarantee {
				id: \((_rust.str & {#in: id}).out),
				title: \((_rust.str & {#in: g.title}).out),
				description: \((_rust.str & {#in: g.description}).out),
				comes_from: \({handshake: "Origin::Handshake", encoding: "Origin::Encoding", policy: "Origin::Policy", type: "Origin::Type"}[g.comes_from]),
				scopes: \((_rust.scopes & {#in: g.scopes}).out),
		\((_rust.lifecycle & {#in: g}).out)
			},
		"""}], "\n"))
	];

	/// Every fact, in the order profiles list them.
	pub const FACTS: &[Fact] = &[
	\(strings.Join([for id, f in facts {"""
			Fact {
				id: \((_rust.str & {#in: id}).out),
				applies_to: \((_rust.kind & {#in: f.applies_to}).out),
				title: \((_rust.str & {#in: f.title}).out),
				description: \((_rust.str & {#in: f.description}).out),
				values: &[\(strings.Join([for v, meaning in f.values {"Value { id: \((_rust.str & {#in: v}).out), meaning: \((_rust.str & {#in: meaning}).out) }"}], ", "))],
				establishes: \((_rust.opt & {#in: [if f.establishes != _|_ {f.establishes}, null][0]}).out),
		\((_rust.lifecycle & {#in: f}).out)
			},
		"""}], "\n"))
	];

	\(strings.Join([for id, e in encodings if e.sizes != _|_ {(_rust.sizes & {#name: "SIZES_" + (_rust.const & {#in: id}).out, #in: e.sizes}).out + "\n"}], "\n"))
	/// Every official encoding, by its wire name.
	pub const ENCODINGS: &[Encoding] = &[
	\(strings.Join([for id, e in encodings {"""
			Encoding {
				id: \((_rust.str & {#in: id}).out),
				kind: \((_rust.kind & {#in: e.kind}).out),
				title: \((_rust.str & {#in: e.title}).out),
				description: \((_rust.str & {#in: e.description}).out),
				aliases: \((_rust.list & {#in: e.aliases}).out),
				frozen_as: \((_rust.opt & {#in: [if e.frozen_as != _|_ {e.frozen_as}, null][0]}).out),
				profile: \(e.profile),
				feature: \((_rust.opt & {#in: [if e.feature != _|_ {e.feature}, null][0]}).out),
				sizes: \([if e.sizes != _|_ {"Some(&SIZES_" + (_rust.const & {#in: id}).out + ")"}, "None"][0]),
				tags: \((_rust.list & {#in: e.tags}).out),
		\((_rust.lifecycle & {#in: e}).out)
			},
		"""}], "\n"))
	];

	/// Every wire feature, in the order `features.json` lists them.
	pub const FEATURES: &[Feature] = &[
	\(strings.Join([for id, f in features {"""
			Feature {
				id: \((_rust.str & {#in: id}).out),
				title: \((_rust.str & {#in: f.title}).out),
				description: \((_rust.str & {#in: f.description}).out),
		\((_rust.lifecycle & {#in: f}).out)
			},
		"""}], "\n"))
	];

	/// Guarantee ids.
	pub mod guarantee {
	\((_ids & {#in: guarantees}).out)
	}

	/// Fact ids.
	pub mod fact {
	\((_ids & {#in: facts}).out)
	}

	/// Encoding wire names.
	pub mod encoding {
	\((_ids & {#in: encodings}).out)
	}

	/// Feature ids.
	pub mod feature {
	\((_ids & {#in: features}).out)
	}

	/// The catalog's sets.
	pub mod sets {
	\((_set & {#name: "GUARANTEES", #in: sets.guarantees}).out)
	\((_set & {#name: "SCOPES", #in: sets.scopes}).out)
	\((_set & {#name: "FORMATS", #in: sets.formats}).out)
	\((_set & {#name: "FRAMINGS", #in: sets.framings}).out)
	\((_set & {#name: "HANDSHAKES", #in: sets.handshakes}).out)
	\((_set & {#name: "OFFICIAL", #in: sets.official}).out)
	\((_set & {#name: "PROFILED", #in: sets.profiled}).out)
	\((_set & {#name: "FORMAT_FACTS", #in: sets.format_facts}).out)
	\((_set & {#name: "FRAMING_FACTS", #in: sets.framing_facts}).out)
	\((_set & {#name: "HANDSHAKE_FACTS", #in: sets.handshake_facts}).out)
	\((_set & {#name: "FEATURES", #in: sets.features}).out)
	    /// Every alias, with the wire name it resolves to.
	    pub const ALIASES: &[(&str, &str)] = &[\(strings.Join([for a, id in sets.aliases {"(\((_rust.str & {#in: a}).out), \((_rust.str & {#in: id}).out))"}], ", "))];
	}

	"""

// The spec's version, from `VERSION`: `specs/check.sh` passes it as
// `-t version=...`.
_spec_version: *"" | =~"^[0-9]+\\.[0-9]+$" @tag(version)
