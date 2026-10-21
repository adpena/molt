# Toolchain fact cache

Status: implemented in `src/molt/toolchain_facts.py`; the six probe classes
below read it. Ledger row HF-357 in `docs/agent/V1_HANDOFF_FINDINGS.md`
records the measurements.

## Problem

A warm `molt build` of `examples/hello.py` with an unchanged toolchain
spawned 32 to 36 children through `process_guard.run_completed_command`
(three samples, 2026-10-21, macOS arm64, load 26 to 30). 31 of them asked
questions the toolchain had already answered in the previous build:

| probe | site | per warm build |
| --- | --- | --- |
| rustc target metadata (`--print cfg/sysroot/file-names/split-debuginfo`) | `cli/runtime_cargo_plan._rust_target_metadata` | 16 (10 through sccache, 6 unwrapped) |
| rustc host (`-vV`) | `cli/runtime_cargo_plan.resolve_runtime_cargo_plan` | 3 |
| `rustup which rustc` / `cargo` | `rust_toolchain.resolve_rustup_proxy` | 6 |
| `cargo --version` | `cli/runtime_cargo_plan._require_cargo_build_directory` | 3 |
| `xcode-select --print-path`, `xcrun --show-sdk-path` | `platform_toolchain` | 2 |
| C driver `-print-prog-name=ld` | `cli/native_link_tool_identity._system_linker_from_driver` | 1 |

Three Cargo plans are resolved per build (backend admission, the runtime
build, the runtime identity), and each plan asked every question again.
The remaining one to five children are link work (`clang`, `lipo`,
`dyld_info`, `llvm-strip`, `codesign`) that the link cache owns.

Each child costs a fork/exec, dynamic loading of a 400 KB launcher plus an
80 MB `librustc_driver`, and 10 to 60 ms of wall time; sccache-wrapped
probes also contact the sccache server. Cargo and Go spawn none of these on
an unchanged build.

## How leading tools do it

- Cargo, `src/util/rustc.rs`: one file, `target/.rustc_info.json`, holds
  `{rustc_fingerprint, outputs: {key: output}, successes: {key: bool}}`.
  `rustc_fingerprint` hashes the resolved rustc path, its length, creation
  and modification time, the same for `RUSTC_WRAPPER` and
  `RUSTC_WORKSPACE_WRAPPER`, and for a rustup proxy `RUSTUP_TOOLCHAIN`,
  `RUSTUP_HOME` and the real toolchain rustc's mtime. Each output is keyed
  by a `process_fingerprint` of the argv and the variables the command sets
  explicitly. A fingerprint change discards every output. Cargo caches
  failures too; the file is written once when dirty and a write failure is
  ignored.
- Go, `cmd/go/internal/work/buildid.go`: `toolID` identifies each tool
  (`compile`, `link`, `asm`) by its `-V=full` output, a release version or a
  content-derived build ID, memoized per build process; every action ID in
  the build cache depends on that tool ID, so a changed compiler invalidates
  its outputs without a per-build probe of the sources.

Molt follows Cargo's shape (one host file of process outputs keyed by a
tool fingerprint) with a stricter key, because a stale answer is a
miscompile or a wrong link.

## Design

One authority, `molt.toolchain_facts`:

- `ProbeTool`: an executable the probe runs, by invoked path, resolved
  content path, size, SHA-256 and stat token `(dev, ino, mode, size,
  mtime_ns, ctime_ns)`. Identities come from the existing executable
  authority: `toolchain_identity._capture_executable_content` for rustc and
  its wrappers, the plan's `CargoExecutableCustody` for cargo, and the host
  `FileDigestCache` for the selectors and the C driver, so an unchanged tool
  costs one `lstat` there.
- `ProbeKey`: the probe class, the tools in execution order, the argument
  vector, the working directory, a typed environment subset (present or
  absent, under the captured name rule) and file inputs by no-follow token
  (absent is a value; a symlink carries its link text and the token of the
  file it resolves to, because the tool reads through it). Its digest is the
  canonical JSON SHA-256.
- `ProbeAnswer`: the process's stdout and stderr. Only zero-exit answers are
  kept; a failure is raised by its consumer as today and asked again next
  time.
- `ToolchainFactStore`: one JSON file,
  `default_paths.toolchain_fact_store_path(cache_root)` =
  `<cache>/toolchain-facts/v1.json`, beside the digest cache in the host
  cache namespace. The file is read once per process under the shared cache
  lock; a missing, corrupt or foreign file, or a record whose key does not
  hash to its digest, reads as absent and is replaced by the next write.
  Each write merges over the current host generation under the lock, drops
  records older than the cache age budget (`MOLT_CACHE_MAX_AGE_DAYS`, 30 by
  default) and replaces the file atomically. A record read in the second
  half of its age is refreshed, so a toolchain in use is never re-probed for
  age alone.
- `completed_probe(store, key, run, accept)`: serve the recorded answer,
  else run the probe and record a zero-exit answer. `run` returns only after
  its own fence (`ProbeTool.fenced`) proved every executable unchanged
  across the process, so an answer is never recorded under a key that names
  other bytes. `accept` rejects a recorded answer that no longer holds (a
  selected path that was removed) and the probe runs again.

The store lives in the host cache namespace, not the build-state root the
first draft proposed, for three reasons: the answers are host facts (a tool's
output for its inputs) and are the same for every checkout; three of the
six probe classes (`xcrun`, `xcode-select`, the link driver) have no
project root to address a build-state root with; and the digest cache that
proves tool bytes already lives there under the same budget and lock
family.

### Keys per probe class

Every key carries the tools, argv, cwd and the typed environment subset
below. Inputs are the files the tool consults for that question. "Why
sufficient" names the mechanism that would otherwise change the answer.

| probe | tools | environment | inputs | why sufficient |
| --- | --- | --- | --- | --- |
| `rustc-target-metadata`, `rustc-host` | wrappers then rustc (`_rust_probe_command`) | `RUSTC_PROBE_ENVIRONMENT`: `RUST_TARGET_PATH`, `RUSTC_BOOTSTRAP`, `RUSTC_FORCE_RUSTC_VERSION`, `RUSTC_OVERRIDE_VERSION_STRING`, `RUSTUP_TOOLCHAIN`, `RUSTUP_HOME`, `SDKROOT`, `DEVELOPER_DIR`, the Apple `*_DEPLOYMENT_TARGET` names, `DYLD_LIBRARY_PATH`, `DYLD_FALLBACK_LIBRARY_PATH`, `DYLD_INSERT_LIBRARIES`, `LD_LIBRARY_PATH`, `LD_PRELOAD` | every direct file in `<sysroot>/lib` and every dynamic library beside the launcher; for each target name in the question: `<RUST_TARGET_PATH entry>/<name>.json`, `<sysroot>/lib/rustlib/<name>/target.json`, or the `.json` path itself; an explicit codegen-backend library; every `rustlib/*/codegen-backends` listing | The launcher is 400 KB; the compiler is `librustc_driver`, loaded from `../lib` (Windows: beside the launcher), so its token is keyed. The target spec is built in or found by `Target::search` in exactly those places. `cfg(target_feature)` comes from the codegen backend. The dynamic loader variables can redirect which driver loads. The argv is normalized with one scratch placeholder (`rust_probe_arguments`), so the per-call temporary directory does not fragment keys. A wrapper is keyed by its bytes, as in Cargo; its transparency for `--print` questions is assumed, as Cargo assumes. |
| `rustup-which` | the selected proxy, the rustup CLI (`rustup_proxy_selection`) | `RUSTUP_PROBE_ENVIRONMENT`: `RUSTUP_TOOLCHAIN`, `RUSTUP_HOME`, `RUSTUP_AUTO_INSTALL`, `HOME`, `USERPROFILE` | `rust-toolchain.toml` and `rust-toolchain` in every directory of the resolved root's chain and of the root's chain as spelled; on Unix the fallback settings file (`RUSTUP_OVERRIDE_UNIX_FALLBACK_SETTINGS`, joined to the resolved root when relative, else `/etc/rustup/settings.toml`); `<rustup home>/settings.toml`, `<rustup home>/toolchains` and every `toolchains/<name>/bin` listing, with the home from `rust_toolchain.rustup_home` | rustup's override chain is the explicit toolchain, the toolchain files from its working directory upward, then `settings.toml`, then the Unix fallback settings. It walks the physical directory on POSIX (`getcwd`) and the spelled one on Windows (`GetCurrentDirectoryW`), so both chains are keyed. The home follows the `home` crate (`rustup_home_with_cwd`): a relative `RUSTUP_HOME` joins the working directory, Windows reads `USERPROFILE`, and no home leaves its absence in the environment inputs. The installed set and each toolchain's `bin` listing decide which binary a component resolves to, including the cargo fallback to another channel. A recorded path is served only while it names a file. `test_store_answers_what_real_rustup_answers` checks each input against a real `rustup which`. |
| `cargo-version` | cargo (its `CargoExecutableCustody` identity) | `CARGO_VERSION_ENVIRONMENT`: `RUSTUP_TOOLCHAIN`, `RUSTUP_HOME` | none | `cargo --version` prints the binary's own version; the rustup pair covers a proxy that was not recognized as one. |
| `xcode-select-path`, `xcrun-sdk-path`, `xcrun-find` | the selector binary (digest cache) | `DARWIN_SELECTOR_ENVIRONMENT`: `DEVELOPER_DIR`, `SDKROOT`, `TOOLCHAINS` | `/var/db/xcode_select_link` (target included); the real `<developer>/usr/bin/xcrun`; for the SDK: the `SDKs` directories and `MacOSX.sdk` links under the developer directory; for `--find`: `<developer>/usr/bin`, every `*.xctoolchain/usr/bin` under `<developer>/Toolchains` and `~/Library/Developer/Toolchains`, and the SDK's `usr/bin` | The `/usr/bin` shims dispatch on the link and `DEVELOPER_DIR`; xcrun resolves `macosx` to the `MacOSX.sdk` link and finds tools in those directories, with `TOOLCHAINS` selecting an alternate toolchain. A recorded path is served only while it is a directory (SDK, developer dir) or a file (tool). |
| `link-driver-linker` | the C driver (digest cache) | `LINK_DRIVER_PROBE_ENVIRONMENT`: `PATH`, `COMPILER_PATH`, `GCC_EXEC_PREFIX`, `DEVELOPER_DIR`, `SDKROOT`, `TOOLCHAINS` | `/var/db/xcode_select_link` on macOS | clang and gcc search their own program paths, `COMPILER_PATH`, the GCC prefix, then `PATH`; the Apple shim dispatches on the developer selection. The probe inherits the process environment, so the key reads it. A recorded name is served only while it resolves beside the driver or on `PATH`. |

Not cached: `vswhere` and `VsDevCmd` on Windows (`platform_toolchain._query`).
`VsDevCmd && set` prints the whole environment, a function of the whole
environment; `vswhere` reads the installer's instance database. Both stay
per-build probes until a key for them is designed and measured.

### Qualification of rustc answers

`resolve_runtime_cargo_plan` asks each rustc question twice: once to
discover resource locations, once after capturing the resource closure, to
prove the answer under the captured generation. A first answer is
provisional: it lives in this process only. The replay with `qualify=True`
runs rustc again and persists the answer only when both agree; on
disagreement nothing is persisted and the plan fails closed as before. A
persisted answer satisfies both asks without a process. The fences around
the replay (`metadata_resources.verify`, `rustc_custody.verify`) still
bracket the key computation, so the tokens the key reads and the captured
resources are one generation.

### Costs

Hot path (warm toolchain, per `molt` process): one lock acquire, one read
and parse of the store (about 40 KB for one toolchain and two targets), then
per question: `lstat` of each tool (identity through the digest cache or the
plan's custody, no hashing on POSIX), 2 `scandir` plus about 15 `lstat` for
the rustc driver libraries and target-spec candidates, about 20 `lstat` for
the rustup toolchain-file chain, one canonical-JSON SHA-256 of a 2 KB key
and a dict lookup. No process, no file content read, no write.

Cold path (a new toolchain generation): the probe as today plus one
read-merge-write of the store under the lock per recorded answer (the rustc
replay records once per question). Each write is one `os.replace`. A build
that discovers a whole toolchain writes about ten times.

Measured on `examples/hello.py` (macOS arm64, `molt build --verbose`,
children counted at `process_guard.run_completed_command`, 2026-10-21).
The interleaved pairs ran the lane base (cb9413ec2) and the lane from
separate Cargo targets under the same venv, alternating, at a load average
of 17 to 23; the link steps (`clang`, `lipo`, `dyld_info`, `llvm-strip`,
`codesign`) re-ran in every sample because each sample named a new output.

| state | children | of which probes | wall |
| --- | --- | --- | --- |
| base, warm toolchain, interleaved (3) | 30, 30, 30 | 25 | 8.09 s, 6.25 s, 8.58 s |
| lane, warm store, interleaved (3) | 5, 5, 5 | 0 | 5.86 s, 5.67 s, 6.59 s |
| lane, cold store (first build after the change) | 18 | 11 (once per distinct question) | 12.8 s at load 10 |
| lane, warm store, same output path (3) | 1, 1, 1 | 0 | 4.20 s, 4.36 s, 3.81 s at load 9 |
| base, earlier, same output path (3) | 36, 32, 32 | 31 | 8.59 s, 5.47 s, 11.34 s at load 26 to 30 |

The host store after the cold build holds 9 records (two rustc questions,
the host query, two rustup selections, the Cargo version, the two Darwin
selectors and the linker) in 20.7 KB.

### Soundness notes

- A changed rustc binary at the same path, a changed driver library, a
  changed wrapper, a changed rustup selection (file chain, settings,
  `RUSTUP_TOOLCHAIN`), a changed SDK selection (link target, `DEVELOPER_DIR`,
  `SDKROOT`) and a changed search environment each change the key.
- An answer is recorded only after its probe's fence passed: the bytes that
  ran are the bytes the key names.
- Unrelated environment variables are not keyed. A variable that a tool
  reads and that is missing from its set is a defect in the set; the sets
  are documented constants with the mechanism that justifies each name.
  Checked on rustc 1.99.0: `RUSTC_OVERRIDE_VERSION_STRING=9.9.9` makes
  `-vV` print `rustc 9.9.9`; without `RUSTC_BOOTSTRAP` the `-Z` options
  the cfg query may carry are refused.
- A rustup proxy copied beside no `rustup` binary is not recognized as a
  proxy, today and before this change; the rustup variables in the rustc
  key narrow that gap.

## Tests

`tests/test_toolchain_facts.py` counts spawns through a fake at the real
`process_guard.run_completed_command` boundary and proves, per probe class,
that an unchanged toolchain spawns nothing in a new process, that each key
input forces a probe (tool bytes with a restored mtime, wrapper bytes, the
driver library, a target spec that appears, the environment subset, the
argument vector, the working directory, the toolchain-file chain,
`settings.toml`, a newly installed toolchain, a removed answer path, the
developer link, SDK and toolchain listings, `COMPILER_PATH`), that an
unrelated variable does not, that a failed or disagreeing probe records
nothing, that a corrupt or foreign store is discarded and rewritten, that
aged records are dropped and used ones refreshed, that parallel writers
merge, and that every `ProbeKey` field changes the digest. A real-toolchain
test resolves a Cargo plan in a fresh process view and asserts zero probes.

## Test custody

The pytest conftest isolates the default host fact namespace and its process
memo for every test. It creates the private directory lazily on the first fact
request, including requests through previously imported selector functions.
Synthetic successful probe answers therefore cannot publish into the developer's
shared store or satisfy another test. This does not redirect the session's
`MOLT_CACHE` or discard real build-cache reuse. Tests of explicit shared-store
ownership use `toolchain_fact_store(root)` or `ToolchainFactStore(root)` with a
private root; child-process integrations use `isolated_molt_cache` to pass that
private namespace in their environment.
