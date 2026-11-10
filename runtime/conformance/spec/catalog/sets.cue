package catalog

// Each set is a query over the entries, so it can't drift from them. A
// label that names a top-level field is quoted, so it doesn't shadow it.
sets: {
	"guarantees": [for id, _ in guarantees {id}]
	// Where an `@unverified` marker may sit, outermost to innermost.
	scopes: #Scopes
	formats: [for id, e in encodings if e.kind == "format" {id}]
	framings: [for id, e in encodings if e.kind == "framing" {id}]
	handshakes: [for id, e in encodings if e.kind == "handshake" {id}]
	official: [for id, e in encodings for t in e.tags if t == "official" {id}]
	// The encodings with a profile: `profiles/<name>.json` exists for each.
	profiled: [for id, e in encodings if e.profile {id}]
	format_facts: [for id, f in facts if f.applies_to == "format" {id}]
	framing_facts: [for id, f in facts if f.applies_to == "framing" {id}]
	handshake_facts: [for id, f in facts if f.applies_to == "handshake" {id}]
	// Every feature: `vectors/<id>.json` exists for each.
	"features": [for id, _ in features {id}]
	// Every alias, to the wire name it resolves to.
	aliases: _spellings
}

// What `catalog/gen/catalog.json` holds, for readers without CUE.
catalog: {
	"guarantees": guarantees
	"facts":      facts
	"encodings":  encodings
	"features":   features
	"sets":       sets
}

// What `features.json` holds, in the shape `specs/features.cue` gives it.
features_json: {
	$schema: "specs/gen/features.schema.json"
	"features": [for id, f in features {"id": id, title: f.title, description: f.description}]
}
