// molt_build_determinism.qnt — Quint model for Molt build determinism.
//
// Models the real Molt compilation pipeline:
//   Module discovery → Topological sort (Kahn's, stable) → Layer-based parallel
//   compilation → Content-addressed caching (SHA256) → Linking → Artifact
//
// Checks that the final artifact digest is schedule-independent.
//
// Usage:
//   quint run  formal/quint/molt_build_determinism.qnt --invariant=Inv --max-steps=12
//   quint verify formal/quint/molt_build_determinism.qnt --invariant=Inv

module molt_build_determinism {

  // =========================================================================
  // Types
  // =========================================================================
  type ModuleId = int
  type Digest = int
  type Artifact = { mod: ModuleId, digest: Digest }

  // =========================================================================
  // Constants (bounded for tractable model checking, N ≤ 6)
  // =========================================================================
  pure val N = 4
  pure val Modules: Set[int] = 0.to(N - 1)

  // Dependency graph: module i depends on modules in deps(i).
  // Models _topo_sort_modules in cli.py (Kahn's algorithm with sorted() for stability).
  // Module 0: no deps, Module 1: depends on 0, Module 2: depends on 0, Module 3: depends on 1,2
  pure def deps(m: ModuleId): Set[ModuleId] =
    if (m == 0) Set()
    else if (m == 1) Set(0)
    else if (m == 2) Set(0)
    else if (m == 3) Set(1, 2)
    else Set()

  // =========================================================================
  // Pure functions (deterministic compilation model)
  // =========================================================================

  // Content-addressed compilation: same module → same digest.
  // Models _cache_key() in cli.py: SHA256(IR_payload | target | fingerprints | schema).
  pure def compileDigest(m: ModuleId): Digest = (m * 1103515245 + 12345) % 2147483647

  pure def compile(m: ModuleId): Artifact = { mod: m, digest: compileDigest(m) }

  // Canonical link digest: order-independent combination of module digests.
  // Models the real linker which processes modules in a deterministic order.
  pure def linkDigest(ms: Set[ModuleId]): Digest =
    0.to(N - 1).fold(0, (acc, i) =>
      if (ms.contains(i)) acc + compileDigest(i) else acc
    )

  // Check if all dependencies of module m are satisfied.
  pure def depsReady(m: ModuleId, built: Set[ModuleId]): bool =
    deps(m).forall(d => built.contains(d))

  // The set of modules ready to build (deps satisfied, not yet built).
  pure def readySet(remaining: Set[ModuleId], built: Set[ModuleId]): Set[ModuleId] =
    remaining.filter(m => depsReady(m, built))

  // =========================================================================
  // State
  // =========================================================================
  var todo: Set[ModuleId]
  var done: Set[ModuleId]
  var cache: ModuleId -> Artifact
  var artifacts: Set[Artifact]

  // Determinism-pinned nondet sources
  var hashSeed: int    // models PYTHONHASHSEED=0 enforcement

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    todo' = Modules,
    done' = Set(),
    cache' = Modules.mapBy(m => { mod: m, digest: -1 }),
    artifacts' = Set(),
    hashSeed' = 0,  // PYTHONHASHSEED always pinned to 0 in Molt
  }

  // =========================================================================
  // Steps
  // =========================================================================

  // Build one module whose dependencies are satisfied.
  // Models nondeterministic scheduling within a compilation layer.
  action buildOne = {
    val ready = readySet(todo, done)

    // Only proceed if there are modules whose deps are met
    nondet m = ready.oneOf()

    val cached = cache.get(m).digest != -1
    val art = if (cached) cache.get(m) else compile(m)

    all {
      cache' = if (cached) cache else cache.set(m, art),
      done' = done.union(Set(m)),
      todo' = todo.exclude(Set(m)),
      artifacts' = artifacts.union(Set(art)),
      hashSeed' = hashSeed,  // always pinned
    }
  }

  // Stutter when nothing is ready (waiting for deps or all done).
  action stutter = all {
    todo' = todo,
    done' = done,
    cache' = cache,
    artifacts' = artifacts,
    hashSeed' = hashSeed,
  }

  action step = {
    val ready = readySet(todo, done)
    if (ready != Set()) buildOne else stutter
  }

  // =========================================================================
  // Derived state
  // =========================================================================

  // Compute final digest from collected artifacts (order-independent).
  val computedDigest: Digest =
    0.to(N - 1).fold(0, (acc, i) =>
      if (done.contains(i))
        artifacts.fold(0, (d, a) => if (a.mod == i) a.digest else d) + acc
      else
        acc
    )

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: Dependencies respected — no module built before its deps.
  val depsRespected =
    done.forall(m => deps(m).forall(d => done.contains(d)))

  // I2: Cache correctness — cached entries equal deterministic compile output.
  val cacheCorrect =
    Modules.forall(m =>
      cache.get(m).digest == -1 or cache.get(m) == compile(m)
    )

  // I3: Hash seed pinning — PYTHONHASHSEED never drifts from 0.
  val hashSeedPinned = hashSeed == 0

  // I4: Final determinism — once all modules built, digest matches canonical.
  val finalDeterministic =
    (done == Modules) implies (computedDigest == linkDigest(Modules))

  // I5: Artifact correctness — every artifact correctly compiled.
  val artifactsCorrect =
    artifacts.forall(a => a == compile(a.mod))

  // I6: No duplicate artifacts — each module produces exactly one artifact.
  val noDuplicateArtifacts =
    artifacts.forall(a1 =>
      artifacts.forall(a2 =>
        (a1.mod == a2.mod) implies (a1.digest == a2.digest)
      )
    )

  // Combined invariant for verification.
  val Inv = and {
    depsRespected,
    cacheCorrect,
    hashSeedPinned,
    finalDeterministic,
    artifactsCorrect,
    noDuplicateArtifacts,
  }
}

// =========================================================================
// KNOWN-BAD MODEL: demonstrates what breaks when ordering is unstable
// =========================================================================
// This module intentionally models a BUG: using a polynomial hash chain
// (acc * 31 + digest) to accumulate module digests. This is order-dependent:
// hash(A, B, C) ≠ hash(C, A, B). The invariant SHOULD FAIL, proving the
// model checker catches schedule-dependent nondeterminism.

module molt_build_order_dependent {

  type ModuleId = int
  type Digest = int

  pure val N = 3
  pure val Modules: Set[int] = 0.to(N - 1)

  pure def compileDigest(m: ModuleId): Digest = m + 1

  // Canonical digest: polynomial hash in fixed order 0, 1, 2.
  // 0 * 31 + 1 = 1; 1 * 31 + 2 = 33; 33 * 31 + 3 = 1026
  pure def canonicalDigest: Digest =
    0.to(N - 1).fold(0, (acc, i) => acc * 31 + compileDigest(i))

  var todo: Set[ModuleId]
  var done: Set[ModuleId]
  // BUG: polynomial hash accumulator — order-dependent!
  var runningDigest: Digest

  action init = all {
    todo' = Modules,
    done' = Set(),
    runningDigest' = 0,
  }

  action buildOne = {
    nondet m = todo.oneOf()
    all {
      todo' = todo.exclude(Set(m)),
      done' = done.union(Set(m)),
      // BUG: polynomial hash chain is NOT commutative.
      // Order (0,1,2): 0→1→33→1026
      // Order (2,0,1): 0→3→94→2916
      runningDigest' = runningDigest * 31 + compileDigest(m),
    }
  }

  action step = {
    if (todo != Set()) buildOne
    else all {
      todo' = todo,
      done' = done,
      runningDigest' = runningDigest,
    }
  }

  // This SHOULD FAIL: runningDigest is order-dependent.
  val OrderDependentInv =
    (done == Modules) implies (runningDigest == canonicalDigest)
}
