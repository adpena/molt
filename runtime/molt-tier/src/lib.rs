//! Tiered execution coordinator for Molt + Monty.
//!
//! Implements a V8-style tiered execution model:
//!
//! ```text
//! Cold code → Monty interprets (<1μs startup)
//!      │
//!      │  execution counter reaches threshold
//!      │
//! Hot path → Molt AOT-compiles to WASM (10-100x faster)
//!      │
//!      │  cached compiled module
//!      │
//! Subsequent calls → execute compiled WASM
//! ```
//!
//! # Architecture
//!
//! The coordinator maintains per-function call counters. When a function's
//! call count exceeds the tier-up threshold, it triggers background
//! compilation via Molt and atomically swaps the entry point.
//!
//! ## Tiers
//!
//! - **Tier 0 (Interpreted):** Python source executed by Monty interpreter.
//!   Instant startup, ~1x CPython speed.
//! - **Tier 1 (Compiled):** AOT-compiled to WASM/native by Molt.
//!   Compilation takes 50-200ms, execution 10-100x faster.
//!
//! ## Thread Safety
//!
//! The coordinator is designed for concurrent access:
//! - Call counters use `AtomicU32` (no locks on the hot path)
//! - Tier-up compilation runs on a background thread
//! - Entry point swap is atomic (no request stalls)
//! - Deoptimization path if type assumptions change

use std::collections::HashMap;
use std::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Execution tier for a function.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Tier {
    /// Interpreted by Monty (or CPython fallback).
    Interpreted = 0,
    /// Being compiled by Molt in the background.
    Compiling = 1,
    /// Compiled and ready to execute.
    Compiled = 2,
    /// Deoptimized back to interpreted (type assumption violated).
    Deoptimized = 3,
}

impl Tier {
    fn from_u8(v: u8) -> Self {
        match v {
            0 => Self::Interpreted,
            1 => Self::Compiling,
            2 => Self::Compiled,
            3 => Self::Deoptimized,
            _ => Self::Interpreted,
        }
    }
}

/// Configuration for the tiered execution coordinator.
#[derive(Debug, Clone)]
pub struct TierConfig {
    /// Number of calls before triggering tier-up compilation.
    pub tier_up_threshold: u32,
    /// Maximum number of concurrent background compilations.
    pub max_concurrent_compilations: usize,
    /// Whether to enable deoptimization on type assumption violations.
    pub enable_deoptimization: bool,
}

impl Default for TierConfig {
    fn default() -> Self {
        Self {
            tier_up_threshold: 100,
            max_concurrent_compilations: 2,
            enable_deoptimization: true,
        }
    }
}

/// Per-function execution state tracked by the coordinator.
#[derive(Debug)]
pub struct FunctionState {
    /// Current execution tier.
    tier: AtomicU8,
    /// Call counter (incremented on every invocation).
    call_count: AtomicU32,
    /// Compiled artifact (set after tier-up compilation completes).
    compiled: Mutex<Option<CompiledArtifact>>,
}

impl FunctionState {
    fn new() -> Self {
        Self {
            tier: AtomicU8::new(Tier::Interpreted as u8),
            call_count: AtomicU32::new(0),
            compiled: Mutex::new(None),
        }
    }

    /// Get the current execution tier.
    pub fn tier(&self) -> Tier {
        Tier::from_u8(self.tier.load(Ordering::Acquire))
    }

    /// Get the current call count.
    pub fn call_count(&self) -> u32 {
        self.call_count.load(Ordering::Relaxed)
    }

    /// Record a call and return whether tier-up should be triggered.
    pub fn record_call(&self, threshold: u32) -> bool {
        let prev = self.call_count.fetch_add(1, Ordering::Relaxed);
        let count = prev + 1;
        // Trigger tier-up exactly once: when count == threshold
        // and we're still in Interpreted state
        count == threshold && self.tier.load(Ordering::Acquire) == Tier::Interpreted as u8
    }

    /// Atomically transition to Compiling state.
    /// Returns false if another thread already started compilation.
    pub fn start_compilation(&self) -> bool {
        self.tier
            .compare_exchange(
                Tier::Interpreted as u8,
                Tier::Compiling as u8,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }

    /// Complete compilation and swap to Compiled state.
    pub fn complete_compilation(&self, artifact: CompiledArtifact) {
        *self.compiled.lock().unwrap() = Some(artifact);
        self.tier.store(Tier::Compiled as u8, Ordering::Release);
    }

    /// Deoptimize back to Interpreted (type assumption violated).
    pub fn deoptimize(&self) {
        self.tier.store(Tier::Deoptimized as u8, Ordering::Release);
        *self.compiled.lock().unwrap() = None;
    }

    /// Get the compiled artifact, if available.
    pub fn compiled_artifact(&self) -> Option<CompiledArtifact> {
        self.compiled.lock().unwrap().clone()
    }
}

/// A compiled function artifact (content-addressed for caching).
#[derive(Debug, Clone)]
pub struct CompiledArtifact {
    /// Content hash of the compiled artifact (for cache lookup).
    pub content_hash: [u8; 32],
    /// The compiled bytes (WASM or native object code).
    pub bytes: Arc<Vec<u8>>,
    /// Source function name for diagnostics.
    pub function_name: String,
    /// Compilation time in milliseconds.
    pub compile_time_ms: u64,
}

/// The tiered execution coordinator.
///
/// Maintains per-function state, triggers background compilation when
/// call counts exceed the threshold, and provides compiled artifacts
/// for hot functions.
pub struct TierCoordinator {
    config: TierConfig,
    /// Per-function execution state, keyed by fully qualified function name.
    functions: Mutex<HashMap<String, Arc<FunctionState>>>,
    /// Content-addressed artifact cache.
    cache: Mutex<HashMap<[u8; 32], CompiledArtifact>>,
    /// Count of currently active background compilations.
    active_compilations: AtomicU32,
}

impl TierCoordinator {
    /// Create a new coordinator with the given configuration.
    pub fn new(config: TierConfig) -> Self {
        Self {
            config,
            functions: Mutex::new(HashMap::new()),
            cache: Mutex::new(HashMap::new()),
            active_compilations: AtomicU32::new(0),
        }
    }

    /// Create a coordinator with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(TierConfig::default())
    }

    /// Get or create the execution state for a function.
    pub fn get_function(&self, name: &str) -> Arc<FunctionState> {
        let mut functions = self.functions.lock().unwrap();
        functions
            .entry(name.to_string())
            .or_insert_with(|| Arc::new(FunctionState::new()))
            .clone()
    }

    /// Record a function call and return the execution decision.
    ///
    /// This is the hot-path entry point. It increments the call counter
    /// and returns what the caller should do.
    pub fn on_call(&self, name: &str) -> ExecutionDecision {
        let state = self.get_function(name);

        match state.tier() {
            Tier::Compiled => {
                if let Some(artifact) = state.compiled_artifact() {
                    return ExecutionDecision::RunCompiled(artifact);
                }
                // Compiled artifact was lost (shouldn't happen)
                ExecutionDecision::Interpret
            }
            Tier::Compiling => {
                // Still compiling — interpret for now
                state.call_count.fetch_add(1, Ordering::Relaxed);
                ExecutionDecision::Interpret
            }
            Tier::Interpreted | Tier::Deoptimized => {
                if state.record_call(self.config.tier_up_threshold) {
                    // Threshold reached — try to start compilation
                    if self.can_start_compilation() && state.start_compilation() {
                        self.active_compilations.fetch_add(1, Ordering::Relaxed);
                        return ExecutionDecision::TierUp(state);
                    }
                }
                ExecutionDecision::Interpret
            }
        }
    }

    /// Notify the coordinator that a compilation completed.
    pub fn compilation_done(&self, name: &str, artifact: CompiledArtifact) {
        let state = self.get_function(name);
        // Cache the artifact by content hash
        self.cache
            .lock()
            .unwrap()
            .insert(artifact.content_hash, artifact.clone());
        state.complete_compilation(artifact);
        self.active_compilations.fetch_sub(1, Ordering::Relaxed);
    }

    /// Check if a compiled artifact exists in the cache.
    pub fn cached_artifact(&self, content_hash: &[u8; 32]) -> Option<CompiledArtifact> {
        self.cache.lock().unwrap().get(content_hash).cloned()
    }

    /// Get a snapshot of all function states for diagnostics.
    pub fn snapshot(&self) -> Vec<(String, Tier, u32)> {
        let functions = self.functions.lock().unwrap();
        functions
            .iter()
            .map(|(name, state)| (name.clone(), state.tier(), state.call_count()))
            .collect()
    }

    fn can_start_compilation(&self) -> bool {
        (self.active_compilations.load(Ordering::Relaxed) as usize)
            < self.config.max_concurrent_compilations
    }
}

/// What the caller should do after `on_call`.
#[derive(Debug)]
pub enum ExecutionDecision {
    /// Interpret the function (Monty or CPython fallback).
    Interpret,
    /// Execute the compiled artifact.
    RunCompiled(CompiledArtifact),
    /// Tier-up: compile this function in the background, interpret for now.
    /// The caller should spawn a compilation task and call `compilation_done`
    /// when it completes.
    TierUp(Arc<FunctionState>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_function_starts_interpreted() {
        let coord = TierCoordinator::with_defaults();
        let state = coord.get_function("test.foo");
        assert_eq!(state.tier(), Tier::Interpreted);
        assert_eq!(state.call_count(), 0);
    }

    #[test]
    fn calls_below_threshold_stay_interpreted() {
        let coord = TierCoordinator::new(TierConfig {
            tier_up_threshold: 100,
            ..Default::default()
        });
        for _ in 0..99 {
            match coord.on_call("test.foo") {
                ExecutionDecision::Interpret => {}
                other => panic!("expected Interpret, got {:?}", other),
            }
        }
        let state = coord.get_function("test.foo");
        assert_eq!(state.tier(), Tier::Interpreted);
        assert_eq!(state.call_count(), 99);
    }

    #[test]
    fn threshold_triggers_tier_up() {
        let coord = TierCoordinator::new(TierConfig {
            tier_up_threshold: 5,
            ..Default::default()
        });
        for _ in 0..4 {
            assert!(matches!(
                coord.on_call("test.foo"),
                ExecutionDecision::Interpret
            ));
        }
        // 5th call should trigger tier-up
        match coord.on_call("test.foo") {
            ExecutionDecision::TierUp(state) => {
                assert_eq!(state.tier(), Tier::Compiling);
            }
            other => panic!("expected TierUp, got {:?}", other),
        }
    }

    #[test]
    fn compilation_completes_to_compiled() {
        let coord = TierCoordinator::new(TierConfig {
            tier_up_threshold: 1,
            ..Default::default()
        });
        // First call triggers tier-up
        coord.on_call("test.foo");
        // Simulate compilation completing
        let artifact = CompiledArtifact {
            content_hash: [0xAB; 32],
            bytes: Arc::new(vec![0, 1, 2, 3]),
            function_name: "test.foo".into(),
            compile_time_ms: 50,
        };
        coord.compilation_done("test.foo", artifact.clone());
        // Next call should return compiled
        match coord.on_call("test.foo") {
            ExecutionDecision::RunCompiled(a) => {
                assert_eq!(a.function_name, "test.foo");
                assert_eq!(a.compile_time_ms, 50);
            }
            other => panic!("expected RunCompiled, got {:?}", other),
        }
    }

    #[test]
    fn deoptimization_resets_to_interpreted() {
        let coord = TierCoordinator::new(TierConfig {
            tier_up_threshold: 1,
            ..Default::default()
        });
        coord.on_call("test.foo");
        let artifact = CompiledArtifact {
            content_hash: [0xCD; 32],
            bytes: Arc::new(vec![]),
            function_name: "test.foo".into(),
            compile_time_ms: 10,
        };
        coord.compilation_done("test.foo", artifact);
        // Deoptimize
        let state = coord.get_function("test.foo");
        state.deoptimize();
        assert_eq!(state.tier(), Tier::Deoptimized);
        // Next call should interpret
        assert!(matches!(
            coord.on_call("test.foo"),
            ExecutionDecision::Interpret
        ));
    }

    #[test]
    fn max_concurrent_compilations_enforced() {
        let coord = TierCoordinator::new(TierConfig {
            tier_up_threshold: 1,
            max_concurrent_compilations: 1,
            ..Default::default()
        });
        // First function triggers tier-up
        match coord.on_call("test.a") {
            ExecutionDecision::TierUp(_) => {}
            other => panic!("expected TierUp for a, got {:?}", other),
        }
        // Second function should NOT tier-up (max concurrent reached)
        match coord.on_call("test.b") {
            ExecutionDecision::Interpret => {}
            other => panic!("expected Interpret for b (max concurrent), got {:?}", other),
        }
    }

    #[test]
    fn snapshot_shows_all_functions() {
        let coord = TierCoordinator::new(TierConfig {
            tier_up_threshold: 100,
            ..Default::default()
        });
        coord.on_call("mod.a");
        coord.on_call("mod.a");
        coord.on_call("mod.b");
        let snap = coord.snapshot();
        assert_eq!(snap.len(), 2);
        let a = snap.iter().find(|(n, _, _)| n == "mod.a").unwrap();
        assert_eq!(a.2, 2); // 2 calls
        let b = snap.iter().find(|(n, _, _)| n == "mod.b").unwrap();
        assert_eq!(b.2, 1); // 1 call
    }

    #[test]
    fn cached_artifact_lookup() {
        let coord = TierCoordinator::with_defaults();
        let hash = [0xFF; 32];
        assert!(coord.cached_artifact(&hash).is_none());
        // After compilation completes, artifact should be cached
        let artifact = CompiledArtifact {
            content_hash: hash,
            bytes: Arc::new(vec![42]),
            function_name: "test.cached".into(),
            compile_time_ms: 5,
        };
        coord.cache.lock().unwrap().insert(hash, artifact);
        assert!(coord.cached_artifact(&hash).is_some());
    }
}
