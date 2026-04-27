//! Pluggable resource control for the Molt runtime.
//!
//! Provides a [`ResourceTracker`] trait that guards heap allocations, execution
//! time, recursion depth, and pre-emptive DoS checks on expensive operations.
//! A thread-local tracker is accessible via [`with_tracker`]; the default is
//! [`UnlimitedTracker`] (no-op), suitable for unconstrained dev builds.
//!
//! For sandboxed deployments (Cloudflare Workers, WASM isolates), install a
//! [`LimitedTracker`] at host initialization time via [`set_tracker`].

use std::cell::RefCell;
use std::fmt;
use std::sync::RwLock;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// ResourceError
// ---------------------------------------------------------------------------

/// Error returned when a resource limit is exceeded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceError {
    /// Heap memory budget exceeded.
    Memory {
        /// Bytes currently in use.
        used: usize,
        /// Configured byte limit.
        limit: usize,
    },
    /// Wall-clock execution time exceeded.
    Time {
        /// Elapsed milliseconds at the point of check.
        elapsed_ms: u64,
        /// Configured millisecond limit.
        limit_ms: u64,
    },
    /// Too many individual allocations.
    Allocation {
        /// Current allocation count.
        count: usize,
        /// Configured count limit.
        limit: usize,
    },
    /// Call stack too deep.
    Recursion {
        /// Current recursion depth.
        depth: usize,
        /// Configured depth limit.
        limit: usize,
    },
    /// A single operation would produce an unreasonably large result.
    OperationTooLarge {
        /// Human-readable description of the operation.
        op: String,
        /// Estimated result size in bytes.
        estimated_bytes: usize,
    },
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Memory { used, limit } => {
                write!(f, "memory limit exceeded: {used} bytes used, limit {limit}")
            }
            Self::Time {
                elapsed_ms,
                limit_ms,
            } => write!(
                f,
                "time limit exceeded: {elapsed_ms}ms elapsed, limit {limit_ms}ms"
            ),
            Self::Allocation { count, limit } => {
                write!(
                    f,
                    "allocation limit exceeded: {count} allocations, limit {limit}"
                )
            }
            Self::Recursion { depth, limit } => {
                write!(f, "recursion limit exceeded: depth {depth}, limit {limit}")
            }
            Self::OperationTooLarge {
                op,
                estimated_bytes,
            } => write!(
                f,
                "operation too large: {op} would produce ~{estimated_bytes} bytes"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// OperationEstimate
// ---------------------------------------------------------------------------

/// Pre-execution size estimate for potentially expensive operations.
///
/// The runtime checks these *before* performing the operation so that a
/// malicious program cannot force an OOM by, e.g., computing `2 ** (1 << 40)`.
#[derive(Debug, Clone)]
pub enum OperationEstimate {
    /// Integer exponentiation. Estimated result bits = `base_bits * exponent`,
    /// multiplied by a 4x safety factor.
    Pow {
        /// Bit-width of the base operand.
        base_bits: u32,
        /// Exponent value.
        exponent: u64,
    },
    /// Sequence repetition (`[x] * n`, `"s" * n`).
    Repeat {
        /// Byte size of a single item.
        item_bytes: usize,
        /// Repeat count.
        count: u64,
    },
    /// Integer multiplication. Result bits ~ `a_bits + b_bits`.
    Multiply {
        /// Bit-width of the first operand.
        a_bits: u32,
        /// Bit-width of the second operand.
        b_bits: u32,
    },
    /// Left shift. Result bits ~ `value_bits + shift`.
    LeftShift {
        /// Bit-width of the value being shifted.
        value_bits: u32,
        /// Shift amount.
        shift: u32,
    },
    /// String replacement where the new substring is larger than the old one.
    StringReplace {
        /// Length of the input string.
        input_len: usize,
        /// Length of the substring being replaced.
        old_len: usize,
        /// Length of the replacement string.
        new_len: usize,
        /// Maximum replacement count.
        count: usize,
    },
}

impl OperationEstimate {
    /// Compute the estimated result size in bytes.
    ///
    /// Returns `None` on overflow (treated as "too large" by callers).
    pub fn estimated_bytes(&self) -> Option<usize> {
        match self {
            Self::Pow {
                base_bits,
                exponent,
            } => {
                // result_bits = base_bits * exponent, with 4x safety multiplier
                let result_bits = (*base_bits as u128)
                    .checked_mul(*exponent as u128)?
                    .checked_mul(4)?;
                let result_bytes = result_bits.div_ceil(8);
                usize::try_from(result_bytes).ok()
            }
            Self::Repeat { item_bytes, count } => (*item_bytes as u128)
                .checked_mul(*count as u128)
                .and_then(|v| usize::try_from(v).ok()),
            Self::Multiply { a_bits, b_bits } => {
                let result_bits = (*a_bits as u64) + (*b_bits as u64);
                let result_bytes = result_bits.div_ceil(8);
                usize::try_from(result_bytes).ok()
            }
            Self::LeftShift { value_bits, shift } => {
                let result_bits = (*value_bits as u64) + (*shift as u64);
                let result_bytes = result_bits.div_ceil(8);
                // 2x safety multiplier for BigInt intermediate allocation during shift
                result_bytes
                    .checked_mul(2)
                    .and_then(|v| usize::try_from(v).ok())
            }
            Self::StringReplace {
                input_len,
                old_len,
                new_len,
                count,
            } => {
                if *old_len == 0 {
                    // Replacing empty string: insertions between every char.
                    return (*new_len as u128)
                        .checked_mul((*input_len as u128) + 1)?
                        .checked_add(*input_len as u128)
                        .and_then(|v| usize::try_from(v).ok());
                }
                let max_replacements = (*count).min((*input_len) / (*old_len).max(1));
                let growth_per = new_len.saturating_sub(*old_len);
                (*input_len as u128)
                    .checked_add((growth_per as u128).checked_mul(max_replacements as u128)?)
                    .and_then(|v| usize::try_from(v).ok())
            }
        }
    }

    /// Human-readable label for error messages.
    fn label(&self) -> String {
        match self {
            Self::Pow {
                base_bits,
                exponent,
            } => format!("pow({base_bits}-bit base, exp={exponent})"),
            Self::Repeat { item_bytes, count } => {
                format!("repeat({item_bytes}B item * {count})")
            }
            Self::Multiply { a_bits, b_bits } => {
                format!("multiply({a_bits}-bit * {b_bits}-bit)")
            }
            Self::LeftShift { value_bits, shift } => {
                format!("lshift({value_bits}-bit << {shift})")
            }
            Self::StringReplace {
                input_len,
                old_len,
                new_len,
                count,
            } => format!("str.replace(input={input_len}, old={old_len}, new={new_len}, n={count})"),
        }
    }
}

// ---------------------------------------------------------------------------
// ResourceTracker trait
// ---------------------------------------------------------------------------

/// Pluggable resource control interface.
///
/// Implementations are installed per-thread via [`set_tracker`]. All hooks are
/// called on the hot path, so implementors should keep work minimal.
pub trait ResourceTracker {
    /// Called before every heap allocation. Return `Err` to deny.
    fn on_allocate(&mut self, size: usize) -> Result<(), ResourceError>;

    /// Called when memory is freed.
    fn on_free(&mut self, size: usize);

    /// Called when a container (list, dict, bytes) grows.
    fn on_grow(&mut self, additional_bytes: usize) -> Result<(), ResourceError>;

    /// Rate-limited wall-clock time check. Implementations should avoid
    /// calling `Instant::elapsed()` on every invocation.
    fn check_time(&mut self) -> Result<(), ResourceError>;

    /// Called before entering a new call frame.
    fn check_recursion_depth(&mut self, depth: usize) -> Result<(), ResourceError>;

    /// Pre-emptive check for operations that could produce enormous results.
    fn check_operation_size(&mut self, op: &OperationEstimate) -> Result<(), ResourceError>;
}

// ---------------------------------------------------------------------------
// ResourceLimits config
// ---------------------------------------------------------------------------

/// Declarative resource limits, typically parsed from a capability manifest.
#[derive(Debug, Clone, Default)]
pub struct ResourceLimits {
    /// Maximum heap memory in bytes.
    pub max_memory: Option<usize>,
    /// Maximum wall-clock execution time.
    pub max_duration: Option<Duration>,
    /// Maximum number of individual allocations.
    pub max_allocations: Option<usize>,
    /// Maximum call-stack recursion depth.
    pub max_recursion_depth: Option<usize>,
    /// Maximum estimated result size (bytes) for a single operation.
    /// Defaults to 10 MB when `None`.
    pub max_operation_result_bytes: Option<usize>,
}

/// Default per-operation result size limit: 10 MB.
const DEFAULT_MAX_OPERATION_RESULT_BYTES: usize = 10 * 1024 * 1024;

/// How often `check_time` actually samples `Instant::elapsed()`.
const TIME_CHECK_INTERVAL: u32 = 10;

// ---------------------------------------------------------------------------
// LimitedTracker
// ---------------------------------------------------------------------------

/// Resource tracker with configurable limits.
///
/// Suitable for sandboxed/multi-tenant deployments where untrusted code must
/// be constrained. Time checks are rate-limited: `Instant::elapsed()` is only
/// called every [`TIME_CHECK_INTERVAL`]th invocation of [`check_time`].
pub struct LimitedTracker {
    /// Current live allocation count.
    allocation_count: usize,
    /// Current live heap bytes.
    memory_used: usize,
    /// Monotonic start time, captured at construction.
    start_time: Instant,
    /// Counter for rate-limiting time checks.
    time_check_counter: u32,

    // --- limits ---
    max_allocations: Option<usize>,
    max_duration: Option<Duration>,
    max_memory: Option<usize>,
    max_recursion_depth: Option<usize>,
    max_operation_result_bytes: usize,
}

impl LimitedTracker {
    /// Create a new tracker from declarative limits. The wall-clock timer
    /// starts immediately.
    pub fn new(limits: &ResourceLimits) -> Self {
        Self {
            allocation_count: 0,
            memory_used: 0,
            start_time: Instant::now(),
            time_check_counter: 0,
            max_allocations: limits.max_allocations,
            max_duration: limits.max_duration,
            max_memory: limits.max_memory,
            max_recursion_depth: limits.max_recursion_depth,
            max_operation_result_bytes: limits
                .max_operation_result_bytes
                .unwrap_or(DEFAULT_MAX_OPERATION_RESULT_BYTES),
        }
    }

    /// Reset the wall-clock timer to now. Useful when re-entering an isolate.
    pub fn reset_timer(&mut self) {
        self.start_time = Instant::now();
        self.time_check_counter = 0;
    }
}

impl ResourceTracker for LimitedTracker {
    #[inline(always)]
    fn on_allocate(&mut self, size: usize) -> Result<(), ResourceError> {
        // Check limits BEFORE committing the increment — denied allocations
        // must not leave phantom counts that corrupt future on_free accounting.
        let new_count = self.allocation_count + 1;
        if let Some(limit) = self.max_allocations
            && new_count > limit
        {
            return Err(ResourceError::Allocation {
                count: new_count,
                limit,
            });
        }
        let new_memory = self.memory_used.saturating_add(size);
        if let Some(limit) = self.max_memory
            && new_memory > limit
        {
            return Err(ResourceError::Memory {
                used: new_memory,
                limit,
            });
        }
        // Commit only after both checks pass.
        self.allocation_count = new_count;
        self.memory_used = new_memory;
        Ok(())
    }

    #[inline(always)]
    fn on_free(&mut self, size: usize) {
        self.memory_used = self.memory_used.saturating_sub(size);
        self.allocation_count = self.allocation_count.saturating_sub(1);
    }

    #[inline(always)]
    fn on_grow(&mut self, additional_bytes: usize) -> Result<(), ResourceError> {
        self.memory_used = self.memory_used.saturating_add(additional_bytes);
        if let Some(limit) = self.max_memory
            && self.memory_used > limit
        {
            return Err(ResourceError::Memory {
                used: self.memory_used,
                limit,
            });
        }
        Ok(())
    }

    #[inline(always)]
    fn check_time(&mut self) -> Result<(), ResourceError> {
        self.time_check_counter += 1;
        if self.time_check_counter < TIME_CHECK_INTERVAL {
            return Ok(());
        }
        self.time_check_counter = 0;

        #[allow(clippy::collapsible_if)] // need `elapsed` in both condition and error
        if let Some(max_dur) = self.max_duration {
            let elapsed = self.start_time.elapsed();
            if elapsed > max_dur {
                return Err(ResourceError::Time {
                    elapsed_ms: elapsed.as_millis() as u64,
                    limit_ms: max_dur.as_millis() as u64,
                });
            }
        }
        Ok(())
    }

    #[inline(always)]
    fn check_recursion_depth(&mut self, depth: usize) -> Result<(), ResourceError> {
        if let Some(limit) = self.max_recursion_depth
            && depth > limit
        {
            return Err(ResourceError::Recursion { depth, limit });
        }
        Ok(())
    }

    #[inline(always)]
    fn check_operation_size(&mut self, op: &OperationEstimate) -> Result<(), ResourceError> {
        let estimated = match op.estimated_bytes() {
            Some(b) => b,
            None => {
                // Overflow means the result is absurdly large.
                return Err(ResourceError::OperationTooLarge {
                    op: op.label(),
                    estimated_bytes: usize::MAX,
                });
            }
        };
        if estimated > self.max_operation_result_bytes {
            return Err(ResourceError::OperationTooLarge {
                op: op.label(),
                estimated_bytes: estimated,
            });
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// UnlimitedTracker
// ---------------------------------------------------------------------------

/// No-op resource tracker for unconstrained dev builds.
///
/// Every hook returns `Ok(())` immediately. This is the default tracker
/// installed on every thread.
pub struct UnlimitedTracker;

impl ResourceTracker for UnlimitedTracker {
    #[inline(always)]
    fn on_allocate(&mut self, _size: usize) -> Result<(), ResourceError> {
        Ok(())
    }

    #[inline(always)]
    fn on_free(&mut self, _size: usize) {}

    #[inline(always)]
    fn on_grow(&mut self, _additional_bytes: usize) -> Result<(), ResourceError> {
        Ok(())
    }

    #[inline(always)]
    fn check_time(&mut self) -> Result<(), ResourceError> {
        Ok(())
    }

    #[inline(always)]
    fn check_recursion_depth(&mut self, _depth: usize) -> Result<(), ResourceError> {
        Ok(())
    }

    #[inline(always)]
    fn check_operation_size(&mut self, _op: &OperationEstimate) -> Result<(), ResourceError> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Global tracker factory
// ---------------------------------------------------------------------------

/// A factory function that creates a fresh [`ResourceTracker`] for each new
/// thread.  This solves the problem where threads spawned without calling
/// [`set_tracker`] would silently get an [`UnlimitedTracker`] even when the
/// host intended to enforce limits.
///
/// Installed via [`set_global_tracker_factory`] and cleared via
/// [`clear_global_tracker_factory`]; all threads created afterwards
/// initialise their thread-local tracker through this factory (or fall
/// back to [`UnlimitedTracker`] when unset).
///
/// Factory `fn` pointer alias — lifted out to keep
/// `static GLOBAL_TRACKER_FACTORY` readable and to satisfy
/// `clippy::type_complexity`.
type TrackerFactory = fn() -> Box<dyn ResourceTracker>;

/// `RwLock` rather than `OnceLock` because hosts (and tests) need to be
/// able to swap or clear the factory across the process lifetime — e.g.
/// to drop a per-test memory cap before the next test runs on the same
/// thread pool.
static GLOBAL_TRACKER_FACTORY: RwLock<Option<TrackerFactory>> = RwLock::new(None);

// ---------------------------------------------------------------------------
// Thread-local accessor
// ---------------------------------------------------------------------------

/// Initialize the thread-local tracker.  If a global factory has been set
/// (by another thread calling [`set_global_tracker_factory`]), use it to
/// create a fresh tracker; otherwise fall back to [`UnlimitedTracker`].
fn make_default_tracker() -> Box<dyn ResourceTracker> {
    let factory = match GLOBAL_TRACKER_FACTORY.read() {
        Ok(guard) => *guard,
        Err(poisoned) => *poisoned.into_inner(),
    };
    match factory {
        Some(f) => f(),
        None => Box::new(UnlimitedTracker),
    }
}

thread_local! {
    static TRACKER: RefCell<Box<dyn ResourceTracker>> =
        RefCell::new(make_default_tracker());
}

/// Access the thread-local resource tracker.
///
/// The tracker is set during WASM host initialization via [`set_tracker`].
/// Default: [`UnlimitedTracker`] (no limits), unless a global factory was
/// installed via [`set_global_tracker_factory`].
///
/// # Reentrancy
///
/// The closure `f` holds a mutable borrow on the thread-local tracker.
/// **Do not call `with_tracker` from within `f`** — this will panic with
/// "already mutably borrowed". Tracker hook implementations must not
/// allocate via paths that re-enter this function.
///
/// ```ignore
/// resource::with_tracker(|t| t.on_allocate(4096))?;
/// ```
#[inline(always)]
pub fn with_tracker<R>(f: impl FnOnce(&mut dyn ResourceTracker) -> R) -> R {
    TRACKER.with(|cell| {
        let mut borrow = cell.borrow_mut();
        f(&mut **borrow)
    })
}

/// Replace the thread-local resource tracker.
///
/// Call this at WASM host initialization time to install a [`LimitedTracker`]
/// (or any custom implementation) for the current thread.
pub fn set_tracker(tracker: Box<dyn ResourceTracker>) {
    TRACKER.with(|cell| {
        *cell.borrow_mut() = tracker;
    });
}

/// Set a global factory function that creates a fresh [`ResourceTracker`]
/// for every new thread.
///
/// Replaces any previously installed factory.  Typically called once during
/// host initialization, before spawning worker threads, but tests and
/// long-lived hosts may swap factories at lifecycle boundaries — pair with
/// [`clear_global_tracker_factory`] when tearing down a scope so threads
/// started afterwards revert to the default [`UnlimitedTracker`].
///
/// Unlike [`set_tracker`] (which only affects the calling thread), this
/// ensures every thread created *after* this call gets a properly configured
/// tracker (e.g. [`LimitedTracker`]) instead of the default
/// [`UnlimitedTracker`].
///
/// # Example
///
/// ```ignore
/// use molt_runtime::resource::{set_global_tracker_factory, LimitedTracker, ResourceLimits};
///
/// let limits = ResourceLimits {
///     max_memory: Some(64 * 1024 * 1024),
///     ..Default::default()
/// };
/// set_global_tracker_factory(|| Box::new(LimitedTracker::new(&limits)));
/// ```
///
/// Note: the factory is a `fn()` pointer (not a closure) to keep it `Send +
/// Sync` without boxing.  If you need captured state, use a static or
/// `OnceLock` for the configuration.
pub fn set_global_tracker_factory(factory: TrackerFactory) {
    if let Ok(mut guard) = GLOBAL_TRACKER_FACTORY.write() {
        *guard = Some(factory);
    }
}

/// Remove the global tracker factory installed by
/// [`set_global_tracker_factory`].  Threads spawned after this call fall
/// back to [`UnlimitedTracker`].
///
/// Lock poisoning (panic while another thread held the write lock) is
/// recovered into the inner guard — clearing on a poisoned lock leaves
/// the factory unset, which is the safe default.
pub fn clear_global_tracker_factory() {
    let mut guard = match GLOBAL_TRACKER_FACTORY.write() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    *guard = None;
}

// ---------------------------------------------------------------------------
// Standalone guard helpers for hot-path use in ops_arith.rs
// ---------------------------------------------------------------------------

/// Pre-emptive guard for integer exponentiation.
///
/// Estimates the result size of `base ** exponent` where `base` has `base_bits`
/// significant bits. Uses a 4x safety multiplier for intermediate allocations.
/// Returns `Err` with a human-readable message when the result would exceed ~10 MB.
///
/// This is intentionally a standalone function (not a method on ResourceTracker)
/// so that call sites in `ops_arith.rs` can use it with minimal ceremony.
#[inline]
pub fn check_pow_size(base_bits: u32, exponent: u64) -> Result<(), String> {
    // 80 million bits ≈ 10 MB (the default limit)
    const MAX_RESULT_BITS: u128 = 80_000_000;
    let estimated_bits = (base_bits as u128)
        .checked_mul(exponent as u128)
        .and_then(|v| v.checked_mul(4));
    match estimated_bits {
        None => {
            return Err(format!(
                "pow result too large: overflow (limit: {MAX_RESULT_BITS} bits)"
            ));
        }
        Some(bits) if bits > MAX_RESULT_BITS => {
            return Err(format!(
                "pow result too large: ~{bits} bits estimated (limit: {MAX_RESULT_BITS} bits)"
            ));
        }
        _ => {}
    }
    Ok(())
}

/// Pre-emptive guard for left shift amplification.
///
/// Estimates the result size of `value << shift` where `value` has `value_bits`
/// significant bits. Returns `Err` when the shift would produce > ~10 MB.
#[inline]
pub fn check_lshift_size(value_bits: u32, shift: u32) -> Result<(), String> {
    const MAX_RESULT_BITS: u64 = 80_000_000;
    let estimated_bits = (value_bits as u64) + (shift as u64);
    if estimated_bits > MAX_RESULT_BITS {
        return Err(format!(
            "left shift result too large: ~{} bits (limit: {} bits)",
            estimated_bits, MAX_RESULT_BITS
        ));
    }
    Ok(())
}

/// Pre-emptive guard for sequence repetition.
///
/// Returns `Err` when `item_bytes * count` would exceed ~10 MB.
#[inline]
pub fn check_repeat_size(item_bytes: usize, count: i64) -> Result<(), String> {
    const MAX_RESULT_BYTES: u64 = 10 * 1024 * 1024;
    if count <= 0 {
        return Ok(());
    }
    let estimated = (item_bytes as u64).saturating_mul(count as u64);
    if estimated > MAX_RESULT_BYTES {
        return Err(format!(
            "repetition result too large: ~{} bytes (limit: {} bytes)",
            estimated, MAX_RESULT_BYTES
        ));
    }
    Ok(())
}

/// Pre-emptive guard for BigInt multiplication.
///
/// Returns `Err` when the result of `a * b` (both BigInts) would exceed ~10 MB.
#[inline]
pub fn check_bigint_mul_size(a_bits: u64, b_bits: u64) -> Result<(), String> {
    const MAX_RESULT_BITS: u64 = 80_000_000;
    let estimated_bits = a_bits.saturating_add(b_bits);
    if estimated_bits > MAX_RESULT_BITS {
        return Err(format!(
            "integer multiplication result too large: ~{} bits (limit: {} bits)",
            estimated_bits, MAX_RESULT_BITS
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_limited(limits: ResourceLimits) -> LimitedTracker {
        LimitedTracker::new(&limits)
    }

    #[test]
    fn unlimited_tracker_always_succeeds() {
        let mut t = UnlimitedTracker;
        assert!(t.on_allocate(1 << 30).is_ok());
        t.on_free(1 << 30);
        assert!(t.on_grow(1 << 30).is_ok());
        assert!(t.check_time().is_ok());
        assert!(t.check_recursion_depth(10_000).is_ok());
        let op = OperationEstimate::Pow {
            base_bits: 1024,
            exponent: 1_000_000,
        };
        assert!(t.check_operation_size(&op).is_ok());
    }

    #[test]
    fn memory_limit_enforced() {
        let mut t = make_limited(ResourceLimits {
            max_memory: Some(1024),
            ..Default::default()
        });
        assert!(t.on_allocate(512).is_ok());
        assert!(t.on_allocate(512).is_ok());
        let err = t.on_allocate(1).unwrap_err();
        assert!(matches!(
            err,
            ResourceError::Memory {
                used: 1025,
                limit: 1024
            }
        ));
    }

    #[test]
    fn memory_freed_allows_reallocation() {
        let mut t = make_limited(ResourceLimits {
            max_memory: Some(1024),
            ..Default::default()
        });
        assert!(t.on_allocate(1024).is_ok());
        t.on_free(512);
        assert!(t.on_allocate(512).is_ok());
        assert!(
            t.on_allocate(1).unwrap_err()
                == ResourceError::Memory {
                    used: 1025,
                    limit: 1024
                }
        );
    }

    #[test]
    fn allocation_count_limit() {
        let mut t = make_limited(ResourceLimits {
            max_allocations: Some(3),
            ..Default::default()
        });
        assert!(t.on_allocate(1).is_ok());
        assert!(t.on_allocate(1).is_ok());
        assert!(t.on_allocate(1).is_ok());
        let err = t.on_allocate(1).unwrap_err();
        assert!(matches!(
            err,
            ResourceError::Allocation { count: 4, limit: 3 }
        ));
    }

    #[test]
    fn recursion_depth_limit() {
        let mut t = make_limited(ResourceLimits {
            max_recursion_depth: Some(100),
            ..Default::default()
        });
        assert!(t.check_recursion_depth(100).is_ok());
        let err = t.check_recursion_depth(101).unwrap_err();
        assert!(matches!(
            err,
            ResourceError::Recursion {
                depth: 101,
                limit: 100
            }
        ));
    }

    #[test]
    fn grow_enforces_memory_limit() {
        let mut t = make_limited(ResourceLimits {
            max_memory: Some(2048),
            ..Default::default()
        });
        assert!(t.on_allocate(1024).is_ok());
        assert!(t.on_grow(1024).is_ok());
        let err = t.on_grow(1).unwrap_err();
        assert!(matches!(
            err,
            ResourceError::Memory {
                used: 2049,
                limit: 2048
            }
        ));
    }

    #[test]
    fn time_check_rate_limited() {
        let mut t = make_limited(ResourceLimits {
            max_duration: Some(Duration::from_millis(0)),
            ..Default::default()
        });
        // First 9 calls should not actually check elapsed time.
        for _ in 0..(TIME_CHECK_INTERVAL - 1) {
            assert!(t.check_time().is_ok());
        }
        // 10th call samples Instant::elapsed(), which must exceed 0ns.
        // Sleep past mach_continuous_time's ~24ns resolution so the
        // elapsed measurement is deterministic on warm CPUs (the bare
        // 10-call sequence can fit inside a single clock tick on
        // AArch64 macOS, making elapsed read as Duration::ZERO).
        std::thread::sleep(Duration::from_micros(1));
        let err = t.check_time().unwrap_err();
        assert!(matches!(err, ResourceError::Time { .. }));
    }

    #[test]
    fn operation_pow_rejects_huge() {
        let mut t = make_limited(ResourceLimits {
            max_operation_result_bytes: Some(1024),
            ..Default::default()
        });
        let small = OperationEstimate::Pow {
            base_bits: 2,
            exponent: 10,
        };
        assert!(t.check_operation_size(&small).is_ok());

        let huge = OperationEstimate::Pow {
            base_bits: 64,
            exponent: 1_000_000,
        };
        let err = t.check_operation_size(&huge).unwrap_err();
        assert!(matches!(err, ResourceError::OperationTooLarge { .. }));
    }

    #[test]
    fn operation_repeat_rejects_huge() {
        let mut t = make_limited(ResourceLimits {
            max_operation_result_bytes: Some(10 * 1024 * 1024),
            ..Default::default()
        });
        let op = OperationEstimate::Repeat {
            item_bytes: 100,
            count: 1_000_000_000,
        };
        assert!(t.check_operation_size(&op).is_err());
    }

    #[test]
    fn operation_left_shift_overflow() {
        let mut t = make_limited(ResourceLimits {
            max_operation_result_bytes: Some(1024),
            ..Default::default()
        });
        let op = OperationEstimate::LeftShift {
            value_bits: 1,
            shift: 1_000_000,
        };
        assert!(t.check_operation_size(&op).is_err());
    }

    #[test]
    fn operation_multiply_small_ok() {
        let mut t = make_limited(ResourceLimits {
            max_operation_result_bytes: Some(1024),
            ..Default::default()
        });
        let op = OperationEstimate::Multiply {
            a_bits: 32,
            b_bits: 32,
        };
        // (32 + 32 + 7) / 8 = 8 bytes, well under 1024
        assert!(t.check_operation_size(&op).is_ok());
    }

    #[test]
    fn operation_string_replace() {
        let mut t = make_limited(ResourceLimits {
            max_operation_result_bytes: Some(1024),
            ..Default::default()
        });
        // Replacing "a" with "bbb" in a 100-char string, up to 100 times:
        // growth_per = 2, max_replacements = min(100, 100/1) = 100
        // result = 100 + 2*100 = 300
        let op = OperationEstimate::StringReplace {
            input_len: 100,
            old_len: 1,
            new_len: 3,
            count: 100,
        };
        assert!(t.check_operation_size(&op).is_ok());

        // Pathological: replace "" with large string in large input
        let op_big = OperationEstimate::StringReplace {
            input_len: 1_000_000,
            old_len: 0,
            new_len: 100,
            count: usize::MAX,
        };
        assert!(t.check_operation_size(&op_big).is_err());
    }

    #[test]
    fn with_tracker_explicit_unlimited() {
        // Explicitly install UnlimitedTracker to ensure this test is
        // independent of any global factory that may have been set by
        // other tests in the same process.
        set_tracker(Box::new(UnlimitedTracker));
        let result = with_tracker(|t| t.on_allocate(1 << 30));
        assert!(result.is_ok());
    }

    #[test]
    fn set_tracker_installs_limited() {
        let limits = ResourceLimits {
            max_memory: Some(256),
            ..Default::default()
        };
        set_tracker(Box::new(LimitedTracker::new(&limits)));

        let result = with_tracker(|t| t.on_allocate(512));
        assert!(result.is_err());

        // Restore default so other tests are not affected.
        set_tracker(Box::new(UnlimitedTracker));
    }

    #[test]
    fn resource_error_display() {
        let err = ResourceError::Memory {
            used: 2048,
            limit: 1024,
        };
        let msg = err.to_string();
        assert!(msg.contains("2048"));
        assert!(msg.contains("1024"));

        let err = ResourceError::OperationTooLarge {
            op: "test".into(),
            estimated_bytes: 999,
        };
        assert!(err.to_string().contains("test"));
    }

    #[test]
    fn estimated_bytes_overflow_returns_none() {
        let op = OperationEstimate::Pow {
            base_bits: u32::MAX,
            exponent: u64::MAX,
        };
        assert!(op.estimated_bytes().is_none());

        let op = OperationEstimate::Repeat {
            item_bytes: usize::MAX,
            count: u64::MAX,
        };
        assert!(op.estimated_bytes().is_none());
    }

    #[test]
    fn global_tracker_factory_inherited_by_spawned_thread() {
        // Install a factory that creates a LimitedTracker with a tiny
        // memory cap.  Threads spawned after this call should inherit it.
        fn factory() -> Box<dyn ResourceTracker> {
            Box::new(LimitedTracker::new(&ResourceLimits {
                max_memory: Some(128),
                ..Default::default()
            }))
        }
        set_global_tracker_factory(factory);

        // Spawn a thread that NEVER calls set_tracker.
        // Its thread-local should be initialized via the factory.
        let handle = std::thread::spawn(|| {
            // 256 bytes exceeds the 128-byte limit from the factory.
            with_tracker(|t| t.on_allocate(256))
        });
        let result = handle.join().expect("child thread panicked");
        // Clear the factory before asserting so a panic from the assert
        // leaves no leaked global state behind to poison parallel test
        // siblings (cargo test runs many tests on a shared thread pool;
        // a permanent 128-byte memory cap turns later tests' alloc_string
        // calls into spurious null returns).
        clear_global_tracker_factory();
        assert!(
            result.is_err(),
            "spawned thread should have inherited the limited tracker \
             from the global factory, but allocation of 256 bytes succeeded"
        );
    }
}
