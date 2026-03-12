// molt_cache_coherence.qnt — Quint model for Molt's build cache coherence.
//
// Models the interaction between Molt's content-addressed build cache,
// compilation determinism, and incremental compilation:
//   - Content-addressed cache keyed by SHA256(source + deps + config)
//   - Dependency-driven invalidation (source or transitive dep change)
//   - Concurrent builds sharing a cache directory
//   - Incremental rebuild correctness (same result as full rebuild)
//
// Prevents regressions where stale cache entries survive dependency
// updates, concurrent builds corrupt cache state, or incremental
// rebuilds diverge from clean builds.
//
// Usage:
//   quint run  formal/quint/molt_cache_coherence.qnt --invariant=Inv --max-steps=15
//   quint verify formal/quint/molt_cache_coherence.qnt --invariant=Inv

module molt_cache_coherence {

  // =========================================================================
  // Types
  // =========================================================================
  type ModuleId = int
  type Hash = int
  type Version = int

  type CacheEntry = {
    mod: ModuleId,
    inputHash: Hash,       // content-addressed key: hash(source + dep hashes + config)
    artifact: Hash,        // compiled output artifact digest
    depHashes: int -> Hash // snapshot of dependency hashes at build time
  }

  type BuildRequest = {
    mod: ModuleId,
    builder: int           // builder id (for concurrent builds)
  }

  type BuildResult = {
    mod: ModuleId,
    artifact: Hash,
    fromCache: bool
  }

  // =========================================================================
  // Constants (bounded for tractable model checking, N <= 6)
  // =========================================================================
  pure val N = 4                          // number of modules
  pure val MAX_BUILDERS = 2               // concurrent builder processes
  pure val MAX_VERSIONS = 3               // source version generations
  pure val Modules: Set[int] = 0.to(N - 1)
  pure val Builders: Set[int] = 0.to(MAX_BUILDERS - 1)

  // Dependency graph: 0 <- 1, 0 <- 2, 1,2 <- 3
  // Module 0: leaf, Module 1: depends on 0, Module 2: depends on 0,
  // Module 3: depends on 1 and 2
  pure def deps(m: ModuleId): Set[ModuleId] =
    if (m == 0) Set()
    else if (m == 1) Set(0)
    else if (m == 2) Set(0)
    else if (m == 3) Set(1, 2)
    else Set()

  // Transitive dependencies (for this specific graph).
  pure def transitiveDeps(m: ModuleId): Set[ModuleId] =
    if (m == 0) Set()
    else if (m == 1) Set(0)
    else if (m == 2) Set(0)
    else if (m == 3) Set(0, 1, 2)
    else Set()

  // =========================================================================
  // Pure functions
  // =========================================================================

  // Content hash: deterministic function of source version and dep hashes.
  // Models _cache_key() in cli.py: SHA256(IR | target | dep_fingerprints).
  pure def contentHash(m: ModuleId, srcVer: Version, depHashes: int -> Hash): Hash = {
    val depContrib = deps(m).fold(0, (acc, d) => acc + depHashes.get(d))
    (m * 1000003 + srcVer * 31 + depContrib) % 2147483647
  }

  // Deterministic compilation: same inputs always produce same artifact.
  // This models Molt's requirement that compilation is a pure function.
  pure def compileArtifact(m: ModuleId, srcVer: Version, depHashes: int -> Hash): Hash = {
    val h = contentHash(m, srcVer, depHashes)
    (h * 2654435761) % 2147483647  // bijective mixing for artifact bits
  }

  // Compute the current input hash for a module given current source versions
  // and the current artifact hashes of dependencies.
  pure def currentInputHash(m: ModuleId, srcVersions: int -> Version,
                            artifactHashes: int -> Hash): Hash = {
    val depH = Modules.mapBy(d => artifactHashes.get(d))
    contentHash(m, srcVersions.get(m), depH)
  }

  // Compute the artifact that a fresh build would produce.
  pure def freshArtifact(m: ModuleId, srcVersions: int -> Version,
                         artifactHashes: int -> Hash): Hash = {
    val depH = Modules.mapBy(d => artifactHashes.get(d))
    compileArtifact(m, srcVersions.get(m), depH)
  }

  // =========================================================================
  // State
  // =========================================================================
  var srcVersions: int -> Version          // current source version per module
  var cache: int -> CacheEntry             // cache slot per module (-1 hash = empty)
  var artifactHashes: int -> Hash          // latest built artifact hash per module (-1 = not built)
  var built: Set[ModuleId]                 // modules that have been built in this generation
  var pendingBuilds: Set[BuildRequest]     // in-flight concurrent build requests
  var buildResults: Set[BuildResult]       // completed build results
  var generation: int                      // monotonic generation counter for source changes

  // =========================================================================
  // Init
  // =========================================================================
  action init = all {
    srcVersions' = Modules.mapBy(m => 0),
    cache' = Modules.mapBy(m => {
      mod: m, inputHash: -1, artifact: -1,
      depHashes: Modules.mapBy(d => -1),
    }),
    artifactHashes' = Modules.mapBy(m => -1),
    built' = Set(),
    pendingBuilds' = Set(),
    buildResults' = Set(),
    generation' = 0,
  }

  // =========================================================================
  // Actions
  // =========================================================================

  // cacheHit: A builder checks the cache for a module and finds a valid entry.
  // The cached artifact is used directly without recompilation.
  action cacheHit = {
    nondet m = Modules.exclude(built).oneOf()
    all {
      // Dependencies must be built first.
      deps(m).forall(d => d.in(built)),
      // Cache entry must exist and match current inputs.
      val entry = cache.get(m)
      val curHash = currentInputHash(m, srcVersions, artifactHashes)
      all {
        entry.inputHash == curHash,
        entry.inputHash != -1,
        // Use cached artifact.
        artifactHashes' = artifactHashes.set(m, entry.artifact),
        built' = built.union(Set(m)),
        buildResults' = buildResults.union(Set({
          mod: m, artifact: entry.artifact, fromCache: true,
        })),
        // unchanged
        srcVersions' = srcVersions,
        cache' = cache,
        pendingBuilds' = pendingBuilds,
        generation' = generation,
      }
    }
  }

  // cacheMiss: Cache miss — module must be compiled fresh.
  // The result is stored in both the cache and the artifact map.
  action cacheMiss = {
    nondet m = Modules.exclude(built).oneOf()
    all {
      // Dependencies must be built first.
      deps(m).forall(d => d.in(built)),
      // Cache entry does not match (miss or empty).
      val curHash = currentInputHash(m, srcVersions, artifactHashes)
      val entry = cache.get(m)
      all {
        entry.inputHash != curHash,
        // Fresh compilation.
        val art = freshArtifact(m, srcVersions, artifactHashes)
        val depH = Modules.mapBy(d => artifactHashes.get(d))
        val newEntry: CacheEntry = {
          mod: m, inputHash: curHash, artifact: art, depHashes: depH,
        }
        all {
          cache' = cache.set(m, newEntry),
          artifactHashes' = artifactHashes.set(m, art),
          built' = built.union(Set(m)),
          buildResults' = buildResults.union(Set({
            mod: m, artifact: art, fromCache: false,
          })),
          // unchanged
          srcVersions' = srcVersions,
          pendingBuilds' = pendingBuilds,
          generation' = generation,
        }
      }
    }
  }

  // invalidate: A source file changes, bumping its version.
  // All downstream modules become un-built (need rebuild).
  // Models the file-watcher or explicit `--force` rebuild trigger.
  action invalidate = {
    nondet m = Modules.oneOf()
    all {
      srcVersions.get(m) < MAX_VERSIONS - 1,
      val newVer = srcVersions.get(m) + 1
      // Invalidate m and all transitive dependents.
      val affected = Modules.filter(x => x == m or m.in(transitiveDeps(x)))
      all {
        srcVersions' = srcVersions.set(m, newVer),
        // Remove affected modules from built set (they need rebuild).
        built' = built.exclude(affected),
        // Reset artifact hashes for affected modules.
        artifactHashes' = affected.fold(artifactHashes, (ah, x) =>
          ah.set(x, -1)
        ),
        generation' = generation + 1,
        // unchanged
        cache' = cache,
        pendingBuilds' = pendingBuilds,
        buildResults' = buildResults,
      }
    }
  }

  // concurrentBuild: Two builders race to build the same module.
  // Both compute the same content-addressed result (determinism guarantee).
  // The cache write is idempotent because same input hash = same artifact.
  action concurrentBuild = {
    nondet m = Modules.exclude(built).oneOf()
    nondet b1 = Builders.oneOf()
    nondet b2 = Builders.exclude(Set(b1)).oneOf()
    all {
      // Dependencies must be built.
      deps(m).forall(d => d.in(built)),
      // Both builders compute the same artifact (determinism).
      val art = freshArtifact(m, srcVersions, artifactHashes)
      val curHash = currentInputHash(m, srcVersions, artifactHashes)
      val depH = Modules.mapBy(d => artifactHashes.get(d))
      val newEntry: CacheEntry = {
        mod: m, inputHash: curHash, artifact: art, depHashes: depH,
      }
      all {
        // Both builders write to cache — idempotent because content-addressed.
        cache' = cache.set(m, newEntry),
        artifactHashes' = artifactHashes.set(m, art),
        built' = built.union(Set(m)),
        buildResults' = buildResults.union(Set(
          { mod: m, artifact: art, fromCache: false },
        )),
        // unchanged
        srcVersions' = srcVersions,
        pendingBuilds' = pendingBuilds,
        generation' = generation,
      }
    }
  }

  // rebuild: Force a full rebuild of a module (ignore cache).
  // Used by `--force` flag. Result must match what cache would produce.
  action rebuild = {
    nondet m = built.oneOf()
    all {
      deps(m).forall(d => d.in(built)),
      val art = freshArtifact(m, srcVersions, artifactHashes)
      val curHash = currentInputHash(m, srcVersions, artifactHashes)
      val depH = Modules.mapBy(d => artifactHashes.get(d))
      val newEntry: CacheEntry = {
        mod: m, inputHash: curHash, artifact: art, depHashes: depH,
      }
      all {
        cache' = cache.set(m, newEntry),
        artifactHashes' = artifactHashes.set(m, art),
        buildResults' = buildResults.union(Set({
          mod: m, artifact: art, fromCache: false,
        })),
        // unchanged
        srcVersions' = srcVersions,
        built' = built,
        pendingBuilds' = pendingBuilds,
        generation' = generation,
      }
    }
  }

  action stutter = all {
    srcVersions' = srcVersions,
    cache' = cache,
    artifactHashes' = artifactHashes,
    built' = built,
    pendingBuilds' = pendingBuilds,
    buildResults' = buildResults,
    generation' = generation,
  }

  action step = any {
    cacheHit,
    cacheMiss,
    invalidate,
    concurrentBuild,
    rebuild,
    stutter,
  }

  // =========================================================================
  // Invariants
  // =========================================================================

  // I1: Cache hits produce identical artifacts to fresh builds (determinism).
  // Any cached entry, if its inputHash matches what we would compute now,
  // must have the same artifact as a fresh compile.
  val cacheHitDeterminism =
    Modules.forall(m =>
      val entry = cache.get(m)
      val curHash = currentInputHash(m, srcVersions, artifactHashes)
      (entry.inputHash != -1 and entry.inputHash == curHash and m.in(built))
        implies entry.artifact == freshArtifact(m, srcVersions, artifactHashes)
    )

  // I2: Dependency changes invalidate downstream cache entries.
  // If a module is built and its dependency's artifact hash changed since
  // the cache entry was written, the cache entry's inputHash must differ
  // from the current inputHash (i.e., the old entry is stale).
  val depInvalidation =
    Modules.forall(m =>
      val entry = cache.get(m)
      (entry.inputHash != -1 and m.in(built)) implies
        deps(m).forall(d =>
          (d.in(built) and artifactHashes.get(d) != -1)
            implies entry.depHashes.get(d) == artifactHashes.get(d)
                    or entry.inputHash != currentInputHash(m, srcVersions, artifactHashes)
        )
    )

  // I3: Concurrent builds don't corrupt cache state.
  // All cache entries are valid: if present, artifact matches the deterministic
  // compile for the recorded inputHash. Since compilation is pure and cache
  // is content-addressed, concurrent writes are idempotent.
  val cacheNotCorrupted =
    Modules.forall(m =>
      val entry = cache.get(m)
      entry.inputHash != -1 implies (
        entry.artifact == compileArtifact(m,
          // We verify structural consistency: artifact matches inputHash derivation.
          // The cache entry stores the depHashes snapshot used at build time.
          srcVersions.get(m), entry.depHashes)
        or entry.inputHash != currentInputHash(m, srcVersions, artifactHashes)
      )
    )

  // I4: Cache is content-addressed (same input = same hash = same output).
  // Within a single build generation, two build results for the same module
  // must have the same artifact. Since buildResults accumulates across
  // invalidation boundaries, we check the weaker property: all currently
  // built modules have consistent artifacts with their cache entries.
  val contentAddressed =
    Modules.forall(m =>
      (m.in(built) and artifactHashes.get(m) != -1 and cache.get(m).inputHash != -1) implies
        cache.get(m).artifact == artifactHashes.get(m)
    )

  // I5: No stale cache entries survive dependency updates.
  // If a module is marked as built, its cache entry's inputHash matches
  // the current computation.
  val noStaleCacheEntries =
    Modules.forall(m =>
      (m.in(built) and cache.get(m).inputHash != -1) implies
        cache.get(m).inputHash == currentInputHash(m, srcVersions, artifactHashes)
    )

  // I6: Incremental rebuild produces same result as full rebuild.
  // For any built module, its artifact hash equals what a fresh compile
  // would produce given current source versions and dep artifacts.
  val incrementalEqualsFullRebuild =
    Modules.forall(m =>
      (m.in(built) and artifactHashes.get(m) != -1) implies
        artifactHashes.get(m) == freshArtifact(m, srcVersions, artifactHashes)
    )

  // Combined invariant.
  val Inv = and {
    cacheHitDeterminism,
    depInvalidation,
    cacheNotCorrupted,
    contentAddressed,
    noStaleCacheEntries,
    incrementalEqualsFullRebuild,
  }
}
