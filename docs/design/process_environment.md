# Process environment authority

Status: all six implementation steps migrated; qualification is recorded in
the HF-355 row of `docs/agent/V1_HANDOFF_FINDINGS.md`. The release matrix remains
the authority for supported execution cells.

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

One module, `molt.process_environment`, owns environment-name comparison.
Consumers select its explicit host or portable declaration rule; filename,
tool-role and secret-redaction comparisons have separate semantics.
Hidden compiler override removal uses the portable declaration rule on every
host, while retained SDK/search variables keep the input snapshot's own rule.
A POSIX `Path` and `PATH` therefore remain distinct even when mixed-case
compiler-control names are removed.

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
  It replaces `_CargoEnvironment`. DX, discovery, Cargo, proof, release and
  benchmark producers retain the rule through construction and freeze complete
  environments. Consumers that continue construction use a builder or `updated`.
- Spawn boundary: `molt.process_guard` spawn helpers take a
  `ProcessEnvironment`, so `ty` rejects a plain `dict` there. Plans and
  identities that hold an environment hold this type, including
  `RuntimeCargoPlan.environment`. Dictionaries belong at the OS or serialized
  data boundary; declared overlays remain ordinary mappings.
- Fixed consumer projections name variables as their consumer spells them
  (cc-rs, Cargo), never as the host folds them. Execution receipts retain actual
  exported spellings because those records attest the child block. Their raw
  declaration and receipt admission checks precede snapshot construction;
  canonicalization must not conceal conflicting claims.
- The source-isolated process guard retains its existing per-checkout loader.
  Its environment type and the admitted harness's type have different nominal
  identities. Each boundary projects the closed `NameRule` value explicitly
  into the recipient's enum before constructing its snapshot. Ordinary copies
  never guess a foreign object's rule. This preserves worktree custody without
  a second registry, import alias or fallback.
- Portable proof declarations reject Windows-equivalent duplicate or locked
  names on every host. Actual child lookups use the captured rule. Windows
  executable suffixes, search separators and filesystem rules still describe
  the executing platform, independently of the environment-name test coordinate.
- The dependency-free release bootstrap verifies both path and environment
  source bytes before executing either in an isolated in-memory package with
  no filesystem search path. It neither imports an ambient Molt installation
  nor executes an unverified dependency. Both source digests define its locked
  environment generation.
- Development gates bind structured Python commands to their admitted virtual
  environment executable before spawning. Windows CreateProcess searches the
  parent's PATH, not the child's block ([CPython subprocess contract](https://docs.python.org/3/library/subprocess.html#subprocess.Popen)).
  Shell gates keep source text as a single custody field through to the native
  spawn boundary. `process_spawn.native_popen_command` applies CPython's outer
  shell quoting to explicit `cmd.exe /c` or `/k` source; ordinary argv retains
  CRT serialization. Guards, direct probes and owned children share this one
  encoding. No additional process, temporary script or guest work is needed.

## Costs

These costs belong to compiler and development processes, not emitted guest
binaries. A lookup folds one name and probes one dictionary: O(name length).
Building from a raw mapping or freezing mutable construction is linear in the
input names and values; one index stores canonical names and original
spelling/value pairs. Child export sorts that index, O(n log n), only when a
dictionary is required. Copying an existing immutable snapshot with the same
rule shares its immutable index in O(1); mutable sources and rule changes still
produce independent snapshots. The two nominal source-custody boundaries require
linear representation transfers, using the existing cached module loader.
No syscall, process, lock, cache or registry is added to environment lookup.

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
6. Spawn boundary typing in `process_guard`, including DX, harness, wrapper,
   installation and test construction consumers. Remove the replaced folding
   and flattening paths. Source searches establish migration completeness;
   the typed boundary rejects subsequent plain-dictionary callers.

## Verification

- Unit tests of the module under both rules, with Go-style rule parameters.
- Actual bat00 Windows x64/Python 3.12 execution for each migrated family,
  using the existing source-bound guarded runner and exact source/tool custody.
- No hosted-only proof: every family also has a both-rules test that runs on
  macOS and Linux.
- The existing guard tests exercise real source selection, both nominal
  boundaries and DX/limit resolution, intercepting only the final launch.
  A positive/negative `ty` fixture proves typed snapshots are admitted while
  plain dictionaries are refused. Bootstrap tests run plain CPython with
  `-I -S` and reject tampered dependency bytes before executing either module.
