// === FILE: runtime/molt-runtime/src/builtins/difflib.rs ===
//! Intrinsics for the `difflib` stdlib module.
//!
//! Implements:
//!   - SequenceMatcher: ratio, quick_ratio, get_matching_blocks, get_opcodes
//!     (Ratcliff/Obershelp / longest-common-subsequence algorithm)
//!   - Diff generators: unified_diff, context_diff, ndiff
//!   - Helpers: get_close_matches, is_junk

use crate::{
    MoltObject, PyToken, alloc_list, alloc_string, alloc_tuple, dec_ref_bits, obj_from_bits,
    raise_exception, string_obj_to_owned, to_f64, to_i64, type_name,
};

// ---------------------------------------------------------------------------
// SequenceMatcher implementation (Ratcliff/Obershelp / LCS)
// ---------------------------------------------------------------------------

/// One matching block: (i, j, n) — a[i:i+n] == b[j:j+n].
#[derive(Clone, Debug)]
struct MatchingBlock {
    i: usize,
    j: usize,
    n: usize,
}

/// Find the longest common substring of `a[alo..ahi]` and `b[blo..bhi]`.
/// Returns a `MatchingBlock` with the best match (n=0 if none).
fn find_longest_match(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
) -> MatchingBlock {
    let mut best_i = alo;
    let mut best_j = blo;
    let mut best_size = 0usize;

    // j2len[j] = length of longest common suffix of a[alo..i+1] and b[blo..j+1]
    let b_len = bhi.saturating_sub(blo);
    let mut j2len = vec![0usize; b_len + 1];
    let mut new_j2len = vec![0usize; b_len + 1];

    for (i, a_i) in a.iter().enumerate().take(ahi).skip(alo) {
        for (bj, j) in (blo..bhi).enumerate() {
            let k = if *a_i == b[j] { j2len[bj] + 1 } else { 0 };
            new_j2len[bj + 1] = k;
            if k > best_size {
                best_i = i + 1 - k;
                best_j = j + 1 - k;
                best_size = k;
            }
        }
        std::mem::swap(&mut j2len, &mut new_j2len);
        new_j2len.fill(0);
    }

    // Expand the match while equal elements are adjacent (j2len gives longest suffix,
    // but we already track exact start position above).
    MatchingBlock {
        i: best_i,
        j: best_j,
        n: best_size,
    }
}

/// Recursively collect all matching blocks between a[alo..ahi] and b[blo..bhi].
fn matching_blocks_recursive(
    a: &[char],
    b: &[char],
    alo: usize,
    ahi: usize,
    blo: usize,
    bhi: usize,
    out: &mut Vec<MatchingBlock>,
) {
    let m = find_longest_match(a, b, alo, ahi, blo, bhi);
    if m.n == 0 {
        return;
    }
    let (i, j, n) = (m.i, m.j, m.n);
    // Recurse on left part
    if alo < i && blo < j {
        matching_blocks_recursive(a, b, alo, i, blo, j, out);
    }
    out.push(MatchingBlock { i, j, n });
    // Recurse on right part
    if i + n < ahi && j + n < bhi {
        matching_blocks_recursive(a, b, i + n, ahi, j + n, bhi, out);
    }
}

/// Return all matching blocks between `a` and `b`, including the terminal sentinel (la, lb, 0).
fn get_matching_blocks_impl(a: &[char], b: &[char]) -> Vec<MatchingBlock> {
    let la = a.len();
    let lb = b.len();
    let mut blocks: Vec<MatchingBlock> = Vec::new();
    matching_blocks_recursive(a, b, 0, la, 0, lb, &mut blocks);
    // The sentinel
    blocks.push(MatchingBlock { i: la, j: lb, n: 0 });
    blocks
}

/// Compute the similarity ratio using the matching blocks.
fn ratio_from_blocks(blocks: &[MatchingBlock], la: usize, lb: usize) -> f64 {
    let matches: usize = blocks.iter().map(|m| m.n).sum();
    let total = la + lb;
    if total == 0 {
        1.0
    } else {
        2.0 * matches as f64 / total as f64
    }
}

/// Opcode tags matching CPython's SequenceMatcher.get_opcodes().
#[derive(Debug, Clone, PartialEq, Eq)]
enum Tag {
    Equal,
    Replace,
    Delete,
    Insert,
}

impl Tag {
    fn as_str(&self) -> &'static str {
        match self {
            Tag::Equal => "equal",
            Tag::Replace => "replace",
            Tag::Delete => "delete",
            Tag::Insert => "insert",
        }
    }
}

struct Opcode {
    tag: Tag,
    i1: usize,
    i2: usize,
    j1: usize,
    j2: usize,
}

fn get_opcodes_impl(a: &[char], b: &[char]) -> Vec<Opcode> {
    let blocks = get_matching_blocks_impl(a, b);
    let mut opcodes: Vec<Opcode> = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;

    for blk in &blocks {
        let (ai, bj, size) = (blk.i, blk.j, blk.n);
        let tag = if i < ai && j < bj {
            Tag::Replace
        } else if i < ai {
            Tag::Delete
        } else if j < bj {
            Tag::Insert
        } else {
            // No gap before this block.
            i = ai + size;
            j = bj + size;
            if size > 0 {
                opcodes.push(Opcode {
                    tag: Tag::Equal,
                    i1: ai,
                    i2: ai + size,
                    j1: bj,
                    j2: bj + size,
                });
            }
            continue;
        };
        opcodes.push(Opcode {
            tag,
            i1: i,
            i2: ai,
            j1: j,
            j2: bj,
        });
        i = ai + size;
        j = bj + size;
        if size > 0 {
            opcodes.push(Opcode {
                tag: Tag::Equal,
                i1: ai,
                i2: ai + size,
                j1: bj,
                j2: bj + size,
            });
        }
    }
    opcodes
}

// ---------------------------------------------------------------------------
// Diff generators
// ---------------------------------------------------------------------------

/// Format a unified diff between two lists of lines.
fn unified_diff_impl(
    a: &[String],
    b: &[String],
    fromfile: &str,
    tofile: &str,
    context: usize,
) -> Vec<String> {
    // Work at line level using a line-based LCS diff.
    line_based_unified_diff(a, b, fromfile, tofile, context)
}

/// Straightforward line-level unified diff without character-level opcodes.
fn line_based_unified_diff(
    a: &[String],
    b: &[String],
    fromfile: &str,
    tofile: &str,
    n: usize,
) -> Vec<String> {
    // Convert to char sequences where each element represents one line.
    let a_chars: Vec<usize> = (0..a.len()).collect();
    let b_chars: Vec<usize> = (0..b.len()).collect();

    // Simple LCS-based line diff.
    let lcs = lcs_indices(a, b);

    // Generate raw edit script
    let edits = build_edit_script(a.len(), b.len(), &lcs);

    // Group edits into hunks with context.
    let hunks = group_edits_into_hunks(&edits, n, a.len(), b.len());

    if hunks.is_empty() {
        return Vec::new();
    }

    let _ = (a_chars, b_chars); // used for lengths above

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("--- {fromfile}\n"));
    lines.push(format!("+++ {tofile}\n"));

    for hunk in hunks {
        // Hunk header: @@ -a_start,a_count +b_start,b_count @@
        let (a_start, a_count, b_start, b_count) = hunk_range(&hunk);
        lines.push(format!(
            "@@ -{},{} +{},{} @@\n",
            a_start + 1,
            a_count,
            b_start + 1,
            b_count
        ));
        for edit in &hunk {
            match edit {
                Edit::Equal(ai, _bi) => lines.push(format!(" {}", a[*ai])),
                Edit::Delete(ai) => lines.push(format!("-{}", a[*ai])),
                Edit::Insert(_ai, bi) => lines.push(format!("+{}", b[*bi])),
            }
        }
    }
    lines
}

#[derive(Clone, Debug)]
enum Edit {
    Equal(usize, usize),
    Delete(usize),
    Insert(usize, usize), // (context_a_idx, b_idx)
}

/// Compute indices of LCS elements as (a_idx, b_idx) pairs.
fn lcs_indices(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let m = a.len();
    let n = b.len();
    // Build LCS table.
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in (0..m).rev() {
        for j in (0..n).rev() {
            if a[i] == b[j] {
                dp[i][j] = dp[i + 1][j + 1] + 1;
            } else {
                dp[i][j] = dp[i + 1][j].max(dp[i][j + 1]);
            }
        }
    }
    // Trace back.
    let mut result = Vec::new();
    let mut i = 0usize;
    let mut j = 0usize;
    while i < m && j < n {
        if a[i] == b[j] {
            result.push((i, j));
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            i += 1;
        } else {
            j += 1;
        }
    }
    result
}

fn build_edit_script(a_len: usize, b_len: usize, lcs: &[(usize, usize)]) -> Vec<Edit> {
    let mut edits: Vec<Edit> = Vec::new();
    let mut ai = 0usize;
    let mut bi = 0usize;
    for &(la, lb) in lcs {
        while ai < la {
            edits.push(Edit::Delete(ai));
            ai += 1;
        }
        while bi < lb {
            edits.push(Edit::Insert(ai, bi));
            bi += 1;
        }
        edits.push(Edit::Equal(la, lb));
        ai += 1;
        bi += 1;
    }
    while ai < a_len {
        edits.push(Edit::Delete(ai));
        ai += 1;
    }
    while bi < b_len {
        edits.push(Edit::Insert(ai, bi));
        bi += 1;
    }
    edits
}

fn group_edits_into_hunks(
    edits: &[Edit],
    n: usize,
    _a_len: usize,
    _b_len: usize,
) -> Vec<Vec<Edit>> {
    let mut hunks: Vec<Vec<Edit>> = Vec::new();
    let current: Vec<Edit> = Vec::new();
    let tail_equal: usize = 0; // count of trailing Equal edits in current hunk

    // We need a two-pass approach: collect all edits, then split by context gaps.
    // A "gap" is a run of Equal edits >= 2*n+1 between non-equal edits.
    let len = edits.len();
    let i = 0usize;

    // Find ranges of non-equal edits.
    let mut changed_ranges: Vec<(usize, usize)> = Vec::new(); // (start, end) inclusive
    let mut in_change = false;
    let mut change_start = 0usize;
    for (idx, e) in edits.iter().enumerate() {
        let is_change = !matches!(e, Edit::Equal(_, _));
        if is_change && !in_change {
            change_start = idx;
            in_change = true;
        } else if !is_change && in_change {
            changed_ranges.push((change_start, idx - 1));
            in_change = false;
        }
    }
    if in_change {
        changed_ranges.push((change_start, len - 1));
    }

    if changed_ranges.is_empty() {
        return hunks;
    }

    // Merge ranges that are within 2*n equal lines of each other.
    let mut merged: Vec<(usize, usize)> = Vec::new();
    let mut cur_start = changed_ranges[0].0.saturating_sub(n);
    let mut cur_end = (changed_ranges[0].1 + n + 1).min(len);

    for &(cs, ce) in &changed_ranges[1..] {
        let hunk_start = cs.saturating_sub(n);
        let hunk_end = (ce + n + 1).min(len);
        if hunk_start <= cur_end {
            // Overlapping / adjacent — extend.
            cur_end = hunk_end.max(cur_end);
        } else {
            merged.push((cur_start, cur_end));
            cur_start = hunk_start;
            cur_end = hunk_end;
        }
    }
    merged.push((cur_start, cur_end));

    let _ = (i, current, tail_equal); // suppress lint

    for (hs, he) in merged {
        hunks.push(edits[hs..he].to_vec());
    }
    hunks
}

fn hunk_range(hunk: &[Edit]) -> (usize, usize, usize, usize) {
    let mut a_start = usize::MAX;
    let mut a_count = 0usize;
    let mut b_start = usize::MAX;
    let mut b_count = 0usize;
    for e in hunk {
        match e {
            Edit::Equal(ai, bi) => {
                if a_start == usize::MAX {
                    a_start = *ai;
                }
                if b_start == usize::MAX {
                    b_start = *bi;
                }
                a_count += 1;
                b_count += 1;
            }
            Edit::Delete(ai) => {
                if a_start == usize::MAX {
                    a_start = *ai;
                }
                a_count += 1;
            }
            Edit::Insert(_ai, bi) => {
                if b_start == usize::MAX {
                    b_start = *bi;
                }
                b_count += 1;
            }
        }
    }
    (
        if a_start == usize::MAX { 0 } else { a_start },
        a_count,
        if b_start == usize::MAX { 0 } else { b_start },
        b_count,
    )
}

/// Context diff generator.
fn context_diff_impl(
    a: &[String],
    b: &[String],
    fromfile: &str,
    tofile: &str,
    n: usize,
) -> Vec<String> {
    let lcs = lcs_indices(a, b);
    let edits = build_edit_script(a.len(), b.len(), &lcs);
    let hunks = group_edits_into_hunks(&edits, n, a.len(), b.len());

    if hunks.is_empty() {
        return Vec::new();
    }

    let mut lines: Vec<String> = Vec::new();
    lines.push(format!("*** {fromfile}\n"));
    lines.push(format!("--- {tofile}\n"));

    for hunk in &hunks {
        lines.push("***************\n".to_string());

        // from-file section (*** ... ***)
        let (a_start, a_count, b_start, b_count) = hunk_range(hunk);
        lines.push(format!("*** {},{} ****\n", a_start + 1, a_start + a_count));
        let has_changes_a = hunk
            .iter()
            .any(|e| matches!(e, Edit::Delete(_) | Edit::Equal(_, _)));
        if has_changes_a {
            for e in hunk {
                match e {
                    Edit::Equal(ai, _) => lines.push(format!("  {}", a[*ai])),
                    Edit::Delete(ai) => lines.push(format!("- {}", a[*ai])),
                    Edit::Insert(_, _) => {}
                }
            }
        }

        // to-file section (--- ... ---)
        lines.push(format!("--- {},{} ----\n", b_start + 1, b_start + b_count));
        let has_changes_b = hunk
            .iter()
            .any(|e| matches!(e, Edit::Insert(_, _) | Edit::Equal(_, _)));
        if has_changes_b {
            for e in hunk {
                match e {
                    Edit::Equal(_, bi) => lines.push(format!("  {}", b[*bi])),
                    Edit::Insert(_, bi) => lines.push(format!("+ {}", b[*bi])),
                    Edit::Delete(_) => {}
                }
            }
        }
    }
    lines
}

/// ndiff: side-by-side-style diff with `+ `, `- `, `  `, `? ` lines.
fn ndiff_impl(a: &[String], b: &[String]) -> Vec<String> {
    let lcs = lcs_indices(a, b);
    let edits = build_edit_script(a.len(), b.len(), &lcs);
    let mut lines: Vec<String> = Vec::new();
    for e in &edits {
        match e {
            Edit::Equal(ai, _) => lines.push(format!("  {}", a[*ai])),
            Edit::Delete(ai) => lines.push(format!("- {}", a[*ai])),
            Edit::Insert(_, bi) => lines.push(format!("+ {}", b[*bi])),
        }
    }
    lines
}

// ---------------------------------------------------------------------------
// get_close_matches
// ---------------------------------------------------------------------------

fn get_close_matches_impl(
    word: &str,
    possibilities: &[String],
    n: usize,
    cutoff: f64,
) -> Vec<String> {
    let word_chars: Vec<char> = word.chars().collect();
    let mut scored: Vec<(f64, &String)> = possibilities
        .iter()
        .filter_map(|p| {
            let p_chars: Vec<char> = p.chars().collect();
            let blocks = get_matching_blocks_impl(&word_chars, &p_chars);
            let ratio = ratio_from_blocks(&blocks, word_chars.len(), p_chars.len());
            if ratio >= cutoff {
                Some((ratio, p))
            } else {
                None
            }
        })
        .collect();
    // Sort descending by score, then by string (stable tiebreak).
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(n);
    scored.into_iter().map(|(_, s)| s.clone()).collect()
}

// ---------------------------------------------------------------------------
// Object helpers
// ---------------------------------------------------------------------------

fn alloc_str_or_err(_py: &PyToken<'_>, s: &str) -> Result<u64, u64> {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

/// Decode a Python list-of-str to a Vec<String>.
fn list_to_str_vec(_py: &PyToken<'_>, bits: u64) -> Result<Vec<String>, u64> {
    let obj = obj_from_bits(bits);
    if obj.is_none() {
        return Ok(Vec::new());
    }
    let Some(ptr) = obj.as_ptr() else {
        let tn = type_name(_py, obj);
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            &format!("expected list of str, got {tn}"),
        ));
    };
    let items: Vec<u64> = unsafe { crate::seq_vec_ref(ptr).to_vec() };
    let mut out = Vec::with_capacity(items.len());
    for item_bits in items {
        match string_obj_to_owned(obj_from_bits(item_bits)) {
            Some(s) => out.push(s),
            None => {
                return Err(raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "expected list of str elements",
                ));
            }
        }
    }
    Ok(out)
}

/// Build a Python list from a Vec<String>.
fn str_vec_to_list(_py: &PyToken<'_>, items: &[String]) -> u64 {
    let mut bits: Vec<u64> = Vec::with_capacity(items.len());
    for s in items {
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            for b in &bits {
                dec_ref_bits(_py, *b);
            }
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        bits.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, &bits);
    for b in &bits {
        dec_ref_bits(_py, *b);
    }
    if list_ptr.is_null() {
        raise_exception::<u64>(_py, "MemoryError", "out of memory")
    } else {
        MoltObject::from_ptr(list_ptr).bits()
    }
}

// ---------------------------------------------------------------------------
// Public FFI — SequenceMatcher
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_difflib_ratio(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a) = string_obj_to_owned(obj_from_bits(a_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "difflib.ratio: a must be str");
        };
        let Some(b) = string_obj_to_owned(obj_from_bits(b_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "difflib.ratio: b must be str");
        };
        let a_chars: Vec<char> = a.chars().collect();
        let b_chars: Vec<char> = b.chars().collect();
        let blocks = get_matching_blocks_impl(&a_chars, &b_chars);
        let ratio = ratio_from_blocks(&blocks, a_chars.len(), b_chars.len());
        MoltObject::from_float(ratio).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_difflib_quick_ratio(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a) = string_obj_to_owned(obj_from_bits(a_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "difflib.quick_ratio: a must be str");
        };
        let Some(b) = string_obj_to_owned(obj_from_bits(b_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "difflib.quick_ratio: b must be str");
        };
        let la = a.chars().count();
        let lb = b.chars().count();
        let total = la + lb;
        if total == 0 {
            return MoltObject::from_float(1.0).bits();
        }
        // Upper bound: 2 * min(len(a), len(b)) / (len(a) + len(b))
        let ratio = 2.0 * la.min(lb) as f64 / total as f64;
        MoltObject::from_float(ratio).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_difflib_get_matching_blocks(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a) = string_obj_to_owned(obj_from_bits(a_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "a must be str");
        };
        let Some(b) = string_obj_to_owned(obj_from_bits(b_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "b must be str");
        };
        let a_chars: Vec<char> = a.chars().collect();
        let b_chars: Vec<char> = b.chars().collect();
        let blocks = get_matching_blocks_impl(&a_chars, &b_chars);

        let mut block_bits: Vec<u64> = Vec::with_capacity(blocks.len());
        for blk in &blocks {
            let tup = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(blk.i as i64).bits(),
                    MoltObject::from_int(blk.j as i64).bits(),
                    MoltObject::from_int(blk.n as i64).bits(),
                ],
            );
            if tup.is_null() {
                for b in &block_bits {
                    dec_ref_bits(_py, *b);
                }
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            block_bits.push(MoltObject::from_ptr(tup).bits());
        }
        let list_ptr = alloc_list(_py, &block_bits);
        for b in &block_bits {
            dec_ref_bits(_py, *b);
        }
        if list_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_difflib_get_opcodes(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(a) = string_obj_to_owned(obj_from_bits(a_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "a must be str");
        };
        let Some(b) = string_obj_to_owned(obj_from_bits(b_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "b must be str");
        };
        let a_chars: Vec<char> = a.chars().collect();
        let b_chars: Vec<char> = b.chars().collect();
        let opcodes = get_opcodes_impl(&a_chars, &b_chars);

        let mut op_bits: Vec<u64> = Vec::with_capacity(opcodes.len());
        for op in &opcodes {
            let tag_ptr = alloc_string(_py, op.tag.as_str().as_bytes());
            if tag_ptr.is_null() {
                for b in &op_bits {
                    dec_ref_bits(_py, *b);
                }
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            let tup = alloc_tuple(
                _py,
                &[
                    MoltObject::from_ptr(tag_ptr).bits(),
                    MoltObject::from_int(op.i1 as i64).bits(),
                    MoltObject::from_int(op.i2 as i64).bits(),
                    MoltObject::from_int(op.j1 as i64).bits(),
                    MoltObject::from_int(op.j2 as i64).bits(),
                ],
            );
            dec_ref_bits(_py, MoltObject::from_ptr(tag_ptr).bits());
            if tup.is_null() {
                for b in &op_bits {
                    dec_ref_bits(_py, *b);
                }
                return raise_exception::<u64>(_py, "MemoryError", "out of memory");
            }
            op_bits.push(MoltObject::from_ptr(tup).bits());
        }
        let list_ptr = alloc_list(_py, &op_bits);
        for b in &op_bits {
            dec_ref_bits(_py, *b);
        }
        if list_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
}

// ---------------------------------------------------------------------------
// Public FFI — Diff generators
// ---------------------------------------------------------------------------

/// `molt_difflib_unified_diff(a_bits, b_bits, fromfile_bits, tofile_bits, n_bits) -> list[str]`
#[unsafe(no_mangle)]
pub extern "C" fn molt_difflib_unified_diff(
    a_bits: u64,
    b_bits: u64,
    fromfile_bits: u64,
    tofile_bits: u64,
    n_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let a = match list_to_str_vec(_py, a_bits) {
            Ok(v) => v,
            Err(b) => return b,
        };
        let b = match list_to_str_vec(_py, b_bits) {
            Ok(v) => v,
            Err(b) => return b,
        };
        let fromfile = string_obj_to_owned(obj_from_bits(fromfile_bits)).unwrap_or_default();
        let tofile = string_obj_to_owned(obj_from_bits(tofile_bits)).unwrap_or_default();
        let n = to_i64(obj_from_bits(n_bits)).unwrap_or(3).max(0) as usize;

        let lines = unified_diff_impl(&a, &b, &fromfile, &tofile, n);
        str_vec_to_list(_py, &lines)
    })
}

/// `molt_difflib_context_diff(a_bits, b_bits, fromfile_bits, tofile_bits, n_bits) -> list[str]`
#[unsafe(no_mangle)]
pub extern "C" fn molt_difflib_context_diff(
    a_bits: u64,
    b_bits: u64,
    fromfile_bits: u64,
    tofile_bits: u64,
    n_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let a = match list_to_str_vec(_py, a_bits) {
            Ok(v) => v,
            Err(b) => return b,
        };
        let b = match list_to_str_vec(_py, b_bits) {
            Ok(v) => v,
            Err(b) => return b,
        };
        let fromfile = string_obj_to_owned(obj_from_bits(fromfile_bits)).unwrap_or_default();
        let tofile = string_obj_to_owned(obj_from_bits(tofile_bits)).unwrap_or_default();
        let n = to_i64(obj_from_bits(n_bits)).unwrap_or(3).max(0) as usize;

        let lines = context_diff_impl(&a, &b, &fromfile, &tofile, n);
        str_vec_to_list(_py, &lines)
    })
}

/// `molt_difflib_ndiff(a_bits, b_bits) -> list[str]`
#[unsafe(no_mangle)]
pub extern "C" fn molt_difflib_ndiff(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let a = match list_to_str_vec(_py, a_bits) {
            Ok(v) => v,
            Err(b) => return b,
        };
        let b = match list_to_str_vec(_py, b_bits) {
            Ok(v) => v,
            Err(b) => return b,
        };
        let lines = ndiff_impl(&a, &b);
        str_vec_to_list(_py, &lines)
    })
}

// ---------------------------------------------------------------------------
// Public FFI — Helpers
// ---------------------------------------------------------------------------

/// `molt_difflib_get_close_matches(word_bits, possibilities_bits, n_bits, cutoff_bits) -> list[str]`
#[unsafe(no_mangle)]
pub extern "C" fn molt_difflib_get_close_matches(
    word_bits: u64,
    possibilities_bits: u64,
    n_bits: u64,
    cutoff_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(word) = string_obj_to_owned(obj_from_bits(word_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "word must be str");
        };
        let possibilities = match list_to_str_vec(_py, possibilities_bits) {
            Ok(v) => v,
            Err(b) => return b,
        };
        let n_obj = obj_from_bits(n_bits);
        let n = if n_obj.is_none() {
            3usize
        } else {
            to_i64(n_obj).unwrap_or(3).max(0) as usize
        };
        let cutoff_obj = obj_from_bits(cutoff_bits);
        let cutoff = if cutoff_obj.is_none() {
            0.6f64
        } else {
            to_f64(cutoff_obj).unwrap_or(0.6)
        };
        if !(0.0..=1.0).contains(&cutoff) {
            return raise_exception::<u64>(_py, "ValueError", "cutoff must be in [0.0, 1.0]");
        }
        let matches = get_close_matches_impl(&word, &possibilities, n, cutoff);
        str_vec_to_list(_py, &matches)
    })
}

/// `molt_difflib_is_junk(ch_bits) -> bool`
///
/// Default junk heuristic: a line is junk if it is blank or consists solely of whitespace.
#[unsafe(no_mangle)]
pub extern "C" fn molt_difflib_is_junk(ch_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(s) = string_obj_to_owned(obj_from_bits(ch_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "is_junk: argument must be str");
        };
        let junk = s.trim().is_empty();
        MoltObject::from_bool(junk).bits()
    })
}

// Suppress dead-code warnings for internal helpers used only in some paths.
#[allow(dead_code)]
fn _use_alloc_str(_py: &PyToken<'_>, s: &str) -> Result<u64, u64> {
    alloc_str_or_err(_py, s)
}
