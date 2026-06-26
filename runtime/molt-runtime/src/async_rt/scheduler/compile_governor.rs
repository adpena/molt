use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use super::async_trace_enabled;

// ---------------------------------------------------------------------------
// MOL-213: Rate-limit scheduler v2 - self-healing compilation governor
// ---------------------------------------------------------------------------

/// Optimization level that the scheduler can degrade to under load.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum OptLevel {
    /// Full optimization pipeline (default).
    Full,
    /// Skip expensive passes (escape analysis, specialization, LICM).
    Reduced,
    /// Minimal: only mandatory lowering, no optimization.
    Minimal,
}

#[allow(dead_code)]
impl OptLevel {
    fn label(self) -> &'static str {
        match self {
            OptLevel::Full => "full",
            OptLevel::Reduced => "reduced",
            OptLevel::Minimal => "minimal",
        }
    }
}

/// Per-window accounting for the rate limiter.
#[allow(dead_code)]
struct RateLimitWindow {
    /// Start of the current window.
    window_start: Instant,
    /// Number of compilation tasks admitted in this window.
    admitted: u64,
}

/// Self-healing rate-limit governor for compilation tasks.
///
/// Tracks wall-time budget overruns and automatically degrades the
/// optimization level when the system is overloaded, resuming full
/// optimization once pressure subsides.
#[allow(dead_code)]
pub(crate) struct CompileRateLimiter {
    /// Maximum compilation tasks allowed per time window.
    max_tasks_per_window: u64,
    /// Duration of each rate-limit window.
    window_duration: Duration,
    /// Wall-time budget per individual compilation task.
    task_wall_time_budget: Duration,
    /// Cooldown: how long to stay degraded after an overload event.
    cooldown_duration: Duration,

    // --- mutable state (behind Mutex) ---
    inner: Mutex<RateLimiterState>,
}

#[allow(dead_code)]
struct RateLimiterState {
    window: RateLimitWindow,
    /// Current effective optimization level.
    current_level: OptLevel,
    /// When the last degradation happened (for cooldown tracking).
    last_degrade_at: Option<Instant>,
    /// Consecutive overrun count (resets on recovery).
    consecutive_overruns: u64,
    /// Total degrade events since process start.
    total_degrade_events: u64,
    /// Total tasks rejected by rate limit.
    total_rejected: u64,
}

#[allow(dead_code)]
impl CompileRateLimiter {
    pub(crate) fn from_env() -> Self {
        let max_tasks: u64 = std::env::var("MOLT_COMPILE_RATE_MAX")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(50);

        let window_secs: f64 = std::env::var("MOLT_COMPILE_RATE_WINDOW_SECS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(60.0);

        let budget_secs: f64 = std::env::var("MOLT_COMPILE_TASK_BUDGET_SECS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(30.0);

        let cooldown_secs: f64 = std::env::var("MOLT_COMPILE_COOLDOWN_SECS")
            .ok()
            .and_then(|v| v.trim().parse().ok())
            .unwrap_or(120.0);

        let now = Instant::now();
        Self {
            max_tasks_per_window: max_tasks.max(1),
            window_duration: Duration::from_secs_f64(window_secs.max(1.0)),
            task_wall_time_budget: Duration::from_secs_f64(budget_secs.max(1.0)),
            cooldown_duration: Duration::from_secs_f64(cooldown_secs.max(0.0)),
            inner: Mutex::new(RateLimiterState {
                window: RateLimitWindow {
                    window_start: now,
                    admitted: 0,
                },
                current_level: OptLevel::Full,
                last_degrade_at: None,
                consecutive_overruns: 0,
                total_degrade_events: 0,
                total_rejected: 0,
            }),
        }
    }

    /// Try to admit a compilation task.  Returns the effective optimization
    /// level if admitted, or `None` if the rate limit is exceeded.
    pub(crate) fn try_admit(&self) -> Option<OptLevel> {
        let now = Instant::now();
        let mut state = self.inner.lock().unwrap();

        // Rotate window if expired.
        if now.duration_since(state.window.window_start) >= self.window_duration {
            state.window = RateLimitWindow {
                window_start: now,
                admitted: 0,
            };
        }

        // Check rate limit.
        if state.window.admitted >= self.max_tasks_per_window {
            state.total_rejected += 1;
            if async_trace_enabled() {
                eprintln!(
                    "molt compile governor: rate-limited (admitted={} max={})",
                    state.window.admitted, self.max_tasks_per_window
                );
            }
            return None;
        }

        // Maybe recover from degraded state.
        self.maybe_recover(&mut state, now);

        state.window.admitted += 1;
        Some(state.current_level)
    }

    /// Called when a compilation task exceeds its wall-time budget.
    /// Triggers automatic degradation.
    pub(crate) fn report_overrun(&self, task_elapsed: Duration) {
        let now = Instant::now();
        let mut state = self.inner.lock().unwrap();
        state.consecutive_overruns += 1;

        let prev_level = state.current_level;
        let new_level = match state.consecutive_overruns {
            1..=2 => OptLevel::Reduced,
            _ => OptLevel::Minimal,
        };

        if new_level < prev_level {
            // Already at this level or higher; just update timestamp.
        }

        if new_level != state.current_level {
            state.current_level = new_level;
            state.total_degrade_events += 1;
            state.last_degrade_at = Some(now);
            eprintln!(
                "molt compile governor: DEGRADE {} -> {} (task_elapsed={:.2}s budget={:.2}s overruns={})",
                prev_level.label(),
                new_level.label(),
                task_elapsed.as_secs_f64(),
                self.task_wall_time_budget.as_secs_f64(),
                state.consecutive_overruns,
            );
        } else if state.current_level != OptLevel::Full {
            state.last_degrade_at = Some(now);
        }
    }

    /// Check the wall-time budget and trigger degradation if exceeded.
    pub(crate) fn check_budget(&self, task_start: Instant) {
        let elapsed = task_start.elapsed();
        if elapsed > self.task_wall_time_budget {
            self.report_overrun(elapsed);
        }
    }

    /// Returns the current wall-time budget for tasks.
    pub(crate) fn task_budget(&self) -> Duration {
        self.task_wall_time_budget
    }

    fn maybe_recover(&self, state: &mut RateLimiterState, now: Instant) {
        if state.current_level == OptLevel::Full {
            return;
        }
        let Some(degrade_at) = state.last_degrade_at else {
            return;
        };
        if now.duration_since(degrade_at) >= self.cooldown_duration {
            let prev = state.current_level;
            // Step up one level at a time for graceful recovery.
            state.current_level = match prev {
                OptLevel::Minimal => OptLevel::Reduced,
                OptLevel::Reduced => OptLevel::Full,
                OptLevel::Full => OptLevel::Full,
            };
            state.consecutive_overruns = 0;
            state.last_degrade_at = if state.current_level != OptLevel::Full {
                Some(now)
            } else {
                None
            };
            eprintln!(
                "molt compile governor: RECOVER {} -> {} (cooldown={:.0}s)",
                prev.label(),
                state.current_level.label(),
                self.cooldown_duration.as_secs_f64(),
            );
        }
    }

    /// Snapshot for telemetry / dashboard integration.
    pub(crate) fn status_snapshot(&self) -> CompileGovernorSnapshot {
        let state = self.inner.lock().unwrap();
        CompileGovernorSnapshot {
            opt_level: state.current_level.label(),
            window_admitted: state.window.admitted,
            max_tasks_per_window: self.max_tasks_per_window,
            consecutive_overruns: state.consecutive_overruns,
            total_degrade_events: state.total_degrade_events,
            total_rejected: state.total_rejected,
            is_degraded: state.current_level != OptLevel::Full,
        }
    }
}

/// Telemetry snapshot for the compile rate-limit governor.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub(crate) struct CompileGovernorSnapshot {
    pub opt_level: &'static str,
    pub window_admitted: u64,
    pub max_tasks_per_window: u64,
    pub consecutive_overruns: u64,
    pub total_degrade_events: u64,
    pub total_rejected: u64,
    pub is_degraded: bool,
}

/// Global singleton for the compile governor.
#[allow(dead_code)]
pub(crate) fn compile_rate_limiter() -> &'static CompileRateLimiter {
    static LIMITER: OnceLock<CompileRateLimiter> = OnceLock::new();
    LIMITER.get_or_init(CompileRateLimiter::from_env)
}
