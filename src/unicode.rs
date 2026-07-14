//! Canonical Unicode normalization (NFC/NFD) and case-insensitive keys.
//!
//! This is a from-scratch, std-only implementation of UAX #15 canonical
//! normalization: full canonical decomposition, canonical reordering by
//! combining class, and canonical composition with the standard blocking
//! rule. Hangul syllables are handled algorithmically per the standard;
//! everything else is driven by tables generated from UnicodeData.txt
//! (see `unicode_data.rs`). Compatibility (NFKC/NFKD) mappings are out of
//! scope on purpose: filesystems never apply them.
//!
//! Why this matters here: macOS (HFS+, and APFS via most APIs) stores
//! filenames decomposed while Linux and Windows store whatever bytes they
//! were given. A file created as `café` on Linux and synced to a Mac comes
//! back with a different byte sequence — sync tools that compare bytes see
//! two files. Detecting "not NFC" and "NFC-equal but byte-different"
//! requires real normalization, not a lookup of a few accented letters.

use crate::unicode_data::{CCC, COMPOSE, DECOMP};

// Hangul algorithmic constants (UAX #15 §3.12).
const S_BASE: u32 = 0xAC00;
const L_BASE: u32 = 0x1100;
const V_BASE: u32 = 0x1161;
const T_BASE: u32 = 0x11A7;
const L_COUNT: u32 = 19;
const V_COUNT: u32 = 21;
const T_COUNT: u32 = 28;
const N_COUNT: u32 = V_COUNT * T_COUNT; // 588
const S_COUNT: u32 = L_COUNT * N_COUNT; // 11172

/// Canonical combining class of a code point (0 for starters).
pub fn combining_class(c: char) -> u8 {
    let cp = c as u32;
    let idx = CCC.partition_point(|&(start, _, _)| start <= cp);
    if idx == 0 {
        return 0;
    }
    let (start, end, ccc) = CCC[idx - 1];
    if cp >= start && cp <= end {
        ccc
    } else {
        0
    }
}

/// Canonical decomposition of one code point, pushed onto `out`.
/// Recurses because decompositions can themselves decompose
/// (e.g. U+1E17 -> U+0113 -> U+0065 U+0304).
fn decompose_char(c: char, out: &mut Vec<char>) {
    let cp = c as u32;
    // Hangul syllable: algorithmic decomposition to L V (T).
    if (S_BASE..S_BASE + S_COUNT).contains(&cp) {
        let s_index = cp - S_BASE;
        let l = L_BASE + s_index / N_COUNT;
        let v = V_BASE + (s_index % N_COUNT) / T_COUNT;
        let t = T_BASE + s_index % T_COUNT;
        out.push(char::from_u32(l).unwrap());
        out.push(char::from_u32(v).unwrap());
        if t != T_BASE {
            out.push(char::from_u32(t).unwrap());
        }
        return;
    }
    if let Ok(idx) = DECOMP.binary_search_by_key(&cp, |&(k, _, _)| k) {
        let (_, a, b) = DECOMP[idx];
        decompose_char(char::from_u32(a).unwrap(), out);
        if b != 0 {
            decompose_char(char::from_u32(b).unwrap(), out);
        }
        return;
    }
    out.push(c);
}

/// Canonical reordering: stable-sort each run of non-starters by combining
/// class (UAX #15 canonical ordering algorithm). Runs are short in practice,
/// so a simple insertion sort keeps this allocation-free and stable.
fn canonical_reorder(chars: &mut [char]) {
    let mut i = 1;
    while i < chars.len() {
        let ccc = combining_class(chars[i]);
        if ccc != 0 {
            let mut j = i;
            while j > 0 && combining_class(chars[j - 1]) > ccc {
                chars.swap(j - 1, j);
                j -= 1;
            }
        }
        i += 1;
    }
}

/// Primary composite for a starter + combining pair, if any.
fn compose_pair(starter: char, combining: char) -> Option<char> {
    let (a, b) = (starter as u32, combining as u32);
    // Hangul L+V and LV+T composition is algorithmic.
    if (L_BASE..L_BASE + L_COUNT).contains(&a) && (V_BASE..V_BASE + V_COUNT).contains(&b) {
        let lv = S_BASE + (a - L_BASE) * N_COUNT + (b - V_BASE) * T_COUNT;
        return char::from_u32(lv);
    }
    if (S_BASE..S_BASE + S_COUNT).contains(&a)
        && (a - S_BASE) % T_COUNT == 0
        && (T_BASE + 1..T_BASE + T_COUNT).contains(&b)
    {
        return char::from_u32(a + (b - T_BASE));
    }
    COMPOSE
        .binary_search_by_key(&(a, b), |&(x, y, _)| (x, y))
        .ok()
        .map(|idx| char::from_u32(COMPOSE[idx].2).unwrap())
}

/// Canonical decomposition (NFD) of a string.
pub fn to_nfd(s: &str) -> String {
    let mut chars: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars() {
        decompose_char(c, &mut chars);
    }
    canonical_reorder(&mut chars);
    chars.into_iter().collect()
}

/// Canonical composition (NFC) of a string: full decomposition, canonical
/// reordering, then the UAX #15 composition pass with blocking.
pub fn to_nfc(s: &str) -> String {
    let mut chars: Vec<char> = Vec::with_capacity(s.len());
    for c in s.chars() {
        decompose_char(c, &mut chars);
    }
    canonical_reorder(&mut chars);

    // Composition pass. `starter_idx` tracks the last starter written to
    // `out`; a combining mark composes with it unless blocked by an
    // intervening character of greater-or-equal combining class.
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut starter_idx: Option<usize> = None;
    let mut last_ccc: u8 = 0;
    for &c in &chars {
        let ccc = combining_class(c);
        if let Some(si) = starter_idx {
            let blocked = last_ccc != 0 && last_ccc >= ccc;
            if !blocked {
                if let Some(composed) = compose_pair(out[si], c) {
                    out[si] = composed;
                    // last_ccc keeps tracking the last uncomposed mark.
                    continue;
                }
            }
        }
        if ccc == 0 {
            starter_idx = Some(out.len());
            last_ccc = 0;
        } else {
            last_ccc = ccc;
        }
        out.push(c);
    }
    out.into_iter().collect()
}

/// True when the string is already in NFC form (byte-identical to its NFC).
pub fn is_nfc(s: &str) -> bool {
    // Fast path: pure ASCII is always NFC.
    if s.is_ascii() {
        return true;
    }
    to_nfc(s) == s
}

/// Case-insensitive comparison key approximating how Windows (NTFS) and
/// macOS (APFS/HFS+) match names: canonical normalization first, then full
/// Unicode lowercasing (std's `char::to_lowercase`, which handles one-to-many
/// mappings like U+0130). Two names with equal keys collide on a
/// case-insensitive volume.
pub fn casefold_key(s: &str) -> String {
    to_nfc(s).chars().flat_map(|c| c.to_lowercase()).collect()
}

/// Length of the string in UTF-16 code units — what NTFS and the Win32 API
/// count against the 255-unit component limit.
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(|c| c.len_utf16()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_untouched_and_nfc() {
        assert_eq!(to_nfc("report_2024.txt"), "report_2024.txt");
        assert!(is_nfc("report_2024.txt"));
    }

    #[test]
    fn latin_nfd_composes_to_nfc() {
        // "café" with a decomposed é (e + COMBINING ACUTE ACCENT), the exact
        // form HFS+ stores and rsync/Syncthing ship back to Linux.
        let nfd = "cafe\u{0301}";
        assert_eq!(to_nfc(nfd), "caf\u{00E9}");
        assert!(!is_nfc(nfd));
        assert!(is_nfc("caf\u{00E9}"));
    }

    #[test]
    fn nfc_decomposes_to_nfd() {
        assert_eq!(to_nfd("caf\u{00E9}"), "cafe\u{0301}");
        assert_eq!(to_nfd("\u{1E17}"), "e\u{0304}\u{0301}"); // ḗ -> e + macron + acute
    }

    #[test]
    fn multi_mark_recursive_decomposition_recomposes() {
        // U+1EC7 ệ fully decomposes to e + circumflex + dot-below (reordered
        // to dot-below first, ccc 220 < 230) and must recompose to itself.
        let nfd = to_nfd("\u{1EC7}");
        assert_eq!(nfd, "e\u{0323}\u{0302}");
        assert_eq!(to_nfc(&nfd), "\u{1EC7}");
    }

    #[test]
    fn unordered_marks_are_canonically_reordered() {
        // Same marks in the "wrong" order still normalize identically.
        assert_eq!(to_nfc("e\u{0302}\u{0323}"), to_nfc("e\u{0323}\u{0302}"));
    }

    #[test]
    fn composition_stops_where_the_standard_says() {
        // e + dot-below(220) + acute(230): dot-below composes to ẹ; the acute
        // is not blocked (220 < 230) but (ẹ, acute) has no primary composite,
        // so it stays a bare mark. Verified against Python unicodedata.
        let s = "e\u{0323}\u{0301}";
        assert_eq!(to_nfc(s), "\u{1EB9}\u{0301}");
        // e + acute + acute: the first acute composes to é; the second finds
        // no (é, acute) composite and must survive as a mark, not vanish.
        let s2 = "e\u{0301}\u{0301}";
        assert_eq!(to_nfc(s2), "\u{00E9}\u{0301}");
    }

    #[test]
    fn singleton_decompositions_normalize() {
        // U+212B ANGSTROM SIGN normalizes to U+00C5 in both NFC and NFD paths.
        assert_eq!(to_nfc("\u{212B}"), "\u{00C5}");
        assert!(!is_nfc("\u{212B}"));
    }

    #[test]
    fn composition_exclusions_stay_decomposed() {
        // U+0958 DEVANAGARI QA is a composition exclusion: NFC keeps it
        // decomposed as U+0915 + U+093C.
        assert_eq!(to_nfc("\u{0958}"), "\u{0915}\u{093C}");
        assert_eq!(to_nfd("\u{0958}"), "\u{0915}\u{093C}");
    }

    #[test]
    fn hangul_composes_and_decomposes_algorithmically() {
        // 한 (U+D55C) = ᄒ U+1112 + ᅡ U+1161 + ᆫ U+11AB.
        assert_eq!(to_nfc("\u{1112}\u{1161}\u{11AB}"), "\u{D55C}");
        assert_eq!(to_nfd("\u{D55C}"), "\u{1112}\u{1161}\u{11AB}");
        // LV syllable without trailing consonant: 하 U+D558.
        assert_eq!(to_nfc("\u{1112}\u{1161}"), "\u{D558}");
    }

    #[test]
    fn kana_voiced_marks_compose() {
        // か + U+3099 (combining voiced sound mark) -> が, the classic
        // Japanese NFD filename difference between macOS and Linux.
        assert_eq!(to_nfc("\u{304B}\u{3099}"), "\u{304C}");
        assert_eq!(to_nfd("\u{30D7}"), "\u{30D5}\u{309A}"); // プ -> フ + handakuten
    }

    #[test]
    fn nfc_and_nfd_are_idempotent() {
        for s in ["caf\u{00E9}", "e\u{0323}\u{0302}", "\u{D55C}\u{304C}", "x"] {
            assert_eq!(to_nfc(&to_nfc(s)), to_nfc(s));
            assert_eq!(to_nfd(&to_nfd(s)), to_nfd(s));
            assert_eq!(to_nfc(&to_nfd(s)), to_nfc(s));
        }
    }

    #[test]
    fn combining_class_lookup() {
        assert_eq!(combining_class('a'), 0);
        assert_eq!(combining_class('\u{0301}'), 230); // acute above
        assert_eq!(combining_class('\u{0323}'), 220); // dot below
        assert_eq!(combining_class('\u{3099}'), 8); // kana voicing
    }

    #[test]
    fn casefold_key_merges_case_and_normalization() {
        // É (NFC), É (NFD), and é all collide on a case-insensitive volume.
        let a = casefold_key("R\u{00C9}SUM\u{00C9}.doc");
        let b = casefold_key("re\u{0301}sume\u{0301}.DOC");
        let c = casefold_key("r\u{00E9}sum\u{00E9}.doc");
        assert_eq!(a, b);
        assert_eq!(b, c);
        assert_ne!(casefold_key("resume.doc"), c);
    }

    #[test]
    fn casefold_handles_one_to_many_mappings() {
        // U+0130 LATIN CAPITAL LETTER I WITH DOT ABOVE lowercases to i + dot
        // above — a two-character mapping that a naive per-char map misses.
        assert_eq!(casefold_key("\u{0130}"), "i\u{0307}");
    }

    #[test]
    fn utf16_length_counts_surrogate_pairs() {
        assert_eq!(utf16_len("abc"), 3);
        assert_eq!(utf16_len("\u{1F600}"), 2); // emoji = surrogate pair
        assert_eq!(utf16_len("\u{00E9}"), 1);
    }
}
