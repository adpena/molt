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

### 1.2 Tier B: Native/WASM Packages
- Compile upstream package source and reuse its native kernels through supported compiler/runtime primitives and ABIs.
- Package compatibility does not require Molt-specific substitutes for upstream libraries.
- Capability gating is enforced at package boundaries.
- C-extension compatibility is achieved by recompiling against `libmolt`
  (see `docs/spec/areas/compat/surfaces/c_api/libmolt_c_api_surface.md`).

### 1.3 Tier C: Unresolved Compatibility
- Existing Tier C classifications identify dependencies requiring further source, ABI or packaging work; they do not provide an execution fallback.
- Rebuild compatible extensions against `libmolt`; report unsupported inputs as blockers.
- The [Python execution boundary](../compat/contracts/cpython_bridge_policy.md) excludes embedded and worker-process CPython from compiled execution.

---

## 2. Resolver and Build Flow
1) Resolve dependencies with `uv`.
2) Validate lockfiles and hashes.
3) Classify dependencies into Tier A/B/C.
4) Compile Tier A in-process.
5) Bind Tier B as Molt Packages.
6) Resolve Tier C blockers through source compilation or supported extension
   rebuilding before claiming support; a tier override supplies neither an ABI
   implementation nor a host interpreter.

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

## 5. Execution boundary

Pinned CPython may run as a development-side reference or build tool. Shipped
programs and their extensions execute within Molt without a host Python runtime.
Explicit application IPC retains its own contract and cannot silently replace an
unsupported Python dependency.

---

## 6. Acceptance Criteria
- Common pure-Python libraries compile and run without modification.
- Supported C-extension packages have a qualified source-rebuild and libmolt execution path.
- Deterministic builds fail fast on unsupported or unpinned dependencies.
- High-value C-extensions can be recompiled against `libmolt` with stable ABI tags.
