# Stdlib Intrinsics Loading

Status: Active

## Scope
This spec defines how stdlib modules load Molt intrinsics and the minimum
requirements for correctness, performance, and determinism. It is the canonical
checklist for new or modified stdlib shims.

## Loader Contract

- Resolve intrinsics through `src/molt/stdlib/_intrinsics.py` only.
- Compiled `_intrinsics` exports use the runtime resolver. Python wrappers
  require an active runtime helper/registry; an arbitrary namespace callable
  alone is not authority. `tests/test_intrinsics_bootstrap_contract.py` covers
  wrapper resolution and activation.
- Do not create alternative registries, hidden loaders, or import-time side
  effects that bypass the canonical loader.

## Dynamic callable closure

`molt-tir::passes::collect_app_callable_requirements` owns possible callable
reachability for native app resolvers and WASM imports, trampolines and app
resolvers. Literal intrinsic symbols remain reachable when passed through
wrappers or stored in objects, not just at direct `require_intrinsic` calls.
These are conservative resolver candidates, not unconditional capability
requirements: each target admits them against its linked symbol/ABI/profile
authority. Explicit `_molt_` aliases retain their canonical provider, not a second
resolver entry. A string equal to a runtime symbol does not force an unavailable
provider into WASM Pure. Dynamic optional lookup remains unavailable when its
provider is excluded. Actual calls, builtin materialization, global builtin
lookup and operation requirements still fail early for unsupported profiles.

Mutable global lookup remains a runtime lookup. Its possible builtin target
comes from `BUILTIN_FUNC_SPECS` in `src/molt/frontend/_types.py`, projected by
`tools/gen_wasm_abi.py` into the shared `molt-ir` builtin table and runtime
materialization metadata. This includes `globals`, `locals`, `vars`, and
`__import__`, with their Python defaults and verified callable ABI. A proven
literal name retains only its matching target; an unknown/redefined name retains
the supported builtin family, not all runtime exports. SimpleIR definitions,
including parameter and store targets, govern whether a name is constant.
Possible targets do not become exact `runtime_symbol` provenance or direct calls.
Both native and WASM builtin materialization use the installed app resolver;
there is no native-only address-taking table retaining every builtin.
Python builtin signatures and intrinsic signatures remain distinct authorities:
an ABI-backed builtin need not be an intrinsic. Intrinsic lookup accepts canonical
names and `_molt_` aliases, not inferred Python spellings. Runtime unit tests
without a compiled app use generated, test-only callable address fixtures; an
installed app resolver's miss is authoritative even in tests.

Canonical `builtins` publication atomically seeds the runtime-backed Python
namespace before either generated module metadata or the Python body can import
another module. Class names and their public/internal distinction come from the
runtime class authority; exception names and version/platform gates come from
the object-model exception schema; callable names come from the generated
builtin table and are admitted by the app resolver. Class constructors remain
classes, not their lower-level callable adapters. Canonical publication retains
the supported callable family explicitly in the shared reachability collector.
The Python facade supplies wrappers and projects its public list from this
namespace; it does not duplicate primitive binding or platform/version gates.
Its remaining intrinsic operations call the canonical `require_intrinsic`
directly with the executing namespace. There is no builtin-local forwarding
loader: the same literal operation evidence feeds build enforcement and stdlib
audits, without bootstrap marker calls or a module-name exemption.

Only the current ModuleTable initializer before its first publication can seed
the namespace. A standalone same-named module or a repeated cache publication
cannot refill it. Runtime synthesis is allowed with no builtins module or in
its own actively initializing namespace. Published values win, and a completed
dictionary miss remains a miss. Exception class cache lookup never mutates the
Python namespace, so even constructing/raising a deleted exception cannot
resurrect its binding.

## Checklist

- Import `load_intrinsic` and `require_intrinsic` from `_intrinsics`.
- Required functionality must use `require_intrinsic` or raise explicit
  `RuntimeError`/`ImportError` when missing.
- Optional functionality must be explicit, capability-gated, and never fall
  back to host Python.
- Keep Python shims minimal: argument normalization, error mapping, and
  capability gating only.
- Lower hot paths and semantics into Rust intrinsics for performance and
  correctness.
- Register new intrinsics in
  `runtime/molt-runtime/src/intrinsics/manifest.pyi` and regenerate
  `src/molt/_intrinsics.pyi` plus
  `runtime/molt-runtime/src/intrinsics/generated.rs` via
  `uv run --python 3.12 python tools/gen_intrinsics.py`.
- Literal positional defaults in `manifest.pyi` are canonical metadata. The
  generator records supported concrete trailing defaults (`None`, booleans, and
  integers) in `IntrinsicSpec.defaults`; runtime registration must attach the
  matching `__defaults__` tuple to both eager and lazy intrinsic functions.
  `tests/test_gen_intrinsics.py` guards this by checking generated Rust metadata
  for default-bearing intrinsics such as `molt_operator_length_hint`.

## Lint Gate
- `tools/check_stdlib_intrinsics.py` enforces the loader contract and runs in
  `tools/dev.py lint`.
