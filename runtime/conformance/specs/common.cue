package conformance

// Bytes as lowercase hex.
#Hex: =~"^([0-9a-f]{2})*$"

// An unsigned integer as a decimal string, as 64- and 128-bit integers are
// held, so a JSON reader that holds numbers as doubles keeps them exact.
#Unsigned: =~"^(0|[1-9][0-9]*)$"

// A signed integer as a decimal string.
#Signed: =~"^(0|-?[1-9][0-9]*)$"

// A vector case's id: `<feature>/<name>`.
#CaseId: =~"^[a-z0-9.@_-]+/[a-z0-9.@_-]+$"

// A wire format's or a framing's name, versioned or not, namespaced or not.
#EncodingName: =~"^[a-z0-9._-]+(@[0-9]+)?$"

// A profile's name: an encoding at a version.
#ProfileName: =~"^[a-z0-9._-]+@[0-9]+$"
