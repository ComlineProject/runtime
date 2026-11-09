package catalog

guarantees: {
	schema_agreement: {
		title:       "schema agreement"
		description: "Both ends were built from compatible schemas, which the handshake checks."
		comes_from:  "handshake"
	}
	encoding_agreement: {
		title:       "encoding agreement"
		description: "Both ends use the same format and framing, which the handshake checks."
		comes_from:  "handshake"
	}
	encoding_conformance: {
		title:       "encoding conformance"
		description: "An implementation encodes what its name promises, proven by conformance vectors."
		comes_from:  "encoding"
	}
	version_honesty: {
		title:       "version honesty"
		description: "A version bump says what the change breaks, as the version comparison works it out."
		comes_from:  "policy"
	}
	bounded_size: {
		title:       "bounded size"
		description: "Every value has a maximum size, so a decoder refuses an oversized one before allocating it."
		comes_from:  "type"
	}
}
