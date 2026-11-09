package catalog

import "list"

// What every entry may say about where it stands. `since` is the spec
// version that introduced it, where the changelog names one.
#Lifecycle: {
	status:       *"stable" | "experimental" | "deprecated"
	replaced_by?: string
	since?:       =~"^[0-9]+\\.[0-9]+$"
	if status == "deprecated" {
		replaced_by!: string
	}
}

// A guarantee Comline gives between peers, which a protocol can give up
// with `@unverified`. Its id is the name written in `accept_unverified`,
// diagnostics and generated docs.
#Guarantee: close({
	#Lifecycle
	title!:       string
	description!: string
	// What gives it up: the handshake, the format and framing, a handshake
	// policy, or a type.
	comes_from!: "handshake" | "encoding" | "policy" | "type"
})

// A fact an encoding's profile states, with every value it may take and
// what each means. A handshake's facts are booleans: `true` establishes the
// guarantee named in `establishes`.
#Fact: close({
	#Lifecycle
	applies_to!:  "format" | "framing" | "handshake"
	title!:       string
	description!: string
	values!: {[string]: string}
	establishes?: or([for id, _ in guarantees {id}])
})

// An official encoding, by its wire name: the spelling the handshake
// hashes, which never changes.
#Encoding: close({
	#Lifecycle
	kind!:        "format" | "framing" | "handshake"
	title!:       string
	description!: string
	// Other spellings a schema may write, resolved to this name.
	aliases: [...string]
	// The spelling core freezes into the IR, where it isn't the wire name:
	// the one used before the names were versioned.
	frozen_as?: string
	// Whether `profiles/<name>.json` states its facts.
	profile: bool | *true
	// The feature whose vectors prove it.
	feature?: or([for id, _ in features {id}])
	tags: [...string]
})

// A wire feature: a set of vector cases, `vectors/<id>.json`.
#Feature: close({
	#Lifecycle
	title!:       string
	description!: string
})

// Ids, and nothing else, in each kind: a pattern constraint alone would let
// a label that doesn't match through unchecked.
#SnakeId:    "^[a-z][a-z_]*$"
#EncodingId: "^[a-z0-9._-]+(@[0-9]+)?$"
#FeatureId:  "^[a-z0-9-]+$"

guarantees: {[=~#SnakeId]: #Guarantee, [!~#SnakeId]: _|_}
facts: {[=~#SnakeId]: #Fact, [!~#SnakeId]: _|_}
encodings: {[=~#EncodingId]: #Encoding, [!~#EncodingId]: _|_}
features: {[=~#FeatureId]: #Feature, [!~#FeatureId]: _|_}

// A handshake's facts are booleans.
_boolean_handshake_facts: [for id, f in facts if f.applies_to == "handshake" if [for v, _ in f.values {v}] != ["true", "false"] {id}] & []

// An alias names one encoding, and is never another's wire name or frozen
// spelling. A frozen spelling is also an alias.
_spellings: {
	for id, e in encodings for a in e.aliases {(a): id}
}
_aliases_are_unique: list.UniqueItems() & [for id, e in encodings for a in e.aliases {a}]
_aliases_are_not_names: [for a, _ in _spellings if encodings[a] != _|_ {a}] & []
_frozen_is_an_alias: [for id, e in encodings if e.frozen_as != _|_ if !list.Contains(e.aliases, e.frozen_as) {id}] & []

// An official profile, `profiles/<name>.json`: of an encoding the catalog
// gives a profile, and of that encoding's kind. `specs/profile.cue` says the
// rest.
#OfficialProfile: {
	name!: or(sets.profiled)
	kind!: encodings[name].kind
	...
}
