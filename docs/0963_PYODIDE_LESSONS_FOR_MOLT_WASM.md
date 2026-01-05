# Lessons from Pyodide for Molt WASM
**Document ID:** 0963
**Status:** Canonical Guidance
**Audience:** Molt runtime/compiler engineers, AI coding agents, WASM implementers
**Purpose:** Extract the highest-leverage architectural, UX, and tooling lessons from Pyodide and translate them into concrete, actionable guidance for Molt’s WASM and browser strategy.

---

## 0. Executive summary (for humans and AI)
Pyodide proves that:
- developers want Python in the browser
- they accept constraints if the UX is good
- packaging, loading, and interop matter more than raw speed

Molt should not copy Pyodide’s implementation.
Molt should copy its operational wisdom and surpass it using:
- schema-first boundaries
- compiled execution
- smaller, purpose-built runtimes

---

## 1. What Pyodide is (and is not)
Pyodide is:
- CPython compiled to WebAssembly (via Emscripten)
- interpreter-first
- dynamically loading Python packages
- excellent at Python to JavaScript interop

Pyodide is not:
- a compiled Python
- a small runtime
- a strict execution environment

---

## 2. Packaging & dependency loading (steal the UX)

### 2.1 Dynamic package loading
Pyodide separates:
- core runtime
- bundled packages
- dynamically installed packages

Action for Molt:
- minimal WASM core
- compiled Molt modules as loadable units
- explicit loading with hashes and signatures

AI agent rule:
Never assume everything is preloaded in WASM.

---

### 2.2 Recipe-based ecosystem
Pyodide uses build recipes to port packages.

Action for Molt:
- create molt-recipes
- recipes define schema, build steps, targets, versions
- CI builds signed artifacts

---

## 3. Interop & ABI (critical)
JS to Python interop is expensive and subtle.

Molt advantage:
- schema-first IPC
- no arbitrary object passing
- MsgPack or Arrow IPC
- versioned ABI

Rule:
WASM boundary equals schema boundary.

AI agent rule:
Never design object-level WASM interop APIs.

---

## 4. Performance realities
Pyodide inherits interpreter overhead and large binaries.

Action for Molt:
- scope WASM targets
- optimize for determinism and safety
- not raw numeric speed initially

---

## 5. UX beats purity
Pyodide succeeded because UX was excellent.

Action for Molt:
- document constraints loudly
- clear error messages
- visible loading progress
- fail fast on violations

---

## 6. Where Molt surpasses Pyodide

| Area | Pyodide | Molt |
|----|--------|------|
| Execution | Interpreter | Compiled |
| Boundaries | Dynamic | Schema-first |
| ABI | Ad hoc | Versioned |
| Size | Large | Minimal |
| Server to Browser | Manual | Shared contracts |

---

## 7. Molt WASM checklist
- minimal runtime
- explicit loading
- signed artifacts
- schema-only IPC
- no object proxies

---

## 8. What not to copy
- full stdlib shipping
- implicit loading
- interpreter-first model

---

## 9. AI agent mandatory rules
1. Prefer schemas over objects
2. Prefer explicit loading
3. Prefer determinism
4. Prefer clear errors
5. Avoid CPython emulation

---

## 10. North star
Pyodide proved Python can run in the browser.
Molt makes it production-grade.
