//! CPython's list sort (`Objects/listobject.c`): a stable natural mergesort
//! with galloping and the powersort merge policy that compares with `<` alone,
//! asking the target CPython's comparisons in its order. Run detection is the
//! one part that differs by version ([`RunDetection`]). Every step moves whole
//! elements without loss or duplication, so the slice remains a permutation of
//! its input when a comparison fails; an inconsistent `<` yields some
//! permutation, never a failure of its own.

/// How the sort finds natural runs, which decides its comparisons.
#[derive(Clone, Copy)]
pub(super) enum RunDetection {
    /// Through CPython 3.12: a run is non-descending or strictly descending.
    StrictlyDescending,
    /// From CPython 3.13 (gh-116554): a descending run may hold runs of equal
    /// values, and once reversed it extends over a non-descending suffix.
    WeaklyDescending,
}

/// A failed sort: `Compare` carries the comparison's pending exception;
/// `Memory` has none yet.
pub(super) enum TimsortError {
    Compare,
    Memory,
}

const MIN_GALLOP: usize = 7;

#[derive(Clone, Copy)]
struct Run {
    base: usize,
    len: usize,
    power: u32,
}

struct MergeState<T> {
    min_gallop: usize,
    listlen: usize,
    pending: Vec<Run>,
    temp: Vec<T>,
}

enum MergeExit {
    Done(Result<(), ()>),
    /// One element of the first run is left, and it belongs at the end.
    CopyB,
    /// One element of the second run is left, and it belongs at the front.
    CopyA,
}

/// Sort `v` in place with the strict order `lt` (`Err` stops the sort).
pub(super) fn timsort<T, F>(v: &mut [T], runs: RunDetection, mut lt: F) -> Result<(), TimsortError>
where
    T: Copy,
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    let n = v.len();
    if n < 2 {
        return Ok(());
    }
    let mut ms = MergeState {
        min_gallop: MIN_GALLOP,
        listlen: n,
        pending: Vec::new(),
        temp: Vec::new(),
    };
    // A merge copies the shorter run, at most half the list; the pending
    // stack holds at most one run per bit of the length.
    if ms.temp.try_reserve_exact(n / 2 + 1).is_err()
        || ms
            .pending
            .try_reserve_exact(usize::BITS as usize + 1)
            .is_err()
    {
        return Err(TimsortError::Memory);
    }
    let compare = |_: ()| TimsortError::Compare;
    let minrun = compute_minrun(n);
    let mut lo = 0usize;
    let mut nremaining = n;
    while nremaining > 0 {
        let hi = lo + nremaining;
        let mut run = match runs {
            RunDetection::StrictlyDescending => {
                let (run, descending) = count_run(&mut lt, v, lo, hi).map_err(compare)?;
                if descending {
                    v[lo..lo + run].reverse();
                }
                run
            }
            RunDetection::WeaklyDescending => {
                count_weak_run(&mut lt, &mut v[lo..hi]).map_err(compare)?
            }
        };
        if run < minrun {
            let force = nremaining.min(minrun);
            binarysort(&mut lt, v, lo, lo + force, lo + run).map_err(compare)?;
            run = force;
        }
        found_new_run(&mut ms, &mut lt, v, run).map_err(compare)?;
        ms.pending.push(Run {
            base: lo,
            len: run,
            power: 0,
        });
        lo += run;
        nremaining -= run;
    }
    merge_force_collapse(&mut ms, &mut lt, v).map_err(compare)
}

/// `merge_compute_minrun`: `n` below 64; else a length in 32..=64 such that
/// `n / minrun` is at or just below a power of two.
fn compute_minrun(mut n: usize) -> usize {
    let mut r = 0;
    while n >= 64 {
        r |= n & 1;
        n >>= 1;
    }
    n + r
}

/// `binarysort`: binary insertion of `v[start..hi]` into the sorted
/// `v[lo..start]`. An element moves only after its position is known.
fn binarysort<T: Copy, F>(
    lt: &mut F,
    v: &mut [T],
    lo: usize,
    hi: usize,
    mut start: usize,
) -> Result<(), ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    if lo == start {
        start += 1;
    }
    while start < hi {
        let pivot = v[start];
        let mut l = lo;
        let mut r = start;
        loop {
            let p = l + ((r - l) >> 1);
            if lt(&pivot, &v[p])? {
                r = p;
            } else {
                l = p + 1;
            }
            if l >= r {
                break;
            }
        }
        v.copy_within(l..start, l + 1);
        v[l] = pivot;
        start += 1;
    }
    Ok(())
}

/// CPython 3.12's `count_run`: the length of the run at `lo`, and whether it
/// is strictly descending (which the caller reverses).
fn count_run<T, F>(lt: &mut F, v: &[T], lo: usize, hi: usize) -> Result<(usize, bool), ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    let mut i = lo + 1;
    if i == hi {
        return Ok((1, false));
    }
    let mut n = 2;
    if lt(&v[i], &v[i - 1])? {
        i += 1;
        while i < hi && lt(&v[i], &v[i - 1])? {
            i += 1;
            n += 1;
        }
        Ok((n, true))
    } else {
        i += 1;
        while i < hi && !lt(&v[i], &v[i - 1])? {
            i += 1;
            n += 1;
        }
        Ok((n, false))
    }
}

/// CPython 3.13's `count_run`: the length of the run at the start of `v`,
/// made ascending in place. An ascending run that rises ends at its first
/// fall; an equal prefix instead begins a descending run, which may hold runs
/// of equal values. Each of those is reversed before the whole run is, so
/// equal values keep their order, and the reversed run then extends over a
/// non-descending suffix. A failed comparison leaves the reversals made so
/// far, as CPython's does.
fn count_weak_run<T, F>(lt: &mut F, v: &mut [T]) -> Result<usize, ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    let len = v.len();
    let mut n = 1;
    while n < len && !lt(&v[n], &v[n - 1])? {
        n += 1;
    }
    if n == len {
        return Ok(n);
    }
    // v[n] < v[n-1]: a run that rose somewhere ends here; one that is all
    // equal values continues descending.
    if n > 1 {
        if lt(&v[0], &v[n - 1])? {
            return Ok(n);
        }
        v[..n].reverse();
    }
    n += 1;
    // The run's last `equal + 1` values are equal (CPython's `neq`).
    let mut equal = 0usize;
    while n < len {
        if lt(&v[n], &v[n - 1])? {
            reverse_equal_tail(v, n, &mut equal);
        } else if lt(&v[n - 1], &v[n])? {
            break;
        } else {
            equal += 1;
        }
        n += 1;
    }
    reverse_equal_tail(v, n, &mut equal);
    v[..n].reverse();
    while n < len && !lt(&v[n], &v[n - 1])? {
        n += 1;
    }
    Ok(n)
}

/// `REVERSE_LAST_NEQ`: reverse the `equal + 1` equal values that end before
/// `n`, and start counting again.
fn reverse_equal_tail<T>(v: &mut [T], n: usize, equal: &mut usize) {
    if *equal != 0 {
        v[n - (*equal + 1)..n].reverse();
        *equal = 0;
    }
}

/// `gallop_left`: the `k` with `a[k-1] < key <= a[k]`, searched from `hint`.
fn gallop_left<T, F>(lt: &mut F, key: &T, a: &[T], hint: usize) -> Result<usize, ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    let n = a.len();
    debug_assert!(n > 0 && hint < n);
    let mut lastofs = 0usize;
    let mut ofs = 1usize;
    let (mut lo, hi) = if lt(&a[hint], key)? {
        // a[hint] < key: gallop right until a[hint+lastofs] < key <= a[hint+ofs].
        let maxofs = n - hint;
        while ofs < maxofs {
            if lt(&a[hint + ofs], key)? {
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            } else {
                break;
            }
        }
        ofs = ofs.min(maxofs);
        // Exclusive lower bound hint + lastofs, as a count of skipped slots.
        (hint + lastofs + 1, hint + ofs)
    } else {
        // key <= a[hint]: gallop left until a[hint-ofs] < key <= a[hint-lastofs].
        let maxofs = hint + 1;
        while ofs < maxofs {
            if lt(&a[hint - ofs], key)? {
                break;
            }
            lastofs = ofs;
            ofs = (ofs << 1) + 1;
        }
        ofs = ofs.min(maxofs);
        // a[hint-ofs] < key (hint-ofs may be -1) <= a[hint-lastofs].
        (hint + 1 - ofs, hint - lastofs)
    };
    let mut hi = hi;
    while lo < hi {
        let m = lo + ((hi - lo) >> 1);
        if lt(&a[m], key)? {
            lo = m + 1;
        } else {
            hi = m;
        }
    }
    Ok(hi)
}

/// `gallop_right`: the `k` with `a[k-1] <= key < a[k]`, searched from `hint`.
fn gallop_right<T, F>(lt: &mut F, key: &T, a: &[T], hint: usize) -> Result<usize, ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    let n = a.len();
    debug_assert!(n > 0 && hint < n);
    let mut lastofs = 0usize;
    let mut ofs = 1usize;
    let (mut lo, hi) = if lt(key, &a[hint])? {
        // key < a[hint]: gallop left until a[hint-ofs] <= key < a[hint-lastofs].
        let maxofs = hint + 1;
        while ofs < maxofs {
            if lt(key, &a[hint - ofs])? {
                lastofs = ofs;
                ofs = (ofs << 1) + 1;
            } else {
                break;
            }
        }
        ofs = ofs.min(maxofs);
        (hint + 1 - ofs, hint - lastofs)
    } else {
        // a[hint] <= key: gallop right until a[hint+lastofs] <= key < a[hint+ofs].
        let maxofs = n - hint;
        while ofs < maxofs {
            if lt(key, &a[hint + ofs])? {
                break;
            }
            lastofs = ofs;
            ofs = (ofs << 1) + 1;
        }
        ofs = ofs.min(maxofs);
        (hint + lastofs + 1, hint + ofs)
    };
    let mut hi = hi;
    while lo < hi {
        let m = lo + ((hi - lo) >> 1);
        if lt(key, &a[m])? {
            hi = m;
        } else {
            lo = m + 1;
        }
    }
    Ok(hi)
}

/// `merge_lo`: merge `v[base_a..base_a+na]` and the adjacent
/// `v[base_b..base_b+nb]` (`na <= nb`), copying the first run aside.
fn merge_lo<T: Copy, F>(
    ms: &mut MergeState<T>,
    lt: &mut F,
    v: &mut [T],
    base_a: usize,
    mut na: usize,
    base_b: usize,
    mut nb: usize,
) -> Result<(), ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    debug_assert!(na > 0 && nb > 0 && base_a + na == base_b);
    ms.temp.clear();
    ms.temp.extend_from_slice(&v[base_a..base_a + na]);
    let mut dest = base_a;
    let mut a = 0usize; // in `ms.temp`
    let mut b = base_b; // in `v`
    v[dest] = v[b];
    dest += 1;
    b += 1;
    nb -= 1;
    let exit = 'merge: {
        if nb == 0 {
            break 'merge MergeExit::Done(Ok(()));
        }
        if na == 1 {
            break 'merge MergeExit::CopyB;
        }
        let mut min_gallop = ms.min_gallop;
        loop {
            let mut acount = 0usize;
            let mut bcount = 0usize;
            loop {
                match lt(&v[b], &ms.temp[a]) {
                    Err(()) => break 'merge MergeExit::Done(Err(())),
                    Ok(true) => {
                        v[dest] = v[b];
                        dest += 1;
                        b += 1;
                        bcount += 1;
                        acount = 0;
                        nb -= 1;
                        if nb == 0 {
                            break 'merge MergeExit::Done(Ok(()));
                        }
                        if bcount >= min_gallop {
                            break;
                        }
                    }
                    Ok(false) => {
                        v[dest] = ms.temp[a];
                        dest += 1;
                        a += 1;
                        acount += 1;
                        bcount = 0;
                        na -= 1;
                        if na == 1 {
                            break 'merge MergeExit::CopyB;
                        }
                        if acount >= min_gallop {
                            break;
                        }
                    }
                }
            }
            min_gallop += 1;
            loop {
                min_gallop -= usize::from(min_gallop > 1);
                ms.min_gallop = min_gallop;
                let key = v[b];
                let k = match gallop_right(lt, &key, &ms.temp[a..a + na], 0) {
                    Ok(k) => k,
                    Err(()) => break 'merge MergeExit::Done(Err(())),
                };
                acount = k;
                if k != 0 {
                    v[dest..dest + k].copy_from_slice(&ms.temp[a..a + k]);
                    dest += k;
                    a += k;
                    na -= k;
                    if na == 1 {
                        break 'merge MergeExit::CopyB;
                    }
                    // Impossible for a consistent `<`, which is not assumed.
                    if na == 0 {
                        break 'merge MergeExit::Done(Ok(()));
                    }
                }
                v[dest] = v[b];
                dest += 1;
                b += 1;
                nb -= 1;
                if nb == 0 {
                    break 'merge MergeExit::Done(Ok(()));
                }
                let key = ms.temp[a];
                let k = match gallop_left(lt, &key, &v[b..b + nb], 0) {
                    Ok(k) => k,
                    Err(()) => break 'merge MergeExit::Done(Err(())),
                };
                bcount = k;
                if k != 0 {
                    v.copy_within(b..b + k, dest);
                    dest += k;
                    b += k;
                    nb -= k;
                    if nb == 0 {
                        break 'merge MergeExit::Done(Ok(()));
                    }
                }
                v[dest] = ms.temp[a];
                dest += 1;
                a += 1;
                na -= 1;
                if na == 1 {
                    break 'merge MergeExit::CopyB;
                }
                if acount < MIN_GALLOP && bcount < MIN_GALLOP {
                    break;
                }
            }
            // Penalize leaving galloping mode.
            min_gallop += 1;
            ms.min_gallop = min_gallop;
        }
    };
    match exit {
        MergeExit::Done(result) => {
            // The unplaced first-run elements fill the gap before `b`.
            v[dest..dest + na].copy_from_slice(&ms.temp[a..a + na]);
            result
        }
        MergeExit::CopyB => {
            v.copy_within(b..b + nb, dest);
            v[dest + nb] = ms.temp[a];
            Ok(())
        }
        MergeExit::CopyA => unreachable!("merge_lo never leaves through CopyA"),
    }
}

/// `merge_hi`: [`merge_lo`] from the high end (`na >= nb`), copying the second
/// run aside. Positions run one below a run's start once it is exhausted, so
/// they are signed.
fn merge_hi<T: Copy, F>(
    ms: &mut MergeState<T>,
    lt: &mut F,
    v: &mut [T],
    base_a: usize,
    mut na: usize,
    base_b: usize,
    mut nb: usize,
) -> Result<(), ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    debug_assert!(na > 0 && nb > 0 && base_a + na == base_b);
    ms.temp.clear();
    ms.temp.extend_from_slice(&v[base_b..base_b + nb]);
    let at = |index: isize| index as usize;
    let mut dest = (base_b + nb - 1) as isize;
    let mut a = (base_a + na - 1) as isize; // in `v`
    let mut b = (nb - 1) as isize; // in `ms.temp`
    v[at(dest)] = v[at(a)];
    dest -= 1;
    a -= 1;
    na -= 1;
    let exit = 'merge: {
        if na == 0 {
            break 'merge MergeExit::Done(Ok(()));
        }
        if nb == 1 {
            break 'merge MergeExit::CopyA;
        }
        let mut min_gallop = ms.min_gallop;
        loop {
            let mut acount = 0usize;
            let mut bcount = 0usize;
            loop {
                match lt(&ms.temp[at(b)], &v[at(a)]) {
                    Err(()) => break 'merge MergeExit::Done(Err(())),
                    Ok(true) => {
                        v[at(dest)] = v[at(a)];
                        dest -= 1;
                        a -= 1;
                        acount += 1;
                        bcount = 0;
                        na -= 1;
                        if na == 0 {
                            break 'merge MergeExit::Done(Ok(()));
                        }
                        if acount >= min_gallop {
                            break;
                        }
                    }
                    Ok(false) => {
                        v[at(dest)] = ms.temp[at(b)];
                        dest -= 1;
                        b -= 1;
                        bcount += 1;
                        acount = 0;
                        nb -= 1;
                        if nb == 1 {
                            break 'merge MergeExit::CopyA;
                        }
                        if bcount >= min_gallop {
                            break;
                        }
                    }
                }
            }
            min_gallop += 1;
            loop {
                min_gallop -= usize::from(min_gallop > 1);
                ms.min_gallop = min_gallop;
                let key = ms.temp[at(b)];
                let k = match gallop_right(lt, &key, &v[base_a..base_a + na], na - 1) {
                    Ok(k) => na - k,
                    Err(()) => break 'merge MergeExit::Done(Err(())),
                };
                acount = k;
                if k != 0 {
                    dest -= k as isize;
                    a -= k as isize;
                    v.copy_within(at(a + 1)..at(a + 1) + k, at(dest + 1));
                    na -= k;
                    if na == 0 {
                        break 'merge MergeExit::Done(Ok(()));
                    }
                }
                v[at(dest)] = ms.temp[at(b)];
                dest -= 1;
                b -= 1;
                nb -= 1;
                if nb == 1 {
                    break 'merge MergeExit::CopyA;
                }
                let key = v[at(a)];
                let k = match gallop_left(lt, &key, &ms.temp[..nb], nb - 1) {
                    Ok(k) => nb - k,
                    Err(()) => break 'merge MergeExit::Done(Err(())),
                };
                bcount = k;
                if k != 0 {
                    dest -= k as isize;
                    b -= k as isize;
                    let (to, from) = (at(dest + 1), at(b + 1));
                    v[to..to + k].copy_from_slice(&ms.temp[from..from + k]);
                    nb -= k;
                    if nb == 1 {
                        break 'merge MergeExit::CopyA;
                    }
                    // Impossible for a consistent `<`, which is not assumed.
                    if nb == 0 {
                        break 'merge MergeExit::Done(Ok(()));
                    }
                }
                v[at(dest)] = v[at(a)];
                dest -= 1;
                a -= 1;
                na -= 1;
                if na == 0 {
                    break 'merge MergeExit::Done(Ok(()));
                }
                if acount < MIN_GALLOP && bcount < MIN_GALLOP {
                    break;
                }
            }
            min_gallop += 1;
            ms.min_gallop = min_gallop;
        }
    };
    match exit {
        MergeExit::Done(result) => {
            // The unplaced second-run elements are its lowest `nb`, and they
            // fill the gap ending at `dest`.
            let to = at(dest + 1 - nb as isize);
            v[to..to + nb].copy_from_slice(&ms.temp[..nb]);
            result
        }
        MergeExit::CopyA => {
            let (to, from) = (at(dest + 1 - na as isize), at(a + 1 - na as isize));
            v.copy_within(from..from + na, to);
            dest -= na as isize;
            v[at(dest)] = ms.temp[at(b)];
            Ok(())
        }
        MergeExit::CopyB => unreachable!("merge_hi never leaves through CopyB"),
    }
}

/// `merge_at`: merge pending runs `i` and `i + 1`.
fn merge_at<T: Copy, F>(ms: &mut MergeState<T>, lt: &mut F, v: &mut [T], i: usize) -> Result<(), ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    let Run {
        base: base_a,
        len: na,
        ..
    } = ms.pending[i];
    let Run {
        base: base_b,
        len: nb,
        ..
    } = ms.pending[i + 1];
    debug_assert!(na > 0 && nb > 0 && base_a + na == base_b);
    ms.pending[i].len = na + nb;
    if i + 3 == ms.pending.len() {
        ms.pending[i + 1] = ms.pending[i + 2];
    }
    ms.pending.pop();
    // Elements of the first run before the second's first are in place.
    let key = v[base_b];
    let k = gallop_right(lt, &key, &v[base_a..base_a + na], 0)?;
    let base_a = base_a + k;
    let na = na - k;
    if na == 0 {
        return Ok(());
    }
    // Elements of the second run after the first's last are in place.
    let key = v[base_a + na - 1];
    let nb = gallop_left(lt, &key, &v[base_b..base_b + nb], nb - 1)?;
    if nb == 0 {
        return Ok(());
    }
    if na <= nb {
        merge_lo(ms, lt, v, base_a, na, base_b, nb)
    } else {
        merge_hi(ms, lt, v, base_a, na, base_b, nb)
    }
}

/// `powerloop`: the powersort "power" of the run boundary between the runs
/// `[s1, s1+n1)` and `[s1+n1, s1+n1+n2)` of a list of length `n`.
fn powerloop(s1: usize, n1: usize, n2: usize, n: usize) -> u32 {
    let mut result = 0;
    let mut a = 2 * s1 + n1;
    let mut b = a + n1 + n2;
    loop {
        result += 1;
        if a >= n {
            a -= n;
            b -= n;
        } else if b >= n {
            break;
        }
        a <<= 1;
        b <<= 1;
    }
    result
}

/// `found_new_run`: merge the pending runs whose power exceeds that of the
/// boundary before a new run of length `n2`. The caller pushes the run.
fn found_new_run<T: Copy, F>(
    ms: &mut MergeState<T>,
    lt: &mut F,
    v: &mut [T],
    n2: usize,
) -> Result<(), ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    let Some(&top) = ms.pending.last() else {
        return Ok(());
    };
    let power = powerloop(top.base, top.len, n2, ms.listlen);
    while ms.pending.len() > 1 && ms.pending[ms.pending.len() - 2].power > power {
        let i = ms.pending.len() - 2;
        merge_at(ms, lt, v, i)?;
    }
    let last = ms.pending.len() - 1;
    ms.pending[last].power = power;
    Ok(())
}

/// `merge_force_collapse`: merge every pending run into one.
fn merge_force_collapse<T: Copy, F>(
    ms: &mut MergeState<T>,
    lt: &mut F,
    v: &mut [T],
) -> Result<(), ()>
where
    F: FnMut(&T, &T) -> Result<bool, ()>,
{
    while ms.pending.len() > 1 {
        let mut n = ms.pending.len() - 2;
        if n > 0 && ms.pending[n - 1].len < ms.pending[n + 1].len {
            n -= 1;
        }
        merge_at(ms, lt, v, n)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: [RunDetection; 2] = [
        RunDetection::StrictlyDescending,
        RunDetection::WeaklyDescending,
    ];

    fn sort_counting(values: &mut [(i64, usize)], runs: RunDetection) -> usize {
        let mut compares = 0;
        let sorted = timsort(values, runs, |a: &(i64, usize), b: &(i64, usize)| {
            compares += 1;
            Ok(a.0 < b.0)
        });
        assert!(sorted.is_ok());
        compares
    }

    fn tagged(keys: &[i64]) -> Vec<(i64, usize)> {
        keys.iter().copied().zip(0..).collect()
    }

    fn permutation(len: usize, seed: u64) -> Vec<(i64, usize)> {
        let mut state = seed;
        (0..len)
            .map(|index| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (((state >> 33) % 97) as i64, index)
            })
            .collect()
    }

    /// Sorted and stable: equal keys keep their input order.
    #[test]
    fn sorts_stably_across_run_and_gallop_paths() {
        let mut inputs: Vec<Vec<(i64, usize)>> = [
            (0, 1),
            (1, 2),
            (2, 3),
            (63, 4),
            (64, 5),
            (65, 6),
            (1000, 7),
            (5000, 8),
        ]
        .into_iter()
        .map(|(len, seed)| permutation(len, seed))
        .collect();
        // Long runs and runs with interleaved galloping: ascending blocks
        // followed by a descending block.
        inputs.push(
            (0..600)
                .map(|index| ((index % 200) as i64, index))
                .chain((0..300).map(|index| ((300 - index) as i64, 600 + index)))
                .collect(),
        );
        // Descending runs of equal values, alone and between rising blocks.
        inputs.push(
            (0..900)
                .map(|index| ((900 - index) as i64 / 3, index))
                .collect(),
        );
        inputs.push(
            (0..900)
                .map(|index| {
                    (
                        ((index / 150) % 2) as i64 * 1000 - (index % 150) as i64 / 4,
                        index,
                    )
                })
                .collect(),
        );
        for runs in BOTH {
            for input in &inputs {
                let mut values = input.clone();
                let mut expected = input.clone();
                expected.sort_by_key(|&(key, _)| key);
                sort_counting(&mut values, runs);
                assert_eq!(values, expected, "len {}", input.len());
            }
        }
    }

    /// CPython asks n-1 comparisons of an already sorted list and of a
    /// strictly descending one (one run, reversed in place).
    #[test]
    fn a_single_run_costs_n_minus_one_comparisons() {
        for runs in BOTH {
            let mut ascending: Vec<(i64, usize)> = (0..1000).map(|i| (i as i64, i)).collect();
            assert_eq!(sort_counting(&mut ascending, runs), 999);
            let mut descending: Vec<(i64, usize)> = (0..1000).map(|i| (-(i as i64), i)).collect();
            assert_eq!(sort_counting(&mut descending, runs), 999);
            assert!(descending.windows(2).all(|pair| pair[0].0 < pair[1].0));
        }
    }

    /// From 3.13 a descending run may hold runs of equal values, which keep
    /// their order, and a reversed run extends over a rising suffix: CPython's
    /// own example, `[3, 2, 3, 4, 1]`, starts with a run of four.
    #[test]
    fn weak_runs_take_in_equal_values_stably() {
        let mut values = tagged(&[5, 5, 4, 4, 3, 3]);
        let mut lt = |a: &(i64, usize), b: &(i64, usize)| Ok(a.0 < b.0);
        assert_eq!(count_weak_run(&mut lt, &mut values), Ok(6));
        assert_eq!(values, [(3, 4), (3, 5), (4, 2), (4, 3), (5, 0), (5, 1)]);
        let mut values = tagged(&[3, 2, 3, 4, 1]);
        assert_eq!(count_weak_run(&mut lt, &mut values), Ok(4));
        assert_eq!(values, [(2, 1), (3, 0), (3, 2), (4, 3), (1, 4)]);
        // Through 3.12 the same input starts with a run of two equal values.
        let values = tagged(&[5, 5, 4, 4, 3, 3]);
        assert_eq!(count_run(&mut lt, &values, 0, values.len()), Ok((2, false)));
    }

    /// A failing or inconsistent comparison leaves a permutation.
    #[test]
    fn failures_and_inconsistent_orders_keep_a_permutation() {
        let equal_runs: Vec<(i64, usize)> = (0..3000)
            .map(|index| ((3000 - index) as i64 / 5, index))
            .collect();
        for runs in BOTH {
            for input in [permutation(3000, 11), equal_runs.clone()] {
                for fail_at in [1usize, 2, 3, 5, 10, 100, 1000, 4000] {
                    let mut values = input.clone();
                    let mut calls = 0usize;
                    let result =
                        timsort(&mut values, runs, |a: &(i64, usize), b: &(i64, usize)| {
                            calls += 1;
                            if calls == fail_at {
                                Err(())
                            } else {
                                Ok(a.0 < b.0)
                            }
                        });
                    assert!(matches!(result, Err(TimsortError::Compare)) || calls < fail_at);
                    let mut indexes: Vec<usize> = values.iter().map(|&(_, index)| index).collect();
                    indexes.sort_unstable();
                    assert!(indexes.iter().copied().eq(0..3000), "fail_at {fail_at}");
                }
            }
            let mut values = permutation(3000, 12);
            let mut state = 7u64;
            let result = timsort(&mut values, runs, |_: &(i64, usize), _: &(i64, usize)| {
                state = state
                    .wrapping_mul(2862933555777941757)
                    .wrapping_add(3037000493);
                Ok(state >> 63 == 1)
            });
            assert!(result.is_ok());
            let mut indexes: Vec<usize> = values.iter().map(|&(_, index)| index).collect();
            indexes.sort_unstable();
            assert!(indexes.iter().copied().eq(0..3000));
        }
    }
}
