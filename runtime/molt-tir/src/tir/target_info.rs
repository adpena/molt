//! Unified cost model / `TargetTransformInfo` (Tier-0 substrate **S2**).
//!
//! Before this module existed, every profitability threshold in the TIR
//! pipeline and the SimpleIR inliner was a free-floating magic constant: the
//! inline budget (`30`), the PGO-hot inline budget (`80`), the PGO-hot call
//! threshold (`1000`), the unroll trip cap (`8`) and body cap (`20`), the
//! vectorize SIMD width (`2`), the polyhedral tile size (`32`), and the
//! branchless-count rewrite (applied unconditionally). Each lived next to the
//! pass that read it, so there was no single, target-aware, tunable source of
//! truth — and no way to make a decision depend on the actual backend (native
//! Cranelift / WASM / LLVM / Luau) or build profile.
//!
//! [`TargetInfo`] is that single source of truth. It is consulted by every
//! profitability decision in the pipeline. The [`PassManager`](crate::tir::pass_manager)
//! owns one and threads `&TargetInfo` to each pass's `run`, exactly as it
//! threads the [`AnalysisManager`](crate::tir::analysis::AnalysisManager); the
//! SimpleIR inliner takes one by reference too.
//!
//! ## Behavior-preserving defaults (the firewall)
//!
//! S2 is a *refactor*: it deletes the magic constants and replaces each with a
//! `tti.*` query, but the default field values reproduce the exact prior
//! literals so that today's decisions are bit-for-bit unchanged. The
//! [`TargetInfo::native_release_fast`] constructor is the behavioral baseline —
//! every field equals the constant it replaced (inline `30`, hot-inline `80`,
//! hot-call `1000`, unroll trip `8` / body `20`, vector width `2`, tile `32`,
//! branchless rewrite profitable). The
//! [`tests::native_release_fast_reproduces_legacy_literals`] firewall test pins
//! this; it must stay green forever.
//!
//! Target-aware constructors ([`TargetInfo::native_from_simd_caps`],
//! [`TargetInfo::from_llvm_feature_string`], [`TargetInfo::wasm_release_fast`],
//! [`TargetInfo::llvm_release_fast`]) refine *only* the fields whose
//! refinement is provably behavior-neutral today (the SIMD vector width is a
//! dead annotation — written by `vectorize` but read by no backend lowering —
//! so widening it on an AVX2 host changes no emitted code) or whose target
//! differs structurally (WASM's branch-mispredict cost). They never alter a
//! field that feeds a live codegen decision out from under the firewall
//! baseline.
//!
//! ## PGO hook (reserved)
//!
//! [`ProfileData`] is the profile-guided-optimization hook. It is plumbed
//! through the query API ([`TargetInfo::inline_budget`],
//! [`TargetInfo::is_pgo_hot`]) so a future Tier-5 W1 PGO pass can populate it
//! with real counter data, but no profiling exists yet: it is always `None` in
//! the constructors here. The SimpleIR inliner already consumed a separate
//! `SimpleIR::profile` field for its hot-function set; S2 does not change that
//! wiring — `ProfileData` is the *cost-model-side* reservation for when the
//! TIR pipeline grows PGO-directed decisions.

/// Which backend the cost model is targeting. Distinct backends have distinct
/// instruction-selection capabilities and microarchitectural costs (e.g. WASM
/// has no branch-target-buffer the way a native core does), so a profitability
/// decision can legitimately differ per target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
    /// Native code via the Cranelift backend.
    NativeCranelift,
    /// WebAssembly module (`wasm32`).
    Wasm,
    /// Native code via the LLVM backend.
    Llvm,
    /// Luau bytecode.
    Luau,
}

/// The build profile (optimization aggressiveness). Mirrors the cargo profiles
/// the project ships (`release-fast`, `dev-fast`, `debug-with-asserts`); a more
/// aggressive profile spends more compile time chasing runtime speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BuildProfile {
    /// `release-fast` — the production profile; maximal runtime speed.
    ReleaseFast,
    /// `dev-fast` — fast incremental builds with most optimizations on.
    DevFast,
    /// `debug-with-asserts` — debug build with assertions enabled.
    DebugWithAsserts,
}

/// Host SIMD capabilities used to size the vectorizer's lane count.
///
/// Decoupled from any backend crate (cranelift / inkwell): the native and LLVM
/// entry points detect the host's vector features and hand a plain `SimdCaps`
/// to the constructor, keeping this module free of feature-gated dependencies.
/// The lane-width mapping is the conventional one: SSE2/NEON 128-bit → 2 i64/f64
/// lanes, AVX2 256-bit → 4, AVX-512F 512-bit → 8.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SimdCaps {
    /// 128-bit vectors (SSE2 on x86, always present on aarch64 NEON). Implied
    /// by `avx2`/`avx512f`; kept explicit for completeness.
    pub avx: bool,
    /// 256-bit integer/float vectors (AVX2).
    pub avx2: bool,
    /// 512-bit vectors (AVX-512 Foundation).
    pub avx512f: bool,
}

impl SimdCaps {
    /// The conservative 128-bit baseline (no wide vectors): the lane width the
    /// pipeline used unconditionally before S2.
    pub const BASELINE_128: SimdCaps = SimdCaps {
        avx: false,
        avx2: false,
        avx512f: false,
    };

    /// Detect the host's vector features. On x86_64 this probes AVX/AVX2/
    /// AVX-512F via `std::arch::is_x86_feature_detected!` (the same mechanism
    /// the Cranelift ISA builder already uses to enable BMI/POPCNT). On
    /// aarch64, NEON's 128-bit vectors are baseline, so this reports the
    /// conservative 128-bit width (2 lanes) — matching the prior behavior and
    /// not over-claiming SVE.
    pub fn detect_host() -> SimdCaps {
        #[cfg(target_arch = "x86_64")]
        {
            SimdCaps {
                avx: std::arch::is_x86_feature_detected!("avx"),
                avx2: std::arch::is_x86_feature_detected!("avx2"),
                avx512f: std::arch::is_x86_feature_detected!("avx512f"),
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            SimdCaps::BASELINE_128
        }
    }

    /// The i64/f64 lane count these capabilities support. 512-bit → 8, 256-bit
    /// → 4, otherwise the 128-bit baseline → 2. (i64 and f64 share a lane
    /// width, so one value covers both.)
    fn lane_width_64(self) -> u32 {
        if self.avx512f {
            8
        } else if self.avx2 {
            4
        } else {
            2
        }
    }
}

/// Profile-guided-optimization data. **Reserved for Tier-5 W1** — no profiling
/// pipeline populates this yet, so it is always `None` in every constructor
/// here. It exists so the cost-model query API (`inline_budget`, `is_pgo_hot`)
/// already has the shape a future PGO pass will fill in, rather than bolting it
/// on later.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProfileData {
    /// Names of functions a profile run found to be "hot" (executed above the
    /// hot-call threshold). Empty until PGO lands.
    pub hot_functions: std::collections::BTreeSet<String>,
}

/// The unified cost model. One instance describes the latencies, vector widths,
/// cache hierarchy, and profitability thresholds for a given
/// (target, profile) pair. Passes consult it instead of hardcoding constants.
///
/// Every field documents the legacy constant it subsumes; the
/// [`TargetInfo::native_release_fast`] constructor sets each to that exact
/// literal (the behavioral firewall).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    /// Backend this cost model targets.
    pub target: TargetKind,
    /// Build profile (optimization aggressiveness).
    pub profile: BuildProfile,

    // -- Latency / overhead model (abstract cost units) --------------------
    /// Cost of an integer binary op (add/sub/and/…). The unit baseline `1`.
    pub int_binop_cost: u32,
    /// Penalty (in the same units) of a mispredicted conditional branch.
    /// Drives [`TargetInfo::is_profitable_branchless_rewrite`]: a positive
    /// penalty means converting an unpredictable branch to branchless code
    /// (e.g. branchless boolean counting) pays off. Native cores have a deep
    /// pipeline (high penalty); WASM is interpreted/JITed by the host engine
    /// with a shallower model (lower penalty) but a branch still costs.
    pub branch_mispredict_cost: u32,
    /// Cost of a (non-inlined) function call: argument marshalling, the call/
    /// return, and the lost cross-call optimization. Feeds future inline ROI.
    pub call_overhead: u32,

    // -- Inlining thresholds (subsume passes.rs inline constants) ----------
    /// Max callee op count to inline by default. **Subsumes `INLINE_OP_LIMIT`
    /// (30).** Overridable per-build via `MOLT_INLINE_LIMIT`.
    pub inline_op_limit: usize,
    /// Larger op budget for PGO-hot callees. **Subsumes
    /// `PGO_HOT_INLINE_OP_LIMIT` (80).**
    pub inline_hot_op_limit: usize,
    /// Call-count above which a function is "hot" for inlining. **Subsumes
    /// `_PGO_HOT_CALL_THRESHOLD` (1000).** Consumed when `profile_data` is
    /// populated (Tier-5 W1).
    pub pgo_hot_call_threshold: u64,

    // -- Loop unrolling (subsume loop_unroll.rs constants) -----------------
    /// Max constant trip count to fully unroll. **Subsumes
    /// `MAX_UNROLL_TRIP_COUNT` (8).**
    pub unroll_max_trip: i64,
    /// Max loop-body op count to unroll (anti-bloat). **Subsumes
    /// `MAX_UNROLL_OPS` (20).**
    pub unroll_max_body: usize,

    // -- Vectorization (subsume vectorize.rs simd_width) -------------------
    /// SIMD lane count for i64 elements. **Subsumes the hardcoded `simd_width
    /// = 2`.** `native_release_fast` keeps the conservative 2; the
    /// target-aware native/LLVM constructors widen it from host SIMD caps
    /// (this attr is currently read by no backend lowering, so widening it is
    /// behavior-neutral — see the module docs).
    pub vector_width_i64: u32,
    /// SIMD lane count for f64 elements. Same width as i64 on every target we
    /// emit for (128-bit holds 2× f64, 256-bit 4×, 512-bit 8×).
    pub vector_width_f64: u32,

    // -- Cache hierarchy / tiling (subsume polyhedral.rs tile) -------------
    /// L1-resident tile size (elements per tile edge). **Subsumes the
    /// hardcoded `vec![32]` tile.**
    pub tile_l1: u32,
    /// L2-resident tile size (elements). Reserved for a future two-level
    /// tiling scheme; the current polyhedral annotation uses `tile_l1`.
    pub tile_l2: u32,
    /// L1 data-cache size in bytes (used to derive tiling that fits L1).
    pub l1_cache_bytes: u32,
    /// L2 cache size in bytes.
    pub l2_cache_bytes: u32,

    // -- Global knobs ------------------------------------------------------
    /// When `true`, prefer code size over speed (e.g. WASM payload size).
    /// Reserved as a future gate on size-increasing transforms; not yet read
    /// by any pass (so it never changes a decision today).
    pub optimize_for_size: bool,

    /// PGO data (reserved; always `None` until Tier-5 W1).
    pub profile_data: Option<ProfileData>,
}

impl TargetInfo {
    // ----------------------------------------------------------------------
    // Constructors
    // ----------------------------------------------------------------------

    /// The behavioral firewall baseline: native Cranelift, `release-fast`, with
    /// every field set to the exact magic constant S2 deleted. Every pipeline
    /// decision made with this `TargetInfo` is bit-for-bit identical to the
    /// pre-S2 behavior. Used by tests, utility/roundtrip paths, and as the seed
    /// the target-aware constructors refine.
    pub fn native_release_fast() -> TargetInfo {
        TargetInfo {
            target: TargetKind::NativeCranelift,
            profile: BuildProfile::ReleaseFast,

            // Latency model. The branch-mispredict penalty being positive
            // makes `is_profitable_branchless_rewrite` return `true`, exactly
            // reproducing the prior unconditional branchless-count rewrite.
            int_binop_cost: 1,
            branch_mispredict_cost: 15,
            call_overhead: 10,

            // Inlining — the deleted passes.rs literals.
            inline_op_limit: 30,
            inline_hot_op_limit: 80,
            pgo_hot_call_threshold: 1000,

            // Unrolling — the deleted loop_unroll.rs literals.
            unroll_max_trip: 8,
            unroll_max_body: 20,

            // Vectorization — the deleted vectorize.rs `simd_width = 2`.
            vector_width_i64: 2,
            vector_width_f64: 2,

            // Tiling — the deleted polyhedral.rs `vec![32]`.
            tile_l1: 32,
            tile_l2: 256,
            l1_cache_bytes: 32 * 1024,
            l2_cache_bytes: 256 * 1024,

            optimize_for_size: false,
            profile_data: None,
        }
    }

    /// Native Cranelift `release-fast` refined by detected host SIMD caps.
    /// Widens the vectorizer lane count (AVX2 → 4, AVX-512F → 8) while leaving
    /// every other field at the firewall baseline. Behavior-neutral today
    /// because the SIMD width is a dead annotation (no backend lowering reads
    /// it); structurally correct for when real SIMD codegen (Tier-4 L2) lands.
    pub fn native_from_simd_caps(caps: SimdCaps) -> TargetInfo {
        let width = caps.lane_width_64();
        TargetInfo {
            vector_width_i64: width,
            vector_width_f64: width,
            ..TargetInfo::native_release_fast()
        }
    }

    /// WASM `release-fast`. WASM has no native branch-target-buffer; the host
    /// engine's misprediction model is shallower than a native core's, so the
    /// branch-mispredict penalty is lower — but still positive (a mispredicted
    /// branch costs in any engine), so the branchless rewrite stays profitable
    /// (unchanged from the prior unconditional behavior). WASM SIMD is
    /// `simd128` only → the 128-bit baseline lane width (2), matching the prior
    /// hardcoded value. `optimize_for_size` is on (payload size matters on the
    /// edge) but is not yet read by any pass.
    pub fn wasm_release_fast() -> TargetInfo {
        TargetInfo {
            target: TargetKind::Wasm,
            branch_mispredict_cost: 6,
            optimize_for_size: true,
            ..TargetInfo::native_release_fast()
        }
    }

    /// LLVM `release-fast` with the conservative baseline lane width. Prefer
    /// [`TargetInfo::from_llvm_feature_string`] when the host CPU feature
    /// string is available.
    pub fn llvm_release_fast() -> TargetInfo {
        TargetInfo {
            target: TargetKind::Llvm,
            ..TargetInfo::native_release_fast()
        }
    }

    /// Luau `release-fast`. Luau is GC-managed, so shared ownership/drop facts
    /// lower to no-ops in the backend, but the terminal drop phase still runs so
    /// backend-neutral ownership invariants are proven on the same fact plane as
    /// native, LLVM, and WASM.
    pub fn luau_release_fast() -> TargetInfo {
        TargetInfo {
            target: TargetKind::Luau,
            optimize_for_size: true,
            ..TargetInfo::native_release_fast()
        }
    }

    /// LLVM `release-fast`, deriving the vectorizer lane width from an LLVM host
    /// CPU feature string (the `+avx2,+sse4.2,…` form returned by
    /// `TargetMachine::get_host_cpu_features`). Recognizes `+avx`, `+avx2`, and
    /// `+avx512f`. Like the native SIMD-caps constructor this only refines the
    /// (dead-annotation) vector width — every other field is the firewall
    /// baseline.
    pub fn from_llvm_feature_string(features: &str) -> TargetInfo {
        let caps = SimdCaps {
            avx: feature_present(features, "avx"),
            avx2: feature_present(features, "avx2"),
            avx512f: feature_present(features, "avx512f"),
        };
        let width = caps.lane_width_64();
        TargetInfo {
            target: TargetKind::Llvm,
            vector_width_i64: width,
            vector_width_f64: width,
            ..TargetInfo::native_release_fast()
        }
    }

    /// Attach PGO profile data (Tier-5 W1 hook). Consumed by
    /// [`TargetInfo::inline_budget`] and [`TargetInfo::is_pgo_hot`]. No
    /// profiling pipeline calls this yet.
    pub fn with_profile_data(mut self, data: ProfileData) -> TargetInfo {
        self.profile_data = Some(data);
        self
    }

    // ----------------------------------------------------------------------
    // Query API — the surface passes call instead of reading constants.
    // ----------------------------------------------------------------------

    /// The inline op budget for the callee named `name`. PGO-hot callees (per
    /// `profile_data`) get the larger [`Self::inline_hot_op_limit`]; everyone
    /// else gets [`Self::inline_op_limit`]. With no profile data (the only case
    /// today) this is always the base limit — identical to the prior behavior
    /// where the hot budget applied only to `SimpleIR::profile.hot_functions`.
    pub fn inline_budget(&self, name: &str) -> usize {
        if self.is_pgo_hot(name) {
            self.inline_op_limit.max(self.inline_hot_op_limit)
        } else {
            self.inline_op_limit
        }
    }

    /// The base inline op budget (no PGO). Mirrors what the inliner used as its
    /// `MOLT_INLINE_LIMIT`-overridable default.
    pub fn base_inline_op_limit(&self) -> usize {
        self.inline_op_limit
    }

    /// The PGO-hot inline op budget — the larger budget granted to hot callees.
    pub fn hot_inline_op_limit(&self) -> usize {
        self.inline_hot_op_limit
    }

    /// Whether a loop with this constant `trip` count and `body` op count
    /// should be fully unrolled. Subsumes the two `loop_unroll.rs` constant
    /// checks: positive trip ≤ [`Self::unroll_max_trip`] and body ≤
    /// [`Self::unroll_max_body`].
    pub fn should_unroll(&self, trip: i64, body: usize) -> bool {
        trip > 0 && trip <= self.unroll_max_trip && body <= self.unroll_max_body
    }

    /// Max constant trip count eligible for full unrolling.
    pub fn unroll_max_trip(&self) -> i64 {
        self.unroll_max_trip
    }

    /// Max loop-body op count eligible for full unrolling.
    pub fn unroll_max_body(&self) -> usize {
        self.unroll_max_body
    }

    /// SIMD lane count for the given element type. Returns 1 (scalar) for any
    /// non-vectorizable element type. i64/f64/bool map to the 64-bit lane
    /// width; the vectorizer only ever vectorizes i64/f64 lanes (bool collapses
    /// into the i64 lane category).
    pub fn vector_width(&self, elem: &crate::tir::types::TirType) -> u32 {
        use crate::tir::types::TirType;
        match elem {
            TirType::I64 | TirType::Bool => self.vector_width_i64,
            TirType::F64 => self.vector_width_f64,
            _ => 1,
        }
    }

    /// Tile sizes (per loop level, outer→inner) for a loop tiling whose element
    /// is `elem_size` bytes. The current polyhedral annotation tiles a single
    /// level at the L1 tile edge, so this returns one size — subsuming the
    /// hardcoded `vec![32]`. `elem_size` is accepted for the future two-level,
    /// cache-byte-aware scheme; the L1 tile is the conservative current value.
    pub fn tile_sizes(&self, _elem_size: usize) -> Vec<u32> {
        vec![self.tile_l1]
    }

    /// Whether converting an unpredictable conditional branch into branchless
    /// code (the branchless boolean-counting rewrite) pays off on this target.
    /// True iff the branch-misprediction penalty is positive — which it is on
    /// every target we emit for, so this reproduces the prior *unconditional*
    /// branchless-count rewrite while making the decision explicit and tunable.
    pub fn is_profitable_branchless_rewrite(&self) -> bool {
        self.branch_mispredict_cost > 0
    }

    /// Whether `name` is a PGO-hot function per the attached profile data.
    /// Always `false` until PGO lands (no `profile_data` is populated).
    pub fn is_pgo_hot(&self, name: &str) -> bool {
        self.profile_data
            .as_ref()
            .is_some_and(|p| p.hot_functions.contains(name))
    }
}

/// True if an LLVM CPU feature string contains `+<feature>`. Matches the exact
/// `+name` token (comma- or boundary-delimited) so `avx` does not spuriously
/// match `avx2`/`avx512f`.
fn feature_present(features: &str, feature: &str) -> bool {
    features.split(',').any(|tok| {
        let tok = tok.trim();
        tok.strip_prefix('+').map(str::trim) == Some(feature)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::types::TirType;

    /// THE BEHAVIORAL FIREWALL. Every field of the native `release-fast`
    /// baseline must equal the exact magic constant S2 deleted. If any of these
    /// drifts, a pipeline decision silently changes — the #1 risk of this
    /// refactor. This test must stay green forever.
    #[test]
    fn native_release_fast_reproduces_legacy_literals() {
        let t = TargetInfo::native_release_fast();

        // Identity.
        assert_eq!(t.target, TargetKind::NativeCranelift);
        assert_eq!(t.profile, BuildProfile::ReleaseFast);

        // passes.rs inline constants.
        assert_eq!(t.inline_op_limit, 30, "INLINE_OP_LIMIT");
        assert_eq!(t.inline_hot_op_limit, 80, "PGO_HOT_INLINE_OP_LIMIT");
        assert_eq!(t.pgo_hot_call_threshold, 1000, "_PGO_HOT_CALL_THRESHOLD");

        // loop_unroll.rs constants.
        assert_eq!(t.unroll_max_trip, 8, "MAX_UNROLL_TRIP_COUNT");
        assert_eq!(t.unroll_max_body, 20, "MAX_UNROLL_OPS");

        // vectorize.rs simd_width.
        assert_eq!(t.vector_width_i64, 2, "vectorize simd_width (i64)");
        assert_eq!(t.vector_width_f64, 2, "vectorize simd_width (f64)");

        // polyhedral.rs tile.
        assert_eq!(t.tile_l1, 32, "polyhedral tile size");

        // branchless_count: was applied unconditionally → must stay profitable.
        assert!(
            t.is_profitable_branchless_rewrite(),
            "branchless rewrite was unconditional pre-S2"
        );

        // PGO is reserved/empty.
        assert!(t.profile_data.is_none(), "PGO data must be reserved/None");
    }

    /// The query API reproduces the exact legacy decisions on the baseline.
    #[test]
    fn query_api_matches_legacy_decisions() {
        let t = TargetInfo::native_release_fast();

        // Inline budget: no profile data → base limit for every name.
        assert_eq!(t.inline_budget("anything"), 30);
        assert!(!t.is_pgo_hot("anything"));

        // Unroll: legacy was `trip > 0 && trip <= 8 && body <= 20`.
        assert!(t.should_unroll(8, 20));
        assert!(t.should_unroll(1, 0));
        assert!(!t.should_unroll(9, 1)); // trip too high
        assert!(!t.should_unroll(0, 1)); // non-positive trip
        assert!(!t.should_unroll(-3, 1)); // negative trip
        assert!(!t.should_unroll(4, 21)); // body too big
        assert_eq!(t.unroll_max_trip(), 8);
        assert_eq!(t.unroll_max_body(), 20);

        // Vector width: 2 for i64/f64/bool, 1 (scalar) otherwise.
        assert_eq!(t.vector_width(&TirType::I64), 2);
        assert_eq!(t.vector_width(&TirType::F64), 2);
        assert_eq!(t.vector_width(&TirType::Bool), 2);
        assert_eq!(t.vector_width(&TirType::Str), 1);

        // Tile sizes: single L1-edge tile of 32.
        assert_eq!(t.tile_sizes(8), vec![32]);
    }

    /// PGO hook: when (someday) populated, hot callees get the larger budget;
    /// the mechanism is correct even though no pass populates it yet.
    #[test]
    fn pgo_hook_grants_hot_budget() {
        let mut hot = std::collections::BTreeSet::new();
        hot.insert("hot_fn".to_string());
        let t =
            TargetInfo::native_release_fast().with_profile_data(ProfileData { hot_functions: hot });

        assert!(t.is_pgo_hot("hot_fn"));
        assert!(!t.is_pgo_hot("cold_fn"));
        assert_eq!(t.inline_budget("hot_fn"), 80); // max(30, 80)
        assert_eq!(t.inline_budget("cold_fn"), 30);
    }

    /// SIMD-cap widening maps the conventional lane widths and is consistent
    /// across the native and LLVM-feature-string constructors.
    #[test]
    fn simd_caps_widen_vector_width() {
        let base = TargetInfo::native_from_simd_caps(SimdCaps::BASELINE_128);
        assert_eq!(base.vector_width_i64, 2);
        assert_eq!(base.vector_width_f64, 2);

        let avx2 = TargetInfo::native_from_simd_caps(SimdCaps {
            avx: true,
            avx2: true,
            avx512f: false,
        });
        assert_eq!(avx2.vector_width_i64, 4);

        let avx512 = TargetInfo::native_from_simd_caps(SimdCaps {
            avx: true,
            avx2: true,
            avx512f: true,
        });
        assert_eq!(avx512.vector_width_i64, 8);

        // Every other field is still the firewall baseline.
        assert_eq!(avx512.inline_op_limit, 30);
        assert_eq!(avx512.unroll_max_trip, 8);
        assert_eq!(avx512.tile_l1, 32);
    }

    /// LLVM feature-string parsing matches exact `+name` tokens (no `avx`
    /// matching `avx2`).
    #[test]
    fn llvm_feature_string_parses_exact_tokens() {
        assert!(feature_present("+avx2,+sse4.2,-avx512f", "avx2"));
        assert!(!feature_present("+avx2,+sse4.2", "avx512f"));
        // `+avx2` must NOT satisfy a query for `avx` (exact token match).
        assert!(!feature_present("+avx2", "avx"));
        assert!(feature_present("+avx,+avx2", "avx"));

        let t = TargetInfo::from_llvm_feature_string("+avx2,+fma,+sse4.2");
        assert_eq!(t.target, TargetKind::Llvm);
        assert_eq!(t.vector_width_i64, 4);

        let baseline = TargetInfo::from_llvm_feature_string("+sse2");
        assert_eq!(baseline.vector_width_i64, 2);
    }

    /// WASM differs only where it structurally should: lower (but positive)
    /// branch-mispredict penalty and `optimize_for_size`; all profitability
    /// thresholds match the baseline, and the branchless rewrite stays on.
    #[test]
    fn wasm_release_fast_keeps_thresholds() {
        let w = TargetInfo::wasm_release_fast();
        assert_eq!(w.target, TargetKind::Wasm);
        assert_eq!(w.inline_op_limit, 30);
        assert_eq!(w.unroll_max_trip, 8);
        assert_eq!(w.unroll_max_body, 20);
        assert_eq!(w.vector_width_i64, 2);
        assert_eq!(w.tile_l1, 32);
        assert!(w.optimize_for_size);
        assert!(w.is_profitable_branchless_rewrite());
        assert!(w.branch_mispredict_cost > 0);
    }
}
