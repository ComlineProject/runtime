package catalog

// In the order a profile lists them, which `specs/profile.cue` and its
// generated schema follow.
facts: {
	struct_fields: {
		applies_to:  "format"
		title:       "struct fields"
		description: "How a struct's fields are written."
		values: {
			by_position: "back to back, in declaration order"
			by_name:     "each keyed by its name"
		}
	}
	unknown_field: {
		applies_to:  "format"
		title:       "unknown field"
		description: "What a reader does with a field its type doesn't have."
		values: {
			rejected: "refuses the value"
			ignored:  "skips the field"
		}
	}
	missing_optional: {
		applies_to:  "format"
		title:       "missing optional"
		description: "What a reader does when an optional field isn't there."
		values: {
			rejected: "refuses the value"
			absent:   "reads it as absent"
		}
	}
	enum_variants: {
		applies_to:  "format"
		title:       "enum variants"
		description: "How an enum's variant is written."
		values: {
			by_name:  "by its name"
			by_index: "by its position in the declaration"
		}
	}
	unknown_variant: {
		applies_to:  "format"
		title:       "unknown variant"
		description: "What a reader does with a variant its type doesn't have."
		values: {
			rejected:  "refuses the value"
			tolerated: "reads it without failing"
		}
	}
	integer_width: {
		applies_to:  "format"
		title:       "integer width"
		description: "Whether an integer reads into another width."
		values: {
			by_value: "any width the value fits"
			exact:    "only the width it was written as"
		}
	}
	output: {
		applies_to:  "format"
		title:       "output"
		description: "The grammar a format writes in, which a framing's `payload` must take."
		since:       "0.7"
		values: {
			bytes: "bytes in no grammar a framing reads"
			json:  "JSON text"
		}
	}
	calls: {
		applies_to:  "framing"
		title:       "calls"
		description: "How a request names the function it calls."
		values: {
			by_index: "by the function's position"
			by_name:  "by the function's name"
			by_id:    "by the protocol's and function's wire numbers"
		}
	}
	directions: {
		applies_to:  "framing"
		title:       "directions"
		description: "Whether a frame says if it's a request or a response."
		since:       "0.10"
		values: {
			both: "it does, so calls can go both ways on one transport"
			one:  "it doesn't"
		}
	}
	errors: {
		applies_to:  "framing"
		title:       "errors"
		description: "How an error response names its error."
		values: {
			by_ordinal: "by its position in the declaration"
			by_name:    "by its name"
		}
	}
	payload: {
		applies_to:  "framing"
		title:       "payload"
		description: "What a framing needs of a payload, which a format's `output` must fit."
		since:       "0.7"
		values: {
			opaque: "any bytes, carried as they are"
			json:   "JSON text, written into the frame"
		}
	}
	protocol: {
		applies_to:  "framing"
		title:       "protocol"
		description: "Whether a request's frame names the protocol it calls into."
		since:       "0.11"
		values: {
			named:   "it does, by number or wire id, so several protocols can share one transport"
			unnamed: "it doesn't: a call names only its function"
		}
	}
	schema_agreement: {
		applies_to:  "handshake"
		title:       "schema agreement"
		description: "Whether the handshake checks both ends were built from compatible schemas."
		since:       "0.2"
		establishes: "schema_agreement"
		values: {
			"true":  "it does, and refuses a peer that wasn't"
			"false": "it doesn't"
		}
	}
	encoding_agreement: {
		applies_to:  "handshake"
		title:       "encoding agreement"
		description: "Whether the handshake checks both ends use the same format and framing."
		since:       "0.2"
		establishes: "encoding_agreement"
		values: {
			"true":  "it does, and refuses a peer that doesn't"
			"false": "it doesn't"
		}
	}
}
