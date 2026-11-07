package conformance

// One file of `benches/`: the work every runtime's benchmarks measure.
#Bench: #Formats | #Framings | #Handshake | #Comline2Bench | #Connection

// `formats.json`: encode and decode, per format, per shape.
#Formats: close({
	$schema?: string

	// The formats each shape is measured in
	formats!: [...#EncodingName]

	shapes!: [...#Shape]
})

#Shape: close({
	id!:          string
	description!: string

	// As a `decode` case states it
	type!: #Type

	// A value of that type, as an `encode` case states it, or generated
	value!: #Value | #Generate
})

// An array of `count` items, each `item` with its sequences at that index.
#Generate: close({
	generate!: close({
		count!: uint
		item!:  #Item
	})
})

// A value in which an integer narrower than 64 bits may be a sequence.
#Item:
	#Value |
	close({u8!: #Sequence}) |
	close({u16!: #Sequence}) |
	close({u32!: #Sequence}) |
	close({i8!: #Sequence}) |
	close({i16!: #Sequence}) |
	close({i32!: #Sequence}) |
	close({array!: [...#Item]}) |
	close({tuple!: [...#Item]}) |
	close({option!: #Item}) |
	close({"struct"!: close({name!: string, fields!: [...[string, #Item]]})})

// `start + step * index`, then `modulo` (Euclidean) if it has one. That the
// numbers fit their type is checked when it's expanded, not here.
#Sequence: close({
	start!:  int
	step!:   int
	modulo?: int & >0
})

// `framings.json`: framing encode and decode, per framing, per payload size.
#Framings: close({
	$schema?:       string
	framings!:      [...#EncodingName]
	payload_sizes!: [...uint]

	// The measured frame: an `ok` response
	frame!: close({
		kind!:         "ok"
		request_id!:   #Unsigned
		protocol!:     uint16
		payload_fill!: string
	})
})

// `handshake.json`: the handshake frame.
#Handshake: close({
	$schema?: string
	frame!:   #HandshakeFrame
	ops!: [...("encode" | "decode" | "check")]
})

// `comline2.json`: the `comline@2` handshake's hello.
#Comline2Bench: close({
	$schema?: string
	hello!:   #Comline2Hello
	ops!: [...("encode" | "decode" | "answer")]
})

// `connection.json`: one connection step, a call and its reply.
#Connection: close({
	$schema?:  string
	framing!:  #EncodingName
	format!:   #EncodingName
	protocol!: uint16
	call!: close({
		id!:   uint16
		name!: string
	})

	// The id of the shape whose value is the params
	params!: string

	// An `ok`, with the hex `payload`
	reply!: close({
		kind!:    "ok"
		payload!: #Hex
	})
})
