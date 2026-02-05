# Python Dependency Support Strategy
**Spec ID:** 0013
**Status:** Draft (implementation-targeting)
**Audience:** compiler engineers, runtime engineers, packaging owners
**Goal:** Maximize Python dependency compatibility while preserving determinism and Molt safety guarantees.

---

## 1. Compatibility Tiers
### 1.1 Tier A: Pure Python (Preferred)
- Pure Python packages that stay within the Molt subset compile directly.
- Vendored into the build graph using `uv.lock` as the source of truth.
- Deterministic builds require exact hashes in lockfiles.

### 1.2 Tier B: Molt Packages (Native/WASM)
- High-impact libraries gain native implementations (Rust or WASM).
- Examples: `molt_json`, `molt_msgpack`, `molt_cbor`, data/IO connectors.
- Capability gating enforced at package boundaries.
- C-extension compatibility is achieved by recompiling against `libmolt`
  (see `docs/spec/areas/compat/0214_LIBMOLT_C_API_V0.md`).

### 1.3 Tier C: CPython Bridge (Explicit Escape Hatch)
- Primary C-extension strategy is to recompile against `libmolt` (Tier B).
- The CPython bridge is opt-in and capability-gated, never the default.
- When enabled, run in a CPython worker process with Arrow IPC and structured codecs.
- Deterministic mode restricts nondeterministic APIs unless explicitly allowed.
 - See `docs/spec/areas/compat/0210_CPYTHON_BRIDGE_PYO3.md` for bridge modes,
   capability rules, and performance guardrails.

---

## 2. Resolver and Build Flow
1) Resolve dependencies with `uv`.
2) Validate lockfiles and hashes.
3) Classify dependencies into Tier A/B/C.
4) Compile Tier A in-process.
5) Bind Tier B as Molt Packages.
6) Route Tier C through the CPython bridge only when explicitly enabled; otherwise
   fail fast with a compatibility error.

---

## 3. Compatibility Matrix
Each dependency should declare:
- `tier`: A/B/C
- `features`: supported modules and API constraints
- `capabilities`: network/fs/crypto needs
- `determinism`: allowed/denied in strict builds
- explicit allowlists in `pyproject.toml` (`[tool.molt.deps]`) for tier overrides

---

## 4. Tooling Requirements
- `molt deps`: print tier classification and blockers (initial implementation available;
  supports `--json` and `--verbose` summaries).
- `molt vendor`: materialize Tier A sources into `vendor/` (default) with a
  `manifest.json` and hashed artifacts; supports `--dry-run`, `--output`,
  `--allow-non-tier-a`, `--extras`, `--json`, and `--verbose` (markers/extras
  are evaluated against the host environment).
- `molt extension build` (planned): build helpers and headers for `libmolt`
  extensions, including capability metadata and ABI tagging.
- Git sources are supported when a pinned revision (or tag/branch resolved to a
  commit) is present; the vendor manifest records the resolved commit and tree hash.
- Build/run tooling treats `vendor/packages` and `vendor/local` as module roots
  (and adds them to `PYTHONPATH`) when present; override with `MOLT_MODULE_ROOTS`.
- `molt verify`: confirm hashes and capability declarations (manifest + checksum
  validation plus capability/effect allowlist enforcement; packages declaring
  capabilities/effects require an allowlist via `--capabilities` or config).

---

## 5. CPython Bridge Constraints
- CPython bridge must be isolated and capability-scoped.
- All data must cross via Arrow IPC or MsgPack/CBOR.
- Bridge is opt-in and explicitly disabled in strict deterministic builds.

---

## 6. Acceptance Criteria
- Common pure-Python libraries compile and run without modification.
- C-extension packages have a documented fallback path via the bridge.
- Deterministic builds fail fast on unsupported or unpinned dependencies.
- High-value C-extensions can be recompiled against `libmolt` with stable ABI tags.
