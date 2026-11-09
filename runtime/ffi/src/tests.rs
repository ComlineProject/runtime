/// The rule `disjoint!` checks, as plainly as it can be said: two
/// regions conflict when one is written, neither is empty, and each
/// starts before the other ends. A region is `(start, len, written)`.
fn conflict(a: (usize, usize, bool), b: (usize, usize, bool)) -> bool {
    let len = |(start, len, _): (usize, usize, bool)| if start == 0 { 0 } else { len };
    (a.2 || b.2)
        && len(a) != 0
        && len(b) != 0
        && a.0 < b.0.saturating_add(len(b))
        && b.0 < a.0.saturating_add(len(a))
}

/// Every three regions over a small address space (null included), and
/// lengths up to the end of the address space, against the plain rule,
/// with every split of the three into written and read. Not under Miri:
/// there's no `unsafe` to check, and too many cases.
#[test]
#[cfg_attr(miri, ignore)]
fn disjoint_is_the_plain_rule() {
    let starts = [0, 1, 2, 3, 4, 5, usize::MAX - 1, usize::MAX];
    let lens = [0, 1, 2, 3, usize::MAX];
    let mut all = Vec::new();
    for &start in &starts {
        for &len in &lens {
            all.push((start as *const u8, len));
        }
    }
    let plain = |regions: [(*const u8, usize); 3], writes: usize| {
        let r = |i: usize| (regions[i].0 as usize, regions[i].1, i < writes);
        !(conflict(r(0), r(1)) || conflict(r(0), r(2)) || conflict(r(1), r(2)))
    };
    for &a in &all {
        for &b in &all {
            for &c in &all {
                let at = [a, b, c];
                assert_eq!(
                    disjoint!(writes [a, b, c]),
                    plain(at, 3),
                    "{at:?}, 3 written"
                );
                assert_eq!(
                    disjoint!(writes [a, b], reads [c]),
                    plain(at, 2),
                    "{at:?}, 2 written"
                );
                assert_eq!(
                    disjoint!(writes [a], reads [b, c]),
                    plain(at, 1),
                    "{at:?}, 1 written"
                );
            }
        }
    }
}
