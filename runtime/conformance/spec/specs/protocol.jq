# Holds a framing's `protocol` fact to the cases that prove it: `specs/check.sh`
# runs this on each framing profile ($profile), slurping the framing vectors
# (`.` is the array of them), and fails on any line it prints.
#
#   - every case it names exists and is an exact `request` (not a rejection or
#     an `accept`), in a feature of the profile's own framing;
#   - `named`: the case's input has a `protocol` that is 128 or more, so its
#     varint takes two bytes or more, and is not the case's `call_id`, so the
#     bytes can't hold it as the function's number; and the bytes hold that
#     number's varint (LEB128), from a byte boundary. A request that holds
#     the protocol is one that names it;
#   - `unnamed`: the case's input has no `protocol`, and decoding its bytes
#     gives that input back, so the frame carries none. (A request can't be
#     shown to ignore a protocol it was never given: encoding what the input
#     holds is all a case asks.)
#
# That a `named` number is in the frame for every protocol, and where, is the
# framing's own vectors' to show: this holds the cases the profile names.

# A number's LEB128 bytes, as lowercase hex.
def hex2: "0123456789abcdef" as $d | $d[. / 16 | floor : (. / 16 | floor) + 1] + $d[. % 16 : (. % 16) + 1];
def varint: if . < 128 then [.] else [(. % 128) + 128] + ((. / 128 | floor) | varint) end;
def varint_hex: varint | map(hex2) | join("");

# Whether `needle` (hex) is in `hay` (hex) at a byte boundary.
def holds($needle): . as $hay | any(range(0; ($hay | length) / 2); . as $i | $hay[2 * $i : 2 * $i + ($needle | length)] == $needle);

$profile[0] as $p
| $p.facts.protocol as $f
| ([.[].cases[]] | map({key: .id, value: .}) | from_entries) as $cases
| if $f == null then "no `protocol` fact" else
  $f.proven_by[] as $id
  | $cases[$id] as $c
  | if $c == null then "protocol: no case \($id)"
    elif $c.op != "request" or ($c.expect // "exact") != "exact" then "protocol: \($id) isn't an exact request"
    elif $c.input.framing != $p.name then "protocol: \($id) is for \($c.input.framing), not \($p.name)"
    elif $f.value == "named" then
      if ($c.input.protocol // null) == null then "protocol: \($id) has no protocol to name"
      elif $c.input.protocol < 128 then "protocol: \($id)'s protocol is under 128, which a byte can hold by chance"
      elif $c.input.protocol == $c.input.call_id then "protocol: \($id)'s protocol is its call_id, so its bytes can't tell them apart"
      elif ($c.bytes | holds($c.input.protocol | varint_hex)) | not then "protocol: \($id)'s bytes don't hold its protocol \($c.input.protocol)"
      else empty end
    elif $f.value == "unnamed" then
      if $c.input | has("protocol") then "protocol: \($id) gives a protocol, which its frame doesn't carry"
      else empty end
    else "protocol: \($f.value) isn't a value this check knows" end
  end
