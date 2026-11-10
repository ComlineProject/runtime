# Holds a format's size rules to its size vectors: `specs/check.sh` runs this
# with the format's profile ($profile) and the vectors of the feature its
# `sizes` name ($vectors), and fails on any line it prints.
#
#   - every rule lists cases that exist, in that feature, for that format, and
#     whose type has the rule's kind of value in it;
#   - a scalar's rule is proven by the case whose type is that scalar alone,
#     and states the same bounds as the case;
#   - every case proves some rule;
#   - each witness is as long as its case says: `min` and `max` exactly, a
#     `between` within them; `max` has a witness unless it is `null` or an
#     upper bound.
#   - the facts and the sizes agree on how structs and enums are written.

# The kinds of value a type is built of: its scalars' and constructors' names.
def kinds:
  if type == "string" then [.]
  else
    to_entries[0] as $e
    | [$e.key] + (
      if $e.key == "option" or $e.key == "array" then ($e.value | kinds)
      elif $e.key == "fixed" or $e.key == "bounded_array" or $e.key == "recursive" then ($e.value.of | kinds)
      elif $e.key == "struct" then ([$e.value.fields[][1] | kinds] | add // [])
      else [] end)
  end;

# A scalar as a type spells it: `s8` is `i8`.
def as_type: if test("^s[0-9]+$") then "i" + .[1:] else . end;

def has_kind($ks; $k): $ks | any(. == $k);

# Whether a type with these kinds shows the rule.
def shows($rule; $ks):
  if ($rule | startswith("scalars.")) then has_kind($ks; $rule | ltrimstr("scalars.") | as_type)
  elif $rule == "string" then ($ks | any(. == "string" or . == "bounded_string" or . == "fixed_string"))
  elif $rule == "array" then ($ks | any(. == "array" or . == "bounded_array"))
  elif $rule == "fixed_array" then has_kind($ks; "fixed")
  else has_kind($ks; $rule) end;

def bytes_len: (.bytes | length) / 2;

$profile[0] as $p
| $vectors[0] as $v
| $p.sizes as $s
| ([$s.scalars | to_entries[] | {rule: ("scalars." + .key), rule_def: .value}]
   + [("string", "array", "fixed_array", "option", "struct", "enum") as $k | {rule: $k, rule_def: $s[$k]}]) as $rules
| ($v.cases | map({key: .id, value: .}) | from_entries) as $cases
| ([$s.scalars | to_entries[] | select(.value.upper_bound == true) | .key | as_type]) as $upper_types
| (
    # Rules and the cases that prove them.
    ($rules[] as $r
     | $r.rule_def.proven_by[] as $id
     | ($cases[$id]) as $c
     | if $c == null then "\($r.rule): no case \($id) in \($s.feature)"
       elif ($id | startswith($s.feature + "/")) | not then "\($r.rule): \($id) isn't in \($s.feature)"
       elif $c.input.format != $p.name then "\($r.rule): \($id) is for \($c.input.format), not \($p.name)"
       elif shows($r.rule; $c.input.type | kinds) | not then "\($r.rule): \($id) has none of it in its type"
       elif ($r.rule | startswith("scalars.")) and ($c.input.type != ($r.rule | ltrimstr("scalars.") | as_type)) then
         "\($r.rule): \($id) isn't the scalar alone"
       elif ($r.rule | startswith("scalars."))
            and ([$c.input.min, $c.input.max, ($c.input.upper_bound // false)] != [$r.rule_def.min, $r.rule_def.max, ($r.rule_def.upper_bound // false)]) then
         "\($r.rule): \($id) says [\($c.input.min), \($c.input.max)], the rule [\($r.rule_def.min), \($r.rule_def.max)]"
       else empty end),
    # Every case proves something.
    ($v.cases[] as $c
     | select([$rules[].rule_def.proven_by[]] | any(. == $c.id) | not)
     | "\($c.id): proves no rule"),
    # Witnesses.
    ($v.cases[] as $c
     | $c.input as $i
     | ($i.type | kinds) as $ks
     | (if $c.op != "size" then "\($c.id): not a size case" else empty end),
       (if $i.format != $p.name then "\($c.id): for \($i.format), not \($p.name)" else empty end),
       (if $i.max != null and $i.max < $i.min then "\($c.id): max under min" else empty end),
       (if (($i.upper_bound // false) != ($ks | any(. as $k | $upper_types | any(. == $k)))) and ($i.format == $p.name)
        then "\($c.id): upper_bound doesn't follow the scalars' rules" else empty end),
       ($i.witnesses.min | select(bytes_len != $i.min) | "\($c.id): the min witness is \(bytes_len) bytes, not \($i.min)"),
       (if $i.max == null or ($i.upper_bound // false) then
          (if $i.witnesses.max != null then "\($c.id): a max witness for a max no value is known to reach" else empty end)
        else
          ($i.witnesses.max // null) as $m
          | if $m == null then "\($c.id): no max witness"
            elif ($m | bytes_len) != $i.max then "\($c.id): the max witness is \($m | bytes_len) bytes, not \($i.max)"
            else empty end
        end),
       (($i.witnesses.between // [])[] | select(bytes_len < $i.min or ($i.max != null and bytes_len > $i.max))
         | "\($c.id): a between witness of \(bytes_len) bytes is outside [\($i.min), \($i.max)]"),
       (if ($i.max == null or ($i.max - $i.min) >= 2) and (($i.witnesses.between // []) | length) == 0
        then "\($c.id): no value between min and max" else empty end)),
    # Facts and sizes agree.
    (if (($p.facts.struct_fields.value == "by_name") != ($s.struct.key_extra != null)) then "struct: key_extra goes with struct_fields = by_name" else empty end),
    (if (($p.facts.enum_variants.value == "by_index") != ($s.enum.tag == "varint_index")) then "enum: varint_index goes with enum_variants = by_index" else empty end)
  )
