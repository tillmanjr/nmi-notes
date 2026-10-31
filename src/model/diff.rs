// SPDX-License-Identifier: AGPL-3.0-or-later

//! Single-range diff over UTF-16 text, used to turn an editor's full text
//! into one `Edit` op, and to keep carets stable when remote ops arrive.

/// Replace `old_len` units at `start` with `new_len` units (taken from the new
/// text at the same `start`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Splice16 {
    pub start: usize,
    pub old_len: usize,
    pub new_len: usize,
}

fn is_high_surrogate(u: u16) -> bool {
    (0xD800..0xDC00).contains(&u)
}

fn is_low_surrogate(u: u16) -> bool {
    (0xDC00..0xE000).contains(&u)
}

/// Smallest single splice turning `old` into `new`, or `None` if equal.
/// Never splits a surrogate pair.
pub fn diff_utf16(old: &[u16], new: &[u16]) -> Option<Splice16> {
    if old == new {
        return None;
    }
    let mut prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if prefix > 0 && is_high_surrogate(old[prefix - 1]) {
        prefix -= 1;
    }
    let max_suffix = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    if suffix > 0 && is_low_surrogate(old[old.len() - suffix]) {
        suffix -= 1;
    }
    Some(Splice16 {
        start: prefix,
        old_len: old.len() - prefix - suffix,
        new_len: new.len() - prefix - suffix,
    })
}

/// Applies a splice whose inserted units are `inserted`.
pub fn apply_utf16(old: &[u16], start: usize, old_len: usize, inserted: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(old.len() - old_len + inserted.len());
    out.extend_from_slice(&old[..start]);
    out.extend_from_slice(inserted);
    out.extend_from_slice(&old[start + old_len..]);
    out
}

/// Where a caret at `pos` ends up after the splice. Positions inside the
/// replaced range move to the end of the inserted text.
pub fn shift_offset(pos: usize, start: usize, old_len: usize, new_len: usize) -> usize {
    if pos <= start {
        pos
    } else if pos >= start + old_len {
        pos - old_len + new_len
    } else {
        start + new_len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn u(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn equal_is_none() {
        assert_eq!(diff_utf16(&u("abc"), &u("abc")), None);
    }

    #[test]
    fn insert_delete_replace() {
        assert_eq!(
            diff_utf16(&u("abc"), &u("abXc")),
            Some(Splice16 { start: 2, old_len: 0, new_len: 1 })
        );
        assert_eq!(
            diff_utf16(&u("abc"), &u("ac")),
            Some(Splice16 { start: 1, old_len: 1, new_len: 0 })
        );
        assert_eq!(
            diff_utf16(&u("hello world"), &u("hello there")),
            Some(Splice16 { start: 6, old_len: 5, new_len: 5 })
        );
        assert_eq!(
            diff_utf16(&u("aaa"), &u("aaaa")),
            Some(Splice16 { start: 3, old_len: 0, new_len: 1 })
        );
    }

    #[test]
    fn does_not_split_surrogates() {
        // 😀 = D83D DE00, 😁 = D83D DE01: shared high surrogate.
        let s = diff_utf16(&u("x😀y"), &u("x😁y")).unwrap();
        assert_eq!(s, Splice16 { start: 1, old_len: 2, new_len: 2 });
        // shared low surrogate: 😀 (D83D DE00) vs 𐘀 (D801 DE00)
        let s = diff_utf16(&u("x😀y"), &u("x\u{10600}y")).unwrap();
        assert_eq!(s, Splice16 { start: 1, old_len: 2, new_len: 2 });
    }

    #[test]
    fn shift_offsets() {
        // replace [2,5) with 1 unit
        assert_eq!(shift_offset(1, 2, 3, 1), 1);
        assert_eq!(shift_offset(2, 2, 3, 1), 2);
        assert_eq!(shift_offset(4, 2, 3, 1), 3);
        assert_eq!(shift_offset(5, 2, 3, 1), 3);
        assert_eq!(shift_offset(9, 2, 3, 1), 7);
    }

    proptest! {
        #[test]
        fn roundtrip(old in "\\PC{0,40}", new in "\\PC{0,40}") {
            let (o, n) = (u(&old), u(&new));
            match diff_utf16(&o, &n) {
                None => prop_assert_eq!(&o, &n),
                Some(s) => {
                    let ins = &n[s.start..s.start + s.new_len];
                    prop_assert_eq!(apply_utf16(&o, s.start, s.old_len, ins), n.clone());
                    // Splice boundaries must be valid UTF-16 on both sides.
                    prop_assert!(String::from_utf16(&o[..s.start]).is_ok());
                    prop_assert!(String::from_utf16(&o[s.start..s.start + s.old_len]).is_ok());
                    prop_assert!(String::from_utf16(ins).is_ok());
                }
            }
        }

        #[test]
        fn shifted_offset_stays_in_bounds(len in 0usize..50, a in 0usize..50, b in 0usize..50, new_len in 0usize..20, pos in 0usize..50) {
            let start = a.min(len);
            let old_len = b.min(len - start);
            let pos = pos.min(len);
            let shifted = shift_offset(pos, start, old_len, new_len);
            prop_assert!(shifted <= len - old_len + new_len);
        }
    }
}
