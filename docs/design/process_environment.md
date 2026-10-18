# Process environment authority

Status: steps 1-2 implemented, 2026-10-18; steps 3-6 pending. Owner: integrator
(coordination task `claude-winenv-process-environment`). Ledger: HF-355.

## Problem

Before this migration, Molt modeled child-process environments in at least 35
places, and each place applied the Windows name rule its own way:

- `_CargoEnvironment` and `CargoEnvironment` (`cli/runtime_cargo_plan.py`) fold
  names with `str.upper()` under a `case_insensitive` flag.
- `toolchain_identity.executable_environment_value` folds `PATH` by `casefold`.
- `cargo_execution_policy`, `python_runtime_admission`, `llvm_wasi_tools`,
  `proof_queue_pkg/execution_environment`, `command_identity` and
  `cargo_output_environment` fold with `name.upper() if os.name == "nt"`.
- `platform_toolchain`, `bench_friends_env`, `release/native_build` and
  `release/compiler_payload` fold with `key.upper()` on every host, which
  merges `Path` and `PATH` on POSIX, where they are two variables.

Each copy is a chance to disagree. Two did: a plan copied its folded map into a
plain `dict` and lost the rule, and a C projection keyed entries by the folded
name while its reader used cc-rs names. On Windows that refused every Cargo
plan with C tools (30 hosted failures). bat00 shows more of the same class,
for example a lookup of `AWS_LC_SYS_USE_SYSTEM_aarch64_unknown_linux_gnu` in a
folded map.

## How leading projects do it

- Rust std, `sys/process/windows.rs`: `EnvKey` keeps the original spelling and
  compares with `CompareStringOrdinal(a, b, bIgnoreCase = TRUE)`. The code asks
  the OS to compare because Windows case folding is not a stable public rule.
  The environment block is the map in that order.
- Cargo, `context/environment.rs`: a case-preserving snapshot plus an index
  from uppercased names to real names, consulted only on Windows.
- Go, `os/exec.dedupEnvCase(caseInsensitive, nulOK, env)`: one function with an
  explicit rule parameter. Production passes `GOOS == "windows"`; tests pass
  `true` on every host. Later duplicates win.
- CPython `os.environ` on Windows stores uppercased names (`str.upper()`), so
  it loses the original spelling and expands `ß` to `SS`, which Windows does
  not do.

## Design

One module, `molt.process_environment`, owns the rule. Nothing else folds
names.

- `NameRule`: `EXACT` (POSIX) or `WINDOWS`. `NameRule.host()` is the rule of
  the host that will run the child. A test may pass `WINDOWS` on any host, as
  Go's tests do.
- `NameRule.WINDOWS` compares names by simple per-character uppercase and
  never expands a character (`ß` stays `ß`). That is the ordinal rule Rust gets
  from `CompareStringOrdinal` for every name a build tool uses. On Windows the
  module may ask the OS for the exact comparison instead; the pure rule is the
  portable oracle.
- `ProcessEnvironment`: an immutable `Mapping[str, str]`. It preserves the
  first spelling of each name, as Windows and Rust's `BTreeMap<EnvKey, _>` do.
  Lookup, `in`, `updated(values)` and `without(names)` all apply the rule.
  Iteration yields the stored spellings. `child_block()` returns the exact
  `dict` a spawn passes, in the rule's order.
- Equality requires the same `NameRule` and compares canonical names and values,
  independent of stored spelling or insertion order. A plain mapping has no
  rule and is not an equal process environment.
- `ProcessEnvironmentBuilder`: the one mutable form, for staged construction.
  It replaces `_CargoEnvironment`.
- Spawn boundary: `molt.process_guard` spawn helpers take a
  `ProcessEnvironment`, so `ty` rejects a plain `dict` there. Plans and
  identities that hold an environment hold this type
  (`RuntimeCargoPlan.environment` already holds `CargoEnvironment`, which
  becomes this type).
- Projections that are serialized (identities, C flag projections) name each
  variable as its consumer spells it (cc-rs, Cargo), never as the host folds
  it, so equal inputs give equal identities on every host.

## Costs

Hot path: none new. A lookup folds one name (bounded by the name length) and
does one dict probe, the same as today's `str.upper()` sites. Building a
`ProcessEnvironment` from `os.environ` is one pass, O(n) in the number of
variables, done once per spawn plan; today several sites rebuild their own
folded copy. Memory: one index dict beside the value dict, under 10 KB for a
normal environment.

## Migration

One indivisible change per consumer family, in this order, each with tests
that run under both rules on any host:

1. Add the module and its tests (rule, first-spelling preservation, updated,
   without, child block order, `ß`, POSIX `Path` versus `PATH`).
2. Cargo family: `runtime_cargo_plan`, `cargo_execution_policy`,
   `cargo_output_environment`, `toolchain_capture`. Delete `_CargoEnvironment`
   and `CargoEnvironment`.
3. Proof queue family: `execution_environment`, `command_identity`.
4. Toolchain discovery: `toolchain_identity`, `python_runtime_admission`,
   `llvm_wasi_tools`, `platform_toolchain`.
5. Release and bench tools.
6. Spawn boundary typing in `process_guard`; then delete every remaining
   `name.upper() if os.name == "nt"` site. A search for that spelling is the
   completion check, and the typed boundary keeps it from coming back.

## Verification

- Unit tests of the module under both rules, with Go-style rule parameters.
- The bat00 Windows cell (`windows-x86_64-py312-queue-portability`) for each
  migrated family, through the farm cell runner.
- No hosted-only proof: every family also has a both-rules test that runs on
  macOS and Linux.
