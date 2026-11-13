# Holds a compiler conformance file (`compiler/cases/<group>.json`) to what
# its schema (`#CompilerCases`) can't say: `specs/check.sh` runs this on
# each file, and fails on any line it prints.
#
#   - every case's id is `<group>/<name>`, and ids are unique;
#   - the case's package names are unique, `build` and every dependency name
#     one of them (not the package itself), and the dependencies have no cycle;
#   - a package's schema paths are unique, and its `config` declares each
#     dependency by path (`../<name>`), and accepts what `accept_unverified`
#     says;
#   - `expect` names each protocol of the package built, once, and no
#     others; a protocol's given-up guarantees and its acknowledged ones are
#     sets, and only those given up are acknowledged;
#   - an acknowledgement's reason is written in the package built;
#   - diagnostics are unique;
#   - a build that fails says why: an error with a code, or text it contains.

def protocols_of($package):
  [$package.schemas[].text | scan("(?m)^protocol ([A-Za-z0-9_]+)") | .[0]];

def duplicates: group_by(.) | map(select(length > 1) | .[0]);

# Packages that remain after dropping the ones whose dependencies are all gone,
# until none is left or none can go: the cycle, if any.
def stuck($packages):
  reduce range(0; $packages | length + 1) as $_ (
    $packages;
    . as $left
    | ($left | map(.name)) as $names
    | map(select(any(.dependencies[].name; . as $d | $names | index($d))))
  );

. as $file
| .cases[]
| . as $case
| ($case.id) as $id
| (
    (if ($id | startswith($file.group + "/")) then empty else "\($id): not in group \($file.group)" end),
    ($case.packages | map(.name)) as $names
    | (
        ($names | duplicates | map("\($id): package \(.) twice") | .[]),
        (if ($names | index($case.build)) then empty else "\($id): builds \($case.build), which is no package" end),
        ($case.packages[] | . as $p
          | (
              ($p.dependencies[] | .name as $d
                | (if $d == $p.name then "\($id): \($p.name) depends on itself"
                   elif ($names | index($d)) then empty
                   else "\($id): \($p.name) depends on \($d), which is no package" end),
                  (if ($p.config | contains("path = \"../\($d)\"")) then empty
                   else "\($id): \($p.name)'s config doesn't declare \($d) by path" end),
                  ((.accept_unverified // {}) | to_entries[]
                   | (.key as $protocol | .value as $ids
                      | (if ($p.config | contains("\($protocol) = [" + ($ids | map("\"\(.)\"") | join(", ")) + "]"))
                         then empty
                         else "\($id): \($p.name)'s config doesn't accept \($protocol) for \($ids | join(", "))" end),
                        (if ($ids | length) == ($ids | unique | length) then empty
                         else "\($id): \($p.name) accepts a guarantee of \($protocol) twice" end)))),
              ($p.schemas | map(.path) | duplicates | map("\($id): \($p.name) has \(.) twice") | .[])
            )),
        (if (stuck($case.packages) | length) > 0
         then "\($id): packages depend on each other in a cycle: \(stuck($case.packages) | map(.name) | join(", "))"
         else empty end),
        ($case.packages[] | select(.name == $case.build) as $built
          | $case.expect as $expect
          | (
              ($expect.diagnostics | map(.code) | duplicates | map("\($id): diagnostic \(.) twice") | .[]),
              (if $expect.builds then
                 (protocols_of($built) | sort) as $declared
                 | ($expect.protocols | map(.name) | sort) as $listed
                 | (if $declared == $listed then empty
                    else "\($id): expects protocols \($listed | join(", ")), and \($built.name) declares \($declared | join(", "))" end),
                   ($expect.protocols[]
                    | . as $protocol
                    | (if (.given_up | length) == (.given_up | unique | length) then empty
                       else "\($id): \($protocol.name) gives up a guarantee twice" end),
                      ((.acknowledged // []) | map(.guarantee) as $acknowledged
                       | (if ($acknowledged | length) == ($acknowledged | unique | length) then empty
                          else "\($id): \($protocol.name) acknowledges a guarantee twice" end),
                         ($acknowledged[] | select(. as $g | $protocol.given_up | index($g) | not)
                          | "\($id): \($protocol.name) acknowledges \(.), which it doesn't give up")),
                      ((.acknowledged // [])[] | select(.reason != null)
                       | . as $a
                       | if ($built.schemas | any(.text | contains($a.reason))) or ($built.config | contains($a.reason)) then empty
                         else "\($id): \($protocol.name)'s reason for \($a.guarantee) isn't written in \($built.name)" end))
               else
                 (if ($expect.diagnostics | length) > 0 or (($expect.error_contains // []) | length) > 0 then empty
                  else "\($id): fails and says neither an error nor text it contains" end)
               end)
            ))
      )
  )
