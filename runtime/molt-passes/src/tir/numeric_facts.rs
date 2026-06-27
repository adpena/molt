//! Shared numeric fact primitives for TIR optimization.
//!
//! This module is the common S6 authority for integer intervals, scalar
//! evolution expression shapes, loop trip counts, and Python range arithmetic.
//! Passes consume these facts instead of carrying pass-local numeric helpers
//! that can drift from each other.

use crate::tir::blocks::BlockId;
use crate::tir::op_kinds_generated::CountedLoopComparisonRole;
use crate::tir::values::ValueId;

// ---------------------------------------------------------------------------
// Integer interval
// ---------------------------------------------------------------------------

/// Signed inline-int window low bound: `-2^46`.
pub const INLINE_INT47_LO: i64 = -(1i64 << 46);
/// Signed inline-int window high bound: `2^46 - 1`.
pub const INLINE_INT47_HI: i64 = (1i64 << 46) - 1;

/// A closed integer interval `[lo, hi]` over the i64 domain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IntRange {
    pub lo: i64,
    pub hi: i64,
}

impl IntRange {
    /// The full i64 range (top of the lattice — "anything").
    pub const FULL_I64: IntRange = IntRange {
        lo: i64::MIN,
        hi: i64::MAX,
    };

    /// A single point `[v, v]`.
    pub fn point(v: i64) -> IntRange {
        IntRange { lo: v, hi: v }
    }

    /// A range `[lo, hi]`, normalized: an inverted/empty input saturates to
    /// `FULL_I64` (we model emptiness conservatively as "unknown", never as a
    /// proof of anything).
    pub fn new(lo: i64, hi: i64) -> IntRange {
        if lo > hi {
            IntRange::FULL_I64
        } else {
            IntRange { lo, hi }
        }
    }

    /// Lattice **join** (union over-approximation): the smallest interval
    /// containing both. Used to merge facts from multiple sources.
    pub fn join(self, other: IntRange) -> IntRange {
        IntRange {
            lo: self.lo.min(other.lo),
            hi: self.hi.max(other.hi),
        }
    }

    /// Lattice **meet** (intersection): the tightest interval implied by both
    /// facts. Used when a guard narrows an already-known range. Returns
    /// `FULL_I64` if the intersection is empty (modeled as "unknown" — never a
    /// false proof).
    pub fn meet(self, other: IntRange) -> IntRange {
        let lo = self.lo.max(other.lo);
        let hi = self.hi.min(other.hi);
        IntRange::new(lo, hi)
    }

    /// Saturating `self + other` in i128, clamped to the i64 domain.
    // Intentionally an inherent saturating-interval method, not `std::ops::Add`
    // (whose contract is wrapping/unchecked `+` on a scalar, not interval join).
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, other: IntRange) -> IntRange {
        let lo = (self.lo as i128) + (other.lo as i128);
        let hi = (self.hi as i128) + (other.hi as i128);
        IntRange::from_i128(lo, hi)
    }

    pub(crate) fn from_i128(lo: i128, hi: i128) -> IntRange {
        let clamp = |x: i128| -> i64 {
            if x < i64::MIN as i128 {
                i64::MIN
            } else if x > i64::MAX as i128 {
                i64::MAX
            } else {
                x as i64
            }
        };
        IntRange::new(clamp(lo), clamp(hi))
    }

    /// True if this is the top of the lattice (`FULL_I64`, "anything"). A FULL
    /// operand means "unknown", so most transfer functions degrade to FULL when
    /// any input is FULL.
    pub(crate) fn is_full(self) -> bool {
        self.lo == i64::MIN && self.hi == i64::MAX
    }

    /// Saturating interval subtraction `self - other` in i128.
    ///   `[a.lo - b.hi, a.hi - b.lo]`.
    // Inherent saturating-interval method, not `std::ops::Sub` (see `add`).
    #[allow(clippy::should_implement_trait)]
    pub fn sub(self, other: IntRange) -> IntRange {
        let lo = (self.lo as i128) - (other.hi as i128);
        let hi = (self.hi as i128) - (other.lo as i128);
        IntRange::from_i128(lo, hi)
    }

    /// Saturating interval multiplication in i128: the hull of the four corner
    /// products `{lo·lo, lo·hi, hi·lo, hi·hi}`. Sound for mixed signs.
    // Inherent saturating-interval method, not `std::ops::Mul` (see `add`).
    #[allow(clippy::should_implement_trait)]
    pub fn mul(self, other: IntRange) -> IntRange {
        let a = self.lo as i128;
        let b = self.hi as i128;
        let c = other.lo as i128;
        let d = other.hi as i128;
        let p = [a * c, a * d, b * c, b * d];
        let lo = *p.iter().min().unwrap();
        let hi = *p.iter().max().unwrap();
        IntRange::from_i128(lo, hi)
    }

    /// Saturating interval negation `-self`: `[-hi, -lo]` in i128 (so `-i64::MIN`
    /// saturates to `i64::MAX` rather than wrapping).
    // Inherent saturating-interval method, not `std::ops::Neg` (see `add`).
    #[allow(clippy::should_implement_trait)]
    pub fn neg(self) -> IntRange {
        IntRange::from_i128(-(self.hi as i128), -(self.lo as i128))
    }

    /// Python bitwise-AND transfer over the *mathematical* integer value
    /// (infinite two's-complement). Sound rules:
    ///
    ///   * **`a & m` with a constant `m >= 0`** ⇒ result ∈ `[0, m]`, for *any*
    ///     `a` (even negative). Every bit of `m` above its highest set bit is 0,
    ///     so it is 0 in the AND; the result is a submask of `m`. This is the
    ///     load-bearing `field = i & MASK` rule.
    ///   * **both operands non-negative** (`a, b >= 0`) ⇒ result ∈
    ///     `[0, min(a.hi, b.hi)]`, since `a & b <= min(a, b)` for non-negatives.
    ///
    /// Anything else (a negative operand without a constant non-negative mask)
    /// returns `FULL_I64`. `mask_const` is `Some(m)` when one operand is a
    /// compile-time constant, else `None`.
    pub(crate) fn bit_and(
        self,
        other: IntRange,
        self_const: Option<i64>,
        other_const: Option<i64>,
    ) -> IntRange {
        // Constant non-negative mask on either side bounds the result to [0, m].
        for m in [self_const, other_const].into_iter().flatten() {
            if m >= 0 {
                return IntRange::new(0, m);
            }
        }
        // Both operands provably non-negative ⇒ [0, min(hi_a, hi_b)].
        if self.lo >= 0 && other.lo >= 0 {
            return IntRange::new(0, self.hi.min(other.hi));
        }
        IntRange::FULL_I64
    }

    /// Python bitwise-OR / XOR transfer. Sound only when **both operands are
    /// provably non-negative**: then both results are non-negative and bounded
    /// by `fill_below(max(a.hi, b.hi))` — the smallest `2^k - 1` covering the
    /// larger operand's magnitude (OR/XOR never set a bit above the wider
    /// operand's most-significant bit). Otherwise `FULL_I64`.
    ///
    /// `or_lower_floor` distinguishes OR (whose result is `>= max(a, b)`, so its
    /// low bound is `max(a.lo, b.lo)`) from XOR (whose result can be 0, e.g.
    /// `x ^ x`, so its low bound is 0).
    pub(crate) fn bit_or_xor(self, other: IntRange, or_lower_floor: bool) -> IntRange {
        if self.lo < 0 || other.lo < 0 {
            return IntRange::FULL_I64;
        }
        let hi = fill_below(self.hi.max(other.hi));
        let lo = if or_lower_floor {
            self.lo.max(other.lo)
        } else {
            0
        };
        IntRange::new(lo, hi)
    }

    /// Python `%` transfer with a *constant* divisor `c` (molt/Python modulo:
    /// the result takes the **sign of the divisor**, verified against
    /// `sccp::eval_binary_mod`). Sound for *any* dividend:
    ///   * `c > 0` ⇒ result ∈ `[0, c - 1]`.
    ///   * `c < 0` ⇒ result ∈ `[c + 1, 0]`.
    ///   * `c == 0` ⇒ raises (no value) — caller must not invoke this.
    pub(crate) fn mod_const(c: i64) -> IntRange {
        if c > 0 {
            IntRange::new(0, c - 1)
        } else {
            // c < 0 (c == 0 excluded by caller).
            IntRange::new(c + 1, 0)
        }
    }

    /// Python `%` transfer with a *non-constant* divisor whose range is `divisor`.
    /// Sound only when the divisor's sign is provably uniform AND it cannot be
    /// zero:
    ///
    ///   * divisor provably `> 0` (`divisor.lo >= 1`) ⇒ result ∈
    ///     `[0, divisor.hi - 1]`.
    ///   * divisor provably `< 0` (`divisor.hi <= -1`) ⇒ result ∈
    ///     `[divisor.lo + 1, 0]`.
    ///
    /// A range straddling 0 (possible zero divisor → raise, or mixed sign)
    /// returns `FULL_I64`.
    pub(crate) fn mod_range(divisor: IntRange) -> IntRange {
        if divisor.lo >= 1 {
            // result magnitude < divisor; sign of divisor (positive).
            IntRange::from_i128(0, (divisor.hi as i128) - 1)
        } else if divisor.hi <= -1 {
            IntRange::from_i128((divisor.lo as i128) + 1, 0)
        } else {
            IntRange::FULL_I64
        }
    }

    /// Python floor division `self // d` by a *constant* divisor `d != 0`.
    /// Python `//` rounds toward negative infinity (unlike Rust/C truncation),
    /// so `-7 // 3 == -3` and `7 // -3 == -3`. As a function of the dividend,
    /// `x // d` is monotone — non-decreasing when `d > 0`, non-increasing when
    /// `d < 0` — so the result interval is the hull of the two endpoint
    /// quotients, computed exactly in i128. `from_i128` then saturates the lone
    /// `i64::MIN // -1` overflow (the only two-i64 floordiv leaving i64) to the
    /// i64 extreme, far outside any inline window. Sound for *any* dividend
    /// sign. `d == 0` raises (no value) — caller must exclude it.
    pub(crate) fn floordiv_const(self, d: i64) -> IntRange {
        debug_assert!(d != 0, "zero divisor excluded by caller (ZeroDivisionError)");
        let d = d as i128;
        let a = floor_div_i128(self.lo as i128, d);
        let b = floor_div_i128(self.hi as i128, d);
        // Monotone in the dividend ⇒ the two endpoints bracket the whole result
        // set; min/max absorbs the sign of `d` without a separate branch.
        IntRange::from_i128(a.min(b), a.max(b))
    }

    /// Python floor division `self // divisor` with a *non-constant* divisor
    /// whose range is `divisor`. Sound only when the divisor is provably
    /// non-zero AND sign-uniform: a range straddling 0 admits a zero divisor
    /// (`ZeroDivisionError`) or a sign flip, neither boundable, so it returns
    /// `FULL_I64` — never a false tight range (the inline-int47 truncation P0).
    ///
    /// Over a zero-free, sign-uniform divisor box, `x / y` is monotone in each
    /// variable separately (`∂/∂x = 1/y` has the fixed sign of `y`;
    /// `∂/∂y = -x/y²` has a fixed sign for each fixed `x`), so its extrema — and,
    /// because `floor` is monotone, the extrema of `x // y` — occur at the four
    /// `{lo,hi}×{lo,hi}` corners. The result is the hull of those corner
    /// quotients.
    pub(crate) fn floordiv_range(self, divisor: IntRange) -> IntRange {
        // Provably positive (`lo >= 1`) or provably negative (`hi <= -1`); a
        // range that could contain 0 is unprovable.
        if !(divisor.lo >= 1 || divisor.hi <= -1) {
            return IntRange::FULL_I64;
        }
        let corners = [
            floor_div_i128(self.lo as i128, divisor.lo as i128),
            floor_div_i128(self.lo as i128, divisor.hi as i128),
            floor_div_i128(self.hi as i128, divisor.lo as i128),
            floor_div_i128(self.hi as i128, divisor.hi as i128),
        ];
        let lo = *corners.iter().min().unwrap();
        let hi = *corners.iter().max().unwrap();
        IntRange::from_i128(lo, hi)
    }

    /// Python arithmetic right shift `self >> s` by a *constant* `s >= 0`.
    /// `x >> s == floor(x / 2^s)`, which is monotone non-decreasing in `x`, so
    /// the interval maps to `[floor(lo / 2^s), floor(hi / 2^s)]`. Computed in
    /// i128 with floor division (Rust `/` truncates toward zero, so adjust for
    /// negatives). `s < 0` is a Python `ValueError` (no value) and returns FULL.
    pub(crate) fn shr_const(self, s: i64) -> IntRange {
        if s < 0 {
            return IntRange::FULL_I64;
        }
        if s >= 127 {
            // Shifting any i64 right by >= 127 yields 0 (non-negative) or -1
            // (negative). Bound conservatively by [-1, 0] (sign-preserving).
            return IntRange::new(
                if self.lo < 0 { -1 } else { 0 },
                if self.hi < 0 { -1 } else { 0 },
            );
        }
        let div = 1i128 << s;
        // `div = 2^s > 0`, so the shared floor primitive's `(x<0) != (d<0)` sign
        // rule collapses to `x < 0` here — `x >> s == floor(x / 2^s)`.
        IntRange::from_i128(
            floor_div_i128(self.lo as i128, div),
            floor_div_i128(self.hi as i128, div),
        )
    }

    /// Python left shift `self << s` by a *constant* `s >= 0`:
    /// `[lo << s, hi << s]` in i128 (saturating). `s < 0` returns FULL.
    ///
    /// The product is computed with `checked_mul` because `lo`/`hi` at an i64
    /// extreme times `2^s` for `s >= 64` overflows **i128 itself** (`|i64::MIN| =
    /// 2^63`, `2^63 * 2^64 > i128::MAX`). An overflowing endpoint has definitively
    /// left the i64 domain in that direction, so it saturates to the matching i64
    /// extreme by sign — a sound widening (`from_i128` then clamps the in-range
    /// endpoint). This is what lets `(x << 80)` on an unbounded `x` (FULL range)
    /// yield a sound FULL result instead of panicking on i128 overflow.
    pub(crate) fn shl_const(self, s: i64) -> IntRange {
        if s < 0 {
            return IntRange::FULL_I64;
        }
        if self.lo == 0 && self.hi == 0 {
            return IntRange::point(0); // 0 << s == 0 for every s.
        }
        if s >= 127 {
            // A non-zero operand shifted this far overflows i64 in both
            // directions (handled by the checked path below too, but this avoids
            // forming a 2^127 shift amount).
            return IntRange::FULL_I64;
        }
        let mul = 1i128 << s;
        // A `checked_mul` overflow means the endpoint exceeds i128 — definitively
        // outside i64 — so saturate to the i64 extreme matching the product's
        // sign (the sign of the endpoint, since `mul > 0`).
        let shl_endpoint = |x: i64| -> i128 {
            match (x as i128).checked_mul(mul) {
                Some(p) => p,
                None if x < 0 => i64::MIN as i128,
                None => i64::MAX as i128,
            }
        };
        IntRange::from_i128(shl_endpoint(self.lo), shl_endpoint(self.hi))
    }

    /// True if the whole interval is `>= 0`.
    pub fn is_non_negative(self) -> bool {
        self.lo >= 0
    }

    /// True if the whole interval lies within the signed 47-bit inline window.
    pub fn fits_inline_int47(self) -> bool {
        self.lo >= INLINE_INT47_LO && self.hi <= INLINE_INT47_HI
    }

    /// True if the interval is PROVEN to exclude zero (entirely positive or
    /// entirely negative). `FULL_I64`/"unknown" returns `false` (we never prove
    /// non-zero from the top of the lattice). Used to gate the raw machine
    /// `sdiv`/`srem` lane: a divisor that is not proven non-zero must take the
    /// boxed runtime path, which raises `ZeroDivisionError` on a zero divisor
    /// instead of emitting a poison/trapping raw divide.
    pub fn proves_nonzero(self) -> bool {
        !self.is_full() && (self.lo > 0 || self.hi < 0)
    }

    /// True if the whole interval is inside the valid raw i64 shift-count
    /// domain. This is the shared guard for nothrow shift hoisting and raw-lane
    /// representation seeding; keeping it here prevents `[0, 63]` from becoming
    /// pass-local folklore.
    pub fn proves_i64_shift_count(self) -> bool {
        self.lo >= 0 && self.hi <= 63
    }
}

/// The smallest value of the form `2^k - 1` that is `>= x` (i.e. fill every bit
/// below `x`'s most-significant set bit). For a non-negative `x`, this is a sound
/// upper bound on `a | b` / `a ^ b` when `max(a, b) <= x`, because OR/XOR never
/// set a bit above the operands' highest set bit. `x <= 0` ⇒ 0 (the only
/// non-negative OR/XOR result bounded by a non-positive max is 0). Saturates to
/// `i64::MAX` if `x` has bit 62 set (the next fill would overflow i64).
fn fill_below(x: i64) -> i64 {
    if x <= 0 {
        return 0;
    }
    // Highest set bit position of x (x > 0 here, so 0..=62 for a positive i64;
    // bit 63 is the sign bit and cannot be set in a positive value).
    let hb = 63 - (x as u64).leading_zeros(); // 0..=62
    if hb >= 62 {
        // 2^63 - 1 == i64::MAX is the all-ones positive value.
        i64::MAX
    } else {
        (1i64 << (hb + 1)) - 1
    }
}

// ---------------------------------------------------------------------------
// SCEV expression lattice
// ---------------------------------------------------------------------------

/// A closed-form description of how an SSA value evolves.
///
/// `Add`/`Mul` are kept shallow (over operand `ValueId`s, not nested
/// expressions) — the analysis is intentionally a *linear*/affine recognizer,
/// not a full symbolic algebra system. That keeps it total and cheap; anything
/// it cannot prove affine is `Unknown` (the conservative top of the lattice).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScevExpr {
    /// A compile-time integer constant.
    Constant(i64),
    /// A value that is invariant within every loop it is queried against
    /// (defined outside any loop, or a parameter / non-recurrent definition).
    Invariant(ValueId),
    /// An affine recurrence `{start, +, step}` over `loop_header`:
    /// `start` on entry, `+ step` each back-edge. Sound only when the back-edge
    /// increment is proven non-wrapping (see module soundness rules).
    AddRec {
        start: Box<ScevExpr>,
        step: Box<ScevExpr>,
        loop_header: BlockId,
    },
    /// `a + b` of two sub-expressions (loop-invariant operands only).
    Add(Box<ScevExpr>, Box<ScevExpr>),
    /// `a * b` of two sub-expressions (loop-invariant operands only).
    Mul(Box<ScevExpr>, Box<ScevExpr>),
    /// No closed form proven — the conservative top of the lattice.
    Unknown,
}

impl ScevExpr {
    /// True for a recurrence (an `AddRec`). Convenience for consumers gating on
    /// "is this an induction variable".
    pub fn is_add_rec(&self) -> bool {
        matches!(self, ScevExpr::AddRec { .. })
    }

    /// If this expression is a compile-time constant, return it.
    pub fn as_constant(&self) -> Option<i64> {
        match self {
            ScevExpr::Constant(c) => Some(*c),
            _ => None,
        }
    }
}

/// The number of times a loop's body executes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TripCount {
    /// A statically-known constant trip count.
    Constant(i64),
    /// A symbolic trip count expressed as a (loop-invariant) SCEV expression
    /// (e.g. the `stop` of `for i in range(stop)`). The value is the count
    /// itself, in iterations.
    Symbolic(Box<ScevExpr>),
    /// Not proven.
    Unknown,
}

// ---------------------------------------------------------------------------
// Python range / counted-loop arithmetic
// ---------------------------------------------------------------------------

/// Compute len(range(start, stop, step)) using Python's exclusive-stop range
/// semantics. Returns None for step == 0 or when the exact length cannot fit
/// in Molt's current i64 constant lane; callers must then fail closed rather
/// than materializing a wrong constant.
pub fn python_range_len(start: i64, stop: i64, step: i64) -> Option<i64> {
    if step > 0 {
        if start >= stop {
            Some(0)
        } else {
            checked_i128_to_i64(ceil_div_positive(
                (stop as i128) - (start as i128),
                step as i128,
            ))
        }
    } else if step < 0 {
        if start <= stop {
            Some(0)
        } else {
            checked_i128_to_i64(ceil_div_positive(
                (start as i128) - (stop as i128),
                -(step as i128),
            ))
        }
    } else {
        None
    }
}

/// Return whether a Python range has at least one element without requiring its
/// exact length to fit in i64.
pub fn python_range_is_non_empty(start: i64, stop: i64, step: i64) -> Option<bool> {
    if step > 0 {
        Some(start < stop)
    } else if step < 0 {
        Some(start > stop)
    } else {
        None
    }
}

/// Compute the static trip count for start (cmp_role) stop stepping by
/// step. Inclusive comparison roles include the stop value; exclusive roles
/// match Python range-style loop guards. Direction/role mismatches and overflow
/// fail closed with None.
pub fn ordered_comparison_trip_count(
    cmp_role: CountedLoopComparisonRole,
    start: i64,
    stop: i64,
    step: i64,
) -> Option<i64> {
    if !cmp_role.is_ordered() || step == 0 {
        return None;
    }
    let inclusive_adjustment = if cmp_role.is_inclusive() { 1 } else { 0 };
    if cmp_role.requires_positive_step() {
        if step <= 0 {
            return None;
        }
        if start > stop || (start == stop && !cmp_role.is_inclusive()) {
            return Some(0);
        }
        let span = (stop as i128) - (start as i128) + inclusive_adjustment;
        checked_i128_to_i64(ceil_div_positive(span, step as i128))
    } else {
        if step >= 0 {
            return None;
        }
        if start < stop || (start == stop && !cmp_role.is_inclusive()) {
            return Some(0);
        }
        let span = (start as i128) - (stop as i128) + inclusive_adjustment;
        checked_i128_to_i64(ceil_div_positive(span, -(step as i128)))
    }
}

/// Exact integer hull of an affine induction value `{start, +, step}` over a
/// positive constant trip count. Returns `None` if the last value leaves the
/// i64 domain or if the recurrence does not execute.
pub fn affine_iv_hull(start: i64, step: i64, trip: i64) -> Option<IntRange> {
    if trip < 1 || step == 0 {
        return None;
    }
    let last = (start as i128) + ((trip as i128) - 1) * (step as i128);
    let lo = (start as i128).min(last);
    let hi = (start as i128).max(last);
    if lo < i64::MIN as i128 || hi > i64::MAX as i128 {
        return None;
    }
    Some(IntRange::new(lo as i64, hi as i64))
}

/// Sound range for a SCEV affine recurrence `{start, +, step}`. Constant trips
/// get an exact closed hull; symbolic/unknown trips retain the one-sided
/// monotone bound and rely on guard narrowing for the other side.
pub fn affine_recurrence_range(start: i64, step: i64, trip: &TripCount) -> Option<IntRange> {
    if step == 0 {
        return Some(IntRange::point(start));
    }
    let mono = if step > 0 {
        IntRange::new(start, i64::MAX)
    } else {
        IntRange::new(i64::MIN, start)
    };

    if let TripCount::Constant(t) = trip {
        if *t <= 0 {
            return Some(IntRange::point(start));
        }
        let last = (start as i128) + ((*t as i128) - 1) * (step as i128);
        let (lo, hi) = if step > 0 {
            (start as i128, last)
        } else {
            (last, start as i128)
        };
        return Some(IntRange::from_i128(lo, hi).meet(mono));
    }

    Some(mono)
}

/// Python integer floor division for i64 operands. Returns `None` for division
/// by zero or an exact quotient outside i64.
pub fn py_i64_floordiv(x: i64, y: i64) -> Option<i64> {
    if y == 0 {
        return None;
    }
    checked_i128_to_i64(floor_div_i128(x as i128, y as i128))
}

/// Python integer modulo for i64 operands. The remainder has the divisor's sign.
/// Returns `None` for division by zero or if the exact result is outside i64.
pub fn py_i64_mod(x: i64, y: i64) -> Option<i64> {
    if y == 0 {
        return None;
    }
    let x128 = x as i128;
    let y128 = y as i128;
    let floor_q = floor_div_i128(x128, y128);
    checked_i128_to_i64(x128 - floor_q * y128)
}

/// Exact mathematical floor division `floor(x / d)` over i128 — Python `//`
/// semantics (round toward negative infinity), versus Rust `/` which truncates
/// toward zero. The two disagree only when the quotient is negative and
/// inexact, where the true floor is one below the truncated value. `d != 0`
/// required. This is the single sign-correction site shared by every interval
/// transfer that floors a quotient ([`IntRange::floordiv_const`],
/// [`IntRange::floordiv_range`], [`IntRange::shr_const`]) and by the scalar
/// [`py_i64_floordiv`] / [`py_i64_mod`] folders, so the rule can never drift
/// between them.
fn floor_div_i128(x: i128, d: i128) -> i128 {
    debug_assert!(d != 0, "floor_div_i128 requires a non-zero divisor");
    let q = x / d;
    let r = x % d;
    if r != 0 && ((x < 0) != (d < 0)) {
        q - 1
    } else {
        q
    }
}

fn ceil_div_positive(numer: i128, denom: i128) -> i128 {
    debug_assert!(numer >= 0);
    debug_assert!(denom > 0);
    if numer == 0 {
        0
    } else {
        ((numer - 1) / denom) + 1
    }
}

fn checked_i128_to_i64(value: i128) -> Option<i64> {
    i64::try_from(value).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::op_kinds_generated::CountedLoopComparisonRole;

    #[test]
    fn int_range_join_meet() {
        let a = IntRange::new(0, 10);
        let b = IntRange::new(5, 20);
        assert_eq!(a.join(b), IntRange::new(0, 20));
        assert_eq!(a.meet(b), IntRange::new(5, 10));
        // Disjoint meet → unknown (FULL), never a false tight range.
        assert_eq!(
            IntRange::new(0, 1).meet(IntRange::new(5, 6)),
            IntRange::FULL_I64
        );
    }

    #[test]
    fn int_range_add_saturates() {
        let big = IntRange::point(i64::MAX);
        let r = big.add(IntRange::point(1));
        assert_eq!(r.hi, i64::MAX, "overflow must saturate, not wrap");
    }

    #[test]
    fn fits_inline_int47_boundary() {
        assert!(IntRange::new(INLINE_INT47_LO, INLINE_INT47_HI).fits_inline_int47());
        assert!(!IntRange::new(0, INLINE_INT47_HI + 1).fits_inline_int47());
        assert!(!IntRange::new(INLINE_INT47_LO - 1, 0).fits_inline_int47());
        assert!(!IntRange::FULL_I64.fits_inline_int47());
    }

    // -- transfer-function rules (soundness over the full i64 domain) ---------

    #[test]
    fn transfer_sub_mul_neg() {
        // [2, 5] - [1, 3] = [2-3, 5-1] = [-1, 4].
        assert_eq!(
            IntRange::new(2, 5).sub(IntRange::new(1, 3)),
            IntRange::new(-1, 4)
        );
        // [-2, 3] * [-4, 5] hull of {8,-10,-12,15} = [-12, 15].
        assert_eq!(
            IntRange::new(-2, 3).mul(IntRange::new(-4, 5)),
            IntRange::new(-12, 15)
        );
        // -[3, 7] = [-7, -3]; -[i64::MIN, 0] saturates the low end.
        assert_eq!(IntRange::new(3, 7).neg(), IntRange::new(-7, -3));
        assert_eq!(IntRange::point(i64::MIN).neg().hi, i64::MAX);
    }

    #[test]
    fn transfer_sub_mul_saturate() {
        // Overflowing products/diffs saturate, never wrap.
        let big = IntRange::point(i64::MAX);
        assert_eq!(big.mul(IntRange::point(2)).hi, i64::MAX);
        assert_eq!(
            IntRange::point(i64::MIN).sub(IntRange::point(1)).lo,
            i64::MIN
        );
    }

    #[test]
    fn transfer_bit_and_nonneg_mask() {
        // x & 15 ∈ [0, 15] for ANY x — even negative / unknown.
        let full = IntRange::FULL_I64;
        assert_eq!(
            full.bit_and(IntRange::point(15), None, Some(15)),
            IntRange::new(0, 15)
        );
        // mask on the left operand, dividend unknown.
        assert_eq!(
            full.bit_and(IntRange::point(7), Some(7), None),
            IntRange::new(0, 7)
        );
        // negative x is fine: -1 & 15 == 15 ∈ [0, 15].
        assert_eq!(
            IntRange::new(-100, -1).bit_and(IntRange::point(15), None, Some(15)),
            IntRange::new(0, 15)
        );
        // mask 0 ⇒ [0, 0].
        assert_eq!(
            full.bit_and(IntRange::point(0), None, Some(0)),
            IntRange::point(0)
        );
    }

    #[test]
    fn transfer_bit_and_negative_mask_is_full() {
        // x & (-2) can be negative (e.g. -4 & -2 == -4) — NOT bounded to [0,..].
        let full = IntRange::FULL_I64;
        assert!(full.bit_and(IntRange::point(-2), None, Some(-2)).is_full());
        // Both operands non-negative ⇒ [0, min(hi_a, hi_b)].
        assert_eq!(
            IntRange::new(0, 100).bit_and(IntRange::new(0, 7), None, None),
            IntRange::new(0, 7)
        );
        // One operand possibly-negative, no constant mask ⇒ FULL.
        assert!(
            IntRange::new(-1, 100)
                .bit_and(IntRange::new(0, 7), None, None)
                .is_full()
        );
    }

    #[test]
    fn transfer_bit_or_xor() {
        // 5 | 2 within [0,5]|[0,2] ⇒ low = max(lo) = 0? OR low floor = max(0,0)=0,
        // hi = fill_below(max(5,2)) = fill_below(5) = 7.
        assert_eq!(
            IntRange::new(0, 5).bit_or_xor(IntRange::new(0, 2), true),
            IntRange::new(0, 7)
        );
        // OR low floor: [4,5] | [8,9] ⇒ low = max(4,8) = 8, hi = fill_below(9)=15.
        assert_eq!(
            IntRange::new(4, 5).bit_or_xor(IntRange::new(8, 9), true),
            IntRange::new(8, 15)
        );
        // XOR can be 0 ⇒ low = 0.
        assert_eq!(
            IntRange::new(4, 5).bit_or_xor(IntRange::new(4, 5), false),
            IntRange::new(0, 7)
        );
        // Negative operand ⇒ FULL (can't bound bitwise of negatives cheaply).
        assert!(
            IntRange::new(-1, 5)
                .bit_or_xor(IntRange::new(0, 2), true)
                .is_full()
        );
        // fill_below saturation: huge value near bit 62 → i64::MAX.
        assert_eq!(fill_below((1i64 << 62) + 1), i64::MAX);
        assert_eq!(fill_below(8), 15);
        assert_eq!(fill_below(0), 0);
        assert_eq!(fill_below(-5), 0);
        assert_eq!(fill_below(1), 1);
    }

    #[test]
    fn transfer_mod_const_python_sign() {
        // x % 4 ∈ [0, 3] for ANY x (Python: result has sign of divisor).
        assert_eq!(IntRange::mod_const(4), IntRange::new(0, 3));
        // negative divisor: x % -4 ∈ [-3, 0].
        assert_eq!(IntRange::mod_const(-4), IntRange::new(-3, 0));
        // x % 1 ∈ [0, 0].
        assert_eq!(IntRange::mod_const(1), IntRange::point(0));
    }

    #[test]
    fn transfer_mod_const_matches_cpython_sign() {
        // Spot-check the bound against Python's actual % over mixed-sign dividends.
        // Python: (-7) % 4 == 1, 7 % 4 == 3, (-7) % -4 == -3, 7 % -4 == -1.
        let pos = IntRange::mod_const(4);
        for x in [-7i64, -1, 0, 3, 7, 1000] {
            let py = {
                let r = x % 4;
                if r != 0 && ((r ^ 4) < 0) { r + 4 } else { r }
            };
            assert!(py >= pos.lo && py <= pos.hi, "x%4={py} outside {pos:?}");
        }
        let neg = IntRange::mod_const(-4);
        for x in [-7i64, -1, 0, 3, 7, 1000] {
            let py = {
                let r = x % -4;
                if r != 0 && ((r ^ -4) < 0) { r + -4 } else { r }
            };
            assert!(py >= neg.lo && py <= neg.hi, "x%-4={py} outside {neg:?}");
        }
    }

    #[test]
    fn transfer_mod_range_sign_uniform() {
        // divisor provably in [2, 9] ⇒ result ∈ [0, 8].
        assert_eq!(
            IntRange::mod_range(IntRange::new(2, 9)),
            IntRange::new(0, 8)
        );
        // divisor provably in [-9, -2] ⇒ result ∈ [-8, 0].
        assert_eq!(
            IntRange::mod_range(IntRange::new(-9, -2)),
            IntRange::new(-8, 0)
        );
        // divisor straddles 0 (possible zero → raise, or mixed sign) ⇒ FULL.
        assert!(IntRange::mod_range(IntRange::new(-1, 5)).is_full());
        assert!(IntRange::mod_range(IntRange::new(0, 5)).is_full());
    }

    #[test]
    fn transfer_shr_const() {
        // 100 >> 2 == 25; [0,100] >> 2 = [0, 25].
        assert_eq!(IntRange::new(0, 100).shr_const(2), IntRange::new(0, 25));
        // Negative floor: (-7) >> 1 == floor(-3.5) == -4 in Python.
        assert_eq!(IntRange::new(-7, -7).shr_const(1), IntRange::point(-4));
        assert_eq!(IntRange::new(-8, 8).shr_const(2), IntRange::new(-2, 2));
        // s < 0 ⇒ FULL (ValueError, no value).
        assert!(IntRange::new(0, 100).shr_const(-1).is_full());
        // huge shift ⇒ sign-preserving [-1, 0] band.
        assert_eq!(IntRange::new(-5, 9).shr_const(200), IntRange::new(-1, 0));
    }

    #[test]
    fn transfer_shl_const() {
        // [1, 3] << 2 = [4, 12].
        assert_eq!(IntRange::new(1, 3).shl_const(2), IntRange::new(4, 12));
        // 1 << 70 overflows i64 ⇒ saturates to [i64::MAX, i64::MAX] (a point,
        // soundly above the value, not a wrap) — correctly NOT inline.
        assert_eq!(IntRange::point(1).shl_const(70), IntRange::point(i64::MAX));
        assert!(!IntRange::point(1).shl_const(70).fits_inline_int47());
        // A two-sided operand shifted past the i64 width saturates both ends.
        assert!(IntRange::new(-1, 1).shl_const(70).is_full());
        // 0 << anything == 0.
        assert_eq!(IntRange::point(0).shl_const(200), IntRange::point(0));
    }

    #[test]
    fn transfer_floordiv_const_python_floor_sign() {
        // Positive divisor: monotone non-decreasing in the dividend.
        // [0, 999] // 3 = [0, 333] (999 // 3 == 333) — the `i // 3` loop-IV case.
        assert_eq!(
            IntRange::new(0, 999).floordiv_const(3),
            IntRange::new(0, 333)
        );
        // Negative dividend floors toward -inf, NOT toward zero:
        // -7 // 3 == -3 (truncating division would give -2).
        assert_eq!(IntRange::new(-7, -7).floordiv_const(3), IntRange::point(-3));
        // Zero-crossing dividend: [-7, 7] // 3 = [-3, 2].
        assert_eq!(IntRange::new(-7, 7).floordiv_const(3), IntRange::new(-3, 2));
        // Negative divisor: monotone non-increasing ⇒ endpoints swap.
        // [1, 10] // -3 = [-4, -1] (1 // -3 == -1, 10 // -3 == -4).
        assert_eq!(
            IntRange::new(1, 10).floordiv_const(-3),
            IntRange::new(-4, -1)
        );
        // Both negative ⇒ positive quotient, floored: -7 // -3 == 2.
        assert_eq!(IntRange::point(-7).floordiv_const(-3), IntRange::point(2));
        // Divide by ±1 is identity / negation.
        assert_eq!(IntRange::new(-5, 9).floordiv_const(1), IntRange::new(-5, 9));
        assert_eq!(IntRange::new(-5, 9).floordiv_const(-1), IntRange::new(-9, 5));
    }

    #[test]
    fn transfer_floordiv_const_matches_cpython_scalar() {
        // The interval transfer at a POINT must equal the CPython-verified scalar
        // floor (`py_i64_floordiv`, checked against Python edges in
        // `python_i64_floor_div_and_mod_match_python_edges`), over a grid
        // spanning both signs of dividend and divisor. This ties the interval
        // arithmetic to ground-truth Python `//` semantics — a single shared
        // `floor_div_i128` primitive backs both, so they cannot drift.
        for x in -25i64..=25 {
            for d in [-7i64, -3, -2, -1, 1, 2, 3, 7] {
                let expect = py_i64_floordiv(x, d).expect("non-zero divisor, in range");
                assert_eq!(
                    IntRange::point(x).floordiv_const(d),
                    IntRange::point(expect),
                    "point({x}) // {d}"
                );
            }
        }
    }

    #[test]
    fn transfer_floordiv_const_min_over_neg_one_saturates_non_inline() {
        // i64::MIN // -1 == 2^63 escapes i64 upward (the only two-i64 floordiv
        // that leaves i64); from_i128 saturates to i64::MAX — soundly at/above
        // the true value — and it must NOT be proven inline.
        let r = IntRange::point(i64::MIN).floordiv_const(-1);
        assert_eq!(r, IntRange::point(i64::MAX));
        assert!(!r.fits_inline_int47());
    }

    #[test]
    fn transfer_floordiv_range_sign_uniform() {
        // Positive divisor box: the max quotient is at the SMALLEST divisor.
        // [0, 100] // [2, 5] = [0, 50] (100 // 2 == 50).
        assert_eq!(
            IntRange::new(0, 100).floordiv_range(IntRange::new(2, 5)),
            IntRange::new(0, 50)
        );
        // Negative divisor box: [10, 20] // [-5, -2] = [-10, -2].
        assert_eq!(
            IntRange::new(10, 20).floordiv_range(IntRange::new(-5, -2)),
            IntRange::new(-10, -2)
        );
        // A divisor range straddling 0 (possible ZeroDivisionError or sign flip)
        // is unprovable ⇒ FULL, never a false tight bound (the inline-int47
        // truncation P0).
        assert!(
            IntRange::new(0, 100)
                .floordiv_range(IntRange::new(-1, 5))
                .is_full()
        );
        // A divisor range that merely touches 0 at an endpoint is equally
        // unprovable (the zero divisor itself raises).
        assert!(
            IntRange::new(0, 100)
                .floordiv_range(IntRange::new(0, 5))
                .is_full()
        );
    }

    #[test]
    fn iv_range_positive_step() {
        // for i in range(10): i in [0, 9].
        let r = affine_recurrence_range(0, 1, &TripCount::Constant(10)).unwrap();
        assert_eq!(r, IntRange::new(0, 9));
    }

    #[test]
    fn iv_range_step_two() {
        // for i in range(0, 10, 2): values 0,2,4,6,8 → [0, 8], trip 5.
        let r = affine_recurrence_range(0, 2, &TripCount::Constant(5)).unwrap();
        assert_eq!(r, IntRange::new(0, 8));
    }

    #[test]
    fn iv_range_negative_step() {
        // for i in range(10, 0, -1): values 10,9,...,1 → [1, 10], trip 10.
        let r = affine_recurrence_range(10, -1, &TripCount::Constant(10)).unwrap();
        assert_eq!(r, IntRange::new(1, 10));
    }

    #[test]
    fn python_range_len_matches_python_edges() {
        assert_eq!(python_range_len(0, 10, 1), Some(10));
        assert_eq!(python_range_len(0, 10, 2), Some(5));
        assert_eq!(python_range_len(0, 10, 3), Some(4));
        assert_eq!(python_range_len(0, 0, 1), Some(0));
        assert_eq!(python_range_len(5, 5, 1), Some(0));
        assert_eq!(python_range_len(10, 0, -1), Some(10));
        assert_eq!(python_range_len(10, 0, -2), Some(5));
        assert_eq!(python_range_len(10, 0, -3), Some(4));
        assert_eq!(python_range_len(0, -10, -1), Some(10));
        assert_eq!(python_range_len(0, 10, -1), Some(0));
        assert_eq!(python_range_len(10, 0, 1), Some(0));
        assert_eq!(python_range_len(0, 1, 1), Some(1));
        assert_eq!(python_range_len(-5, 5, 1), Some(10));
        assert_eq!(python_range_len(0, 1, 0), None);
    }

    #[test]
    fn python_range_bool_does_not_need_i64_length_fit() {
        assert_eq!(python_range_is_non_empty(i64::MIN, i64::MAX, 1), Some(true));
        assert_eq!(python_range_len(i64::MIN, i64::MAX, 1), None);
        assert_eq!(python_range_is_non_empty(10, 0, 1), Some(false));
        assert_eq!(python_range_is_non_empty(0, 10, -1), Some(false));
        assert_eq!(python_range_is_non_empty(0, 10, 0), None);
    }

    #[test]
    fn ordered_comparison_trip_count_matches_guard_roles() {
        use CountedLoopComparisonRole::{
            DecreasingExclusive, DecreasingInclusive, IncreasingExclusive, IncreasingInclusive,
        };

        assert_eq!(
            ordered_comparison_trip_count(IncreasingExclusive, 0, 10, 1),
            Some(10)
        );
        assert_eq!(
            ordered_comparison_trip_count(IncreasingExclusive, 0, 10, 3),
            Some(4)
        );
        assert_eq!(
            ordered_comparison_trip_count(IncreasingInclusive, 0, 10, 1),
            Some(11)
        );
        assert_eq!(
            ordered_comparison_trip_count(DecreasingExclusive, 10, 0, -1),
            Some(10)
        );
        assert_eq!(
            ordered_comparison_trip_count(DecreasingInclusive, 10, 0, -1),
            Some(11)
        );
        assert_eq!(
            ordered_comparison_trip_count(IncreasingExclusive, 10, 0, 1),
            Some(0)
        );
        assert_eq!(
            ordered_comparison_trip_count(DecreasingExclusive, 0, 10, -1),
            Some(0)
        );
        assert_eq!(
            ordered_comparison_trip_count(IncreasingExclusive, 0, 10, -1),
            None
        );
    }

    #[test]
    fn python_i64_floor_div_and_mod_match_python_edges() {
        assert_eq!(py_i64_floordiv(7, 3), Some(2));
        assert_eq!(py_i64_floordiv(-7, 3), Some(-3));
        assert_eq!(py_i64_floordiv(7, -3), Some(-3));
        assert_eq!(py_i64_floordiv(-7, -3), Some(2));
        assert_eq!(py_i64_floordiv(i64::MIN, -1), None);
        assert_eq!(py_i64_floordiv(1, 0), None);

        assert_eq!(py_i64_mod(7, 3), Some(1));
        assert_eq!(py_i64_mod(-7, 3), Some(2));
        assert_eq!(py_i64_mod(7, -3), Some(-2));
        assert_eq!(py_i64_mod(-7, -3), Some(-1));
        assert_eq!(py_i64_mod(i64::MIN, -1), Some(0));
        assert_eq!(py_i64_mod(1, 0), None);
    }

    #[test]
    fn affine_helpers_fail_closed_and_preserve_monotone_bounds() {
        assert_eq!(affine_iv_hull(0, 1, 10), Some(IntRange::new(0, 9)));
        assert_eq!(affine_iv_hull(10, -1, 10), Some(IntRange::new(1, 10)));
        assert_eq!(affine_iv_hull(i64::MAX, 1, 2), None);

        assert_eq!(
            affine_recurrence_range(0, 2, &TripCount::Constant(5)),
            Some(IntRange::new(0, 8))
        );
        assert_eq!(
            affine_recurrence_range(10, -1, &TripCount::Unknown),
            Some(IntRange::new(i64::MIN, 10))
        );
    }

    #[test]
    fn shift_count_proof_is_shared() {
        assert!(IntRange::new(0, 63).proves_i64_shift_count());
        assert!(!IntRange::new(-1, 63).proves_i64_shift_count());
        assert!(!IntRange::new(0, 64).proves_i64_shift_count());
        assert!(!IntRange::FULL_I64.proves_i64_shift_count());
    }
}
