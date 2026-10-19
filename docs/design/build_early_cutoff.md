# Build early cutoff

Status: implemented, 2026-10-16 (HF-250). Owner: frontend build speed.

## Problem

A `molt build` whose inputs did not change still runs the whole pipeline to
compute the backend cache key from lowered IR. Measured on a warm checkout,
`super_comprehension_frame_ownership.py`, native, dev, backend cache hit:
32-44 s wall (`backend_prepare` 10-16 s, `module_graph` 7-9 s, the rest in
analysis, lowering, sealing and linking). Differential suites rerun unchanged
programs constantly.

## Prior art

- Cargo: per-unit fingerprints plus rustc dep-info; a no-op build costs
  0.1-0.3 s.
- ninja: `.ninja_log` and depfiles; a no-op build stats each edge's inputs and
  outputs. `ninja -d explain` names the first changed input.
- Go: action IDs hash every input; file hashes are cached by
  (path, size, mtime, inode) (`cmd/go/internal/cache`).
- Bazel, Buck and Salsa: dependencies are recorded where they are read, not
  enumerated afterwards.
- [Bazel Skyframe](https://bazel.build/reference/skyframe#evaluation) distinguishes
  file content, symlink resolution and directory membership. A read without a
  registered dependency can make an incremental result incorrect.
- git: the racy-git rule; a file changed within one timestamp tick of the
  index cannot be trusted by stat alone.
- [Cargo build directories](https://doc.rust-lang.org/cargo/reference/config.html#buildbuild-dir)
  separate intermediate graphs from final target outputs. Target rustflags
  alone cannot invalidate compiled host dependencies when `--target` is used.
- [PyO3 environment signatures](https://pyo3.rs/v0.29.3/building-and-distribution)
  connect interpreter environment changes to Cargo build-script invalidation.
- [rustc dependency tracking](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html)
  distinguishes changed inputs from changed results. Reuse is justified by the
  inputs of the consuming unit, rather than a successful upstream check alone.
- [Cargo compiler-query caching](https://doc.rust-lang.org/nightly/nightly-rustc/src/cargo/util/rustc.rs.html)
  keys the process request plus an additional dependency fingerprint. Molt's
  declared tool and resource closure must also survive preserved-mtime edits;
  reuse therefore goes through the existing file-content custody authority.
- [cc-rs metadata controls](https://docs.rs/cc/1.6.0/cc/struct.Build.html#method.cargo_metadata)
  can suppress environment dependency reporting as well as archive link output.
  Consumers that own archive linking must still invalidate C output inputs.
- [CPython bytecode validation](https://peps.python.org/pep-0552/)
  checks source metadata or its hash; it does not establish equality between
  cached code and current source. Both are build inputs.
  [`source_from_cache`](https://docs.python.org/3/library/importlib.html#importlib.util.source_from_cache)
  owns the cache layout in the interpreter that observed the read, including
  its `sys.pycache_prefix`.

## Design

**Key.** SHA-256 over the schema, every `build()` argument except `verbose`,
the working directory, the interpreter and its version, the host (system,
release, machine, and the CPU model for non-portable builds, which tune for
the host CPU), and the full environment except terminal and session
identifiers and registry entries declared output-neutral. Guard custody and
memory budgets supervise execution without selecting different optimizations.
The output path and the output directory are keyed: building one
program to two names, or under two output directories, produced binaries that
differed in 49 and 48 bytes (the linker signs the binary under its file name;
the debug map names object files under the output directory).

**Two modules.** `molt.input_report` is the reporting side: the token and
probe primitives, the audit hook, worker logs and the `report_*` functions.
Lowering code imports it, so it is part of the frontend's semantic
fingerprint; it is a leaf that imports no other Molt module, and a test keeps
the recorder out of that fingerprint. `molt.build_record` is the recorder: the
ledger, the key, the record, the check and trimming. Only `molt build`
imports it.

**Ledger.** One build records into one ledger. Only the first build of a
process records: process-global caches can serve a later build without a read
to report, whether or not the earlier build was eligible. Inputs reach the
ledger two ways. Repeated equal file, directory and lookup facts are accepted;
conflicting generations or outcomes refuse the record, including an absence
that becomes present and absent again. Later observations never erase a prior
conflict:

- Authorities report when they read: source snapshots (token from `fstat`
  before and after the read, plus the digest), the import topology (listed
  directories, lookups with their resolved target), runtime source trees,
  backend sources, Cargo resource custodies, Cargo configuration lookups,
  frontend data files behind process-global caches (on every use), project-root
  probes (including all four Molt source markers from their shared authority),
  lockfile gate inputs, rustup selection files, link tools, and the
  macOS `xcode-select` link. Every executable search on the native build
  path goes through `toolchain_identity.executable_candidates`, which reports
  each searched directory and the selected entry: a program added to an
  earlier directory, or a retargeted link, selects another executable.
- A `sys.addaudithook` observer sees readable file opens, directory listings
  and started programs. Read/write and append opens retain input dependencies.
  Audit events happen before their syscalls and never grant product ownership:
  failed writes and existing-directory `mkdir(exist_ok=True)` cannot erase a
  prior read or exclude the directory's children. Process-pool
  workers log their facts to a directory the recording process merges; a
  forked child drops the copied ledger and resets its reporting lock before
  logging its own facts. Recognized bytecode reads report both the actual
  cache file and its source in the observing interpreter, including workers
  with a different cache prefix. Windows prefixed caches discard the source
  drive ([CPython cache mapping](https://github.com/python/cpython/blob/v3.12.15/Lib/importlib/_bootstrap_external.py#L503-L524));
  the observer keeps their actual cache dependency and refuses record reuse
  when the source drive cannot be recovered. It never guesses a drive. They use the existing explicit file/probe
  reports so a consumed cache under a product root remains an input. Unmapped
  bytecode explicitly retains its actual file dependency across product roots
  without guessing a source.
  Reads under named product roots (the Molt
  cache, Cargo target and build state roots, the backend daemon's socket
  directory, the artifact root under the output directory, the memory guard's
  state root and its scratch generations) are not inputs. A
  source checkout, a user directory, the temporary directory and the shared
  scratch root are never excluded: projects live there too, and so do guard
  lease targets. The successfully
  published binary is excluded as one file with the receipt, journals, lock
  and stages its publication keeps beside it, preserving explicit input
  reports and every read under its parent directory. A failed lookup keeps
  its exact absence or error outcome. Missing intermediate components become
  probes through the nearest stat-able ancestor: its directory token (or file
  token for `ENOTDIR`) protects against an input subtree removed during the
  build. An existing path without a content-change token refuses the record.
  Only chains completed by this resolution are reused, so sibling lookups do
  not repeatedly traverse shared missing ancestors, and explicit probes never
  substitute for the generation boundary.
- A recorded build keeps its own transient files in a product root:
  `tempfile` points at a per-build directory under
  `<Molt cache>/build_records/.transient`, beside its workers' read logs, as
  Bazel gives each action a temporary directory in its output base. A file
  created and removed in an input directory changes that directory during the
  build, which the racy rule refuses. For the same reason an external artifact
  root proves it accepts child directories once and keeps the proof as a
  `.molt-artifact-root` marker. The existing `OwnedTemporaryDirectory` authority removes a
  completed build's allocation only while its directory identity still matches.
  Record retention never deletes execution scratch: a clock jump, an old
  directory or an exited parent cannot prove that descendant writers closed.
  Allocations left by an interrupted direct build stay preserved until an
  external owner establishes process-tree closure; no age-based reclaimer or
  alternate scratch lifecycle is introduced.

**Captured inputs and isolated Python.** Cargo file custody reports the existing
handle-bound token and digest, including lexical and resolved tool names. An
indirect name also records its resolution: identical bytes at a different target
do not preserve a loader's context. Rustup selection records rejected candidates'
captured bytes as well as absent candidates. Installed-manifest, pinned-tool
and startup-gate classification retain their exact lookup outcomes.

The shared Python probe suffix carries the existing worker-log directory.
The isolated facade initializes that logger after its common capture imports,
preserving runtime identity. Its capture context reports ordered file identities
without another hash or tree walk; the existing enumeration reports directories,
and startup selection reports lexical lookups, including missing archives and
configuration files. An admission created outside the current recording cannot
justify a record: the record is refused while ordinary compilation and its live
custody continue. No new admission, restart or interpreter fallback is introduced.

Record schema v6 rejects earlier records whose inputs could be incomplete.
Reporting adds O(captured paths + visited directories + selectors) facts and
JSONL bytes through the existing flushed worker log, plus exact lookup probes.
An inactive recorder retains its existing cheap reporting branch. These are
compiler/development costs; emitted programs perform none of this work. This
repair establishes input completeness for these authorities, not a whole-build
speedup or release-wide performance qualification.

**Rust metadata.** One Cargo plan owns its metadata answers in one frozen
environment. Identical commands share a discovery answer; wrapped and unwrapped
roles are identical when no wrapper is selected. The existing flag parser and
resource custody retain provisional cfg inputs as well as the selected final
inputs, including the named candidate files in declared custom-target search
directories. [rustc's target lookup](https://doc.rust-lang.org/rustc/targets/custom.html#custom-target-lookup-path)
looks for `TARGET.json`; unrelated specifications and directory subtrees are not
inputs, and an implicit host question does not search for a custom target.
Discovery locates
the resources; one replay of each distinct question under their captured
generation must agree before the plan can be used. Compiler, wrapper and
resource fences surround qualification, and the plan retains those inputs for
its later verification. This covers declared tool inputs; arbitrary wrapper
filesystem or network reads are outside that contract.

There is no process-global metadata-answer cache. On the measured macOS Rust
1.99 toolchain, eight actual metadata queries cost about 117 ms, while a valid
no-change record bypasses all resolution. A prototype that safely retained
cross-plan answers added 188 source lines for roughly 0.1 s of miss-only savings.
Keeping ownership local removes its mutable global state and invalidation
protocol. Discovery and qualification use the same local memo, with one call
per distinct command in each phase. These are bounded mechanism measurements,
not cross-platform build-performance acceptance.

**Refusals.** No record for non-native targets, directory outputs, requests
that publish more than one binary (JSON results, diagnostics, IR, linked or
split output, snapshots, audit logs), non-default codegen backends, BOLT,
file-naming inputs whose loaders do not report (type facts, PGO profiles,
runtime feedback, capability manifests, sysroots, capability files), `ty`
under the trust policy, external static packages, and any ledger refusal,
racy input, or input that changed during the build. A failed record never
fails the build; `--verbose` prints why no record was written and why a
record did not match.

**Racy rule.** `file_hashing.DIGEST_RACY_WINDOW_NS` (2 s) over
`content_change_time_ns` (ctime on POSIX, NTFS ChangeTime converted to
Unix-epoch nanoseconds on Windows; none fails closed), measured from the start
of the recording process
(`molt.process_birth`: `/proc/self/stat`, `proc_pidinfo`, `GetProcessTimes`).
A long-lived process, such as the batch build server, may hold bytes it read
long before its build began; any input changed since it started refuses the
record. An `rsync -a` keeps an old mtime but cannot keep ctime.

**Check.** Stat every recorded directory, probe and file, then the output.
A changed token on a file with a recorded digest falls back to a digest
compare. A hit when the output still holds the binary the build published
does nothing else: there is no artifact store and no copy. Malformed JSON,
including valid JSON scalars and arrays, is a cache miss. The v6 record key
retires records missing captured tool and isolated-Python inputs, including
source-only bytecode records and records produced under inferred write ownership. There is no compatibility reader for the retired contracts.

**Trimming.** A hit refreshes its record's mtime at most once an hour; records
unused for a week are deleted, at most once an hour.

## Measured

- No-change rebuild through `python -m molt.cli`: 0.155 s, from 32-44 s.
  Before the dispatcher imported each command lazily it was 0.87-1.02 s, and
  copying the binary from a store on another volume cost 1.5 s more. Keying
  the C toolchain custody of an Apple SDK by its `SDKSettings.json` took it
  from 0.43 s to 0.155 s.
- The record holds 5,985 files, 397 directories and 767 probes: 1.3 MB of
  JSON. It held 75,312 files and 17,162 directories (26.5 MB) while the C
  toolchain custody listed all 69,329 files of the Xcode SDK.
- Edits to a runtime Rust source, a stdlib module, the intrinsics manifest,
  rustup settings, a Molt CLI module and the checkout's `.cargo/config.toml`,
  and a new `.cargo/config.toml` in an ancestor, each make the record miss.

## Limits

### External inputs and Cargo reuse

An input-record miss and a changed runtime identity do not themselves make
Cargo rebuild. A real dependency-free Cargo reproduction changed an external
header from 7 to 9: the recaptured toolchain identity changed, but Cargo reused
its build-script output and the binary still returned 7. The plan's before/after
resource checks both passed. Publication must not attach the new identity to
those old output bytes.

`RuntimeCargoPlan` therefore pins `CARGO_BUILD_BUILD_DIR` and the matching final
`--config build.build-dir` to a content-addressed intermediate directory. Its
key contains the existing captured executable/wrapper identities, Rust host
and target resources, C resources and output-bearing C environment, final-link
resources, and selected WASI SDK. LLVM's already admitted compiler SDK joins the
same resource custody before compiler identity capture. This covers compiled
host dependencies, target build scripts and nested CMake output through one
authority. The managed final `CARGO_TARGET_DIR` is also pinned through a final
`build.target-dir` override: earlier inline Cargo configuration cannot redirect
published outputs outside their admitted root.
The resolver requires Cargo 1.91 or newer, the version that stabilized
`build.build-dir`, using a guarded probe tied to the captured executable.
Older Cargo can ignore the configuration key, so successful execution alone
would not establish this capability.

The base is `CARGO_BUILD_BUILD_DIR` when explicitly supplied in the incoming
environment, otherwise the managed `CARGO_TARGET_DIR`. The plan retains that
base as `MOLT_CARGO_BUILD_DIR_BASE` when its environment is resolved again.
Molt owns and pins the intermediate layout; Cargo configuration cannot replace
the admitted generation in the child. Resource captures and their cached
content projections are reused: no second SDK enumeration or executable probe
is introduced to compute this key.

Rust source, build-Python content, job counts, output paths and unrelated Cargo
configuration bytes are excluded from this graph key. Ordinary source edits
retain Cargo's incremental graph; actual toolchain/resource changes pay for a
fresh graph and its disk storage. Old generations remain ordinary development
artifacts under the selected build root, subject to its cleanup and custody.
Python-generated tables use the captured interpreter digest as a separate
build-script environment signature, so interpreter drift invalidates their
generators without selecting a fresh graph for every Rust dependency.
Apple deployment controls belong to the same C input authority: with metadata
suppressed, a real explicit-host Cargo control otherwise reused a stale
build-script result after changing `MACOSX_DEPLOYMENT_TARGET`. The macOS, iOS,
watchOS, tvOS and visionOS controls are scalar inputs, not SDK search paths.
Effective generator selection is pinned to an absolute lexical entrypoint after
Cargo configuration merging and before identity capture. Relative selectors and
PATH entries retain their operation-directory meaning when build scripts run
from a crate directory; virtualenv aliases and directory-symlink traversal keep
their executable semantics.

Queued Cargo compilation captures this generator as a contextual prerequisite,
independently of the proof launcher's site-enabled interpreter. Command and
lane declarations identify runtime-generation work; reviewed
compiler-only operations avoid the generator inventory and its probes. An
undeclared compiling Cargo operation retains conservative generator custody.
The lease owns both final outputs and the intermediate base. Its runtime-WASI receipt
validates transport and canonical argv; the executing runtime driver resolves
and verifies the actual resource generation through `RuntimeCargoPlan`. Receipt
shape alone does not prove external resource freshness.

This is development/build-time work. It adds no checks, interpreter dependency,
storage or startup work to emitted programs. It establishes freshness of
captured inputs, not target-version Unicode semantics or native errno policy;
those remain V1-22/V1-23. Bare Cargo retains its documented dependency model.

- Inputs on a network filesystem whose clock is behind this host's can pass
  the racy rule; git, ninja, make and Cargo share this limit.
- A process that writes the output while the build that publishes it runs
  can leave a record of the wrong bytes, as with make and ninja.
- Process-pool workers keep the system temporary directory; no worker code
  creates temporary files today, and one that does refuses the record.
- The publication lists the output's directory for its own journals, so the
  audit hook's listing of that directory is custody. A reader that depends on
  the entries of the output's directory must report the listing explicitly, as
  the import topology does; explicit reports are never excluded.
- Read-back of a fresh product outside the named product roots and the declared
  output remains an input and may conservatively refuse the record. Such a
  producer must use existing product custody; audit events cannot prove it.
- The audit hook is a backstop for named-path reads, not a complete dependency
  protocol for arbitrary descriptor I/O. Integer-descriptor opens and directory
  listings carry no pathname; relative `os.open(..., dir_fd=...)` audit events
  omit their directory descriptor. A semantic reader using these forms needs
  explicit input authority before that path can claim early-cutoff coverage.

## Review history

The first prototype collected inputs at the end of the build; an adversarial
review found 13 gaps, which the read-time ledger closed. A second review found
six ways to serve a stale binary: excluding the custody root and the output's
parent dropped reads under the checkout (`.cargo/config.toml`); tokens taken
at record time could postdate the bytes a long-lived process held; forked
workers lost their reports; tool selection recorded only the resolved
program; stat probes raise no audit event; and an artifact store that shared
an inode with the output could be corrupted in place. Each has a regression
test. The review also showed the output name and directory change the binary,
which removed the artifact store.

## Next

1. `runtime/molt-launcher` (the native `molt`) checks the record in parallel
   before it starts Python, keyed by the invocation; the hit then costs tens of
   milliseconds.
2. A compact record.
3. Stage-level cutoff: the runtime and backend identity checks (13 s of a warm
   build) reuse the ledger to prove by stat that nothing changed.
   Their source half comes from Cargo's own per-unit outputs
   ([Cargo input authority](cargo_input_authority.md)).
4. Runtime tables use pinned Unicode data selected by the target Python version;
   native errno uses target headers with CPython's conditional publication
   policy. Retire build Python after all generated consumers migrate.
5. A directory the build lists records the projection it used (its module
   candidates), not its token, so a new log or editor file beside the entry
   does not invalidate the record; Bazel's `GlobValue` cuts off the same way.
6. The racy window follows each input's filesystem timestamp granularity
   instead of a fixed 2 s, so an edit saved just before a build still records.
