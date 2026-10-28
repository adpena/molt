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
- [Bazel's action cache](https://bazel.build/remote/caching) binds actions to
  inputs, output names, commands and environment. Molt likewise records the
  output selected by the successful action; a request spelling alone does not
  determine the output of a directory or package-entry build.
- [Git's racy-index rule](https://git-scm.com/docs/racy-git): a file changed
  within one timestamp tick of the index cannot be trusted by stat alone.
  Resolution alone is insufficient: [Linux multigrain timestamps](https://docs.kernel.org/filesystems/multigrain-ts.html)
  distinguish clock precision from timestamp generation, and [Windows file times](https://learn.microsoft.com/en-us/windows/win32/sysinfo/file-times)
  can remain unchanged until a writing handle closes. Molt instead binds the
  source bytes actually consumed and checks those bytes on every replay.
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
Lowering code imports it; the frontend semantic fingerprint
([frontend_semantic_fingerprint.md](frontend_semantic_fingerprint.md))
classifies it as reporting, so it stays outside that key. It is a leaf that
imports no other Molt module, and a test keeps it and the recorder out of the
key. `molt.build_record` is the recorder: the ledger, the key, the record, the
check and trimming. Only `molt build` imports it.

**Native JSON results.** Ordinary native binary JSON requests use this same
record and dependency closure, with JSON retained in the request key. Their
successful result owner reports frozen native build facts, original emission
provenance and the promised binary/archive/stub/runtime identities. The archive,
stub and runtime identities are captured before linking and verified again after
finalization and after build cleanup; later observations never replace those
expectations. Compiler and final-binary observations must agree with the
canonical publication observation. Plain builds do not capture these identities. Detached result validation compares
current content with the original expected SHA-256 through the existing host
file-digest cache. Its shared policy always hashes on Windows and hashes racy
POSIX captures; only settled unchanged POSIX observations can reuse a digest.
Result validation batches cache publication across its artifact family. Root
claims and product-store renewals retain selections but never substitute for
this content check or change the expected digest.

The existing artifacts-root, checkout-state and product-store admission
owners report their actual selections into the same ledger. JSON reuse reacquires
these claims in producer order before validating artifacts and holds scope claims
through result emission. Root identity binds the physical directory and resolved
coordinate, not its changing mtime. Product generations renew through the existing
`record_use` path, including uses whose renewal is throttled; a hit performs no
product retention sweep. The result cannot infer custody from an archive parent.

A reused result reports `cache.hit_tier = "build-record"` and `link.skipped = true`.
It retains `emission_origin` unchanged as the original archive's provenance and
keeps `compiled_with_verified = false`; it does not claim another compiler or
linker execution. Backend/function cache paths and daemon observations are omitted
because this invocation did not consult them. The canonical result owner rebuilds
the success envelope and message. Nonempty warnings, process output, diagnostic
results, auxiliary external-native artifact families and incomplete toolchain
observations refuse JSON recording. Existing proof-bearing and unsupported request
exclusions remain unchanged. Missing, replaced or retargeted promised artifacts
require a build; JSON replay never returns a stale path as a current result.

**Python startup.** [CPython's site hook](https://github.com/python/cpython/blob/v3.12.15/Lib/site.py)
runs before the user command. Molt's existing hook classifies test invocations
without initializing DX, temporary-artifact storage or toolchain identities.
Those dependencies load at their actual test consumers; there is no additional
import hook or cache. The leaf path functions remain bound before tests start:
pytest phase reporting must survive tests that replace the imported package.
Actual `-c`, `-m` and script controls exercise ordinary startup, while the owning
guard and package-isolation controls exercise test admission and publication.
This changes compiler/development startup only; emitted native and WASM programs
acquire no checks or work.

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
  and started programs. CPython raises every `open` event with integer flags:
  `FileIO` and `os.open` pass their `open(2)` flags, and the `fopen`-style
  producers (`_Py_fopen_obj`, `_Py_wfopen`, `readline`) pass 0, which counts
  as a read. An event without integer flags refuses the record.
  Read/write and append opens retain input dependencies.
  An open that creates a file (`O_CREAT|O_EXCL`, `O_TMPFILE`) or truncates one
  with write access reads nothing that existed before it, so it is no input:
  `tempfile` opens a guard's capture files that way, and their removal made the
  lease's parent a changing directory input that refused every record (HF-250).
  An exclusive create (`x` mode or `O_CREAT|O_EXCL`) is no input either,
  although it succeeds only on an absent name. A reader whose result depends
  on that outcome reports the name with `report_probe` before it creates.
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
  directory, the derived backend IR lease directory for both the project and
  compiler roots, the artifact root under the output directory, the memory guard's
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

**Import lookup scope.** A successful `PythonImportTopology.probe` consumes the
selected path's resolved target and file type. Its existing exact probe report
captures those facts; it does not consume the other entries in the parent.
Internal directory observations and operation-local validation remain intact.
An actual enumeration reports its directory before reading entries. A negative
probe retains the nearest existing directory generation, including missing
ancestors and `ENOTDIR`, so a removed input subtree cannot grant a reusable
record. This follows Skyframe's separation of file lookup and directory listing
dependencies rather than excluding a shared temporary or user directory.

Positive metadata-only probes no longer add a parent stat/report or worker-log
entry. Listings retain one report per captured enumeration; negative lookups
retain ancestor traversal and generation reporting. No new cache, process,
serialization protocol, lock or emitted-program work is added. Actual record
reuse controls distinguish unrelated parent churn from negative/listing
generations, changed entry types and retargeted symlinks. The native macOS arm64
latency observation is documented in [PERF_AUTHORITY.md](../../tools/PERF_AUTHORITY.md);
it does not qualify other hosts, WASM, or the remaining racy-window policy.

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

On Windows, observer activation initializes its existing file-metadata API
before enabling either the sink or worker log. Importing `ctypes.wintypes`
while observing its own bytecode reads would otherwise recursively initialize
that API from a partial module. This is a one-time reporting setup cost;
ordinary imports and emitted binaries do not initialize the observer.

Product comparisons use the shared path identity authority after filesystem
resolution. Windows DOS/UNC and verbatim spellings compare in one namespace,
including case and component boundaries; the recorded input spellings remain
unchanged. Comparison keys are never opened, so unusual verbatim names retain
their identity. Root keys are computed once and each observation is resolved
once, with no additional filesystem traversal.

Windows directory tokens retain identity, type, modification time and NTFS
ChangeTime, with a zero length field. Consecutive live queries returned 4096
and 0 for unchanged directories: CPython's
[by-name and handle stat conversions](https://github.com/python/cpython/blob/v3.12.15/Python/fileutils.c#L1100-L1165)
obtain lengths from different Windows metadata structures. Directory allocation
length is not a namespace generation. Regular-file lengths remain exact, and
the existing timestamp race rule and namespace mutation controls still apply.

Record schema v7 rejects earlier records whose inputs or output selection could
be incomplete.
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

**Output selection.** Native binary records support default, absolute, relative
and directory destinations, including `--out-dir`. The actual output layout
reports its default name, project/out-dir context and selected coordinate through
the existing input ledger. The request key and recorded entry, configuration and
project/import topology bind that context. A hit reruns the same side-effect-free
path selector and checks the resolved destination before checking the published
binary. It does not resolve or parse the entry again, guess a script stem, or
construct a second output-path policy. Package `__main__` naming stays with the
existing entry authority.

Absence and a regular file share the explicit-file selection branch, so the
first publication can record without mistaking its new binary for a changed
input. A destination becoming a directory, or a symlink redirecting an output
to another coordinate, invalidates the record. Output directories and artifact
claims are created only by the existing build layout, never by record checking.
This adds one small selection record and bounded path checks; the existing
source, tool, environment, output-token and racy-input checks remain unchanged.
Eligible native JSON requests retain their typed result through this same record.
Diagnostic requests still execute the pipeline because they require fresh
measurements; unsupported additional result families also retain that path.

**Refusals.** No record for non-native targets, requests
that request unrecorded additional products or measurements (diagnostics, IR,
linked or split output, snapshots, audit logs), non-default codegen backends, BOLT,
file-naming inputs whose loaders do not report (type facts, PGO profiles,
runtime feedback, capability manifests, sysroots, capability files), `ty`
under the trust policy, external static packages, and any ledger refusal,
racy input, or input that changed during the build. A failed record never
fails the build; `--verbose` prints why no record was written and why a
record did not match.

**Consumed source bytes.** `PythonSourceSnapshot` owns the immutable bytes
used by decoding, AST construction and source hashing. While recording, its
reader scopes a receipt to the one primary open, with before/after descriptor
and pathname generations and lexical selection checks. It preserves ordinary
source-open sharing; unavailable generation evidence refuses reuse without
preventing source parsing. Finite traversal reuses the captured-file iterator
and rejects growth beyond the admitted extent. Source hash consumers bypass
metadata-only memoization while recording. Resolution caches retain snapshots,
and graph validation compares captured content rather than detached text.

A receipt has a monotone process-local identity bound to its recording, process
and thread. Start and completion travel through the same worker fact protocol;
pending, unmatched, duplicate, aborted or crossed-lifetime receipts cannot
publish a reusable record. Only that receipt's primary open is covered. Raw
reads before or after it, another open and other threads remain independent
dependencies. Plain-fork children discard the inherited ledger and establish
their own reporting session while retaining worker-log custody.

Cached text, ASTs and digests report their snapshot's actual immutable buffer
once per recording, including an inherited snapshot used in a fork. The
acknowledgment and publication are atomic under the recording lifecycle lock.
Deduplication lives on the snapshot, with no global strong-owner registry that
would retain temporary source bytes or ASTs.

The ledger unions closed `FileEvidence` obligations: unbound audit reads,
custody captures and consumed bytes. A supplied digest alone is not consumed
proof. Changed generations and distinct digests under an equal token refuse
permanently; later equal observations cannot erase an earlier conflict. Every
consumed row has persistent checksum validation, including an initially old or
cached snapshot. Its checksum never ages into metadata-only trust. The explicit
input facts survive named product-root projection: generated MessagePack leases
are products, but an explicitly consumed source inside their namespace still
invalidates the build. `dx.backend_ir_lease_dir` is the single namespace used by
the producer and projection; transport, scratch selection and cleanup are unchanged.

**Racy rule.** Except for files proven exclusively consumed as above,
`file_hashing.DIGEST_RACY_WINDOW_NS` (2 s) over
`content_change_time_ns` (ctime on POSIX, NTFS ChangeTime converted to
Unix-epoch nanoseconds on Windows; none fails closed), measured from the start
of the recording process
(`molt.process_birth`: `/proc/self/stat`, `proc_pidinfo`, `GetProcessTimes`).
A long-lived process, such as the batch build server, may hold bytes it read
long before its build began; weakly observed inputs changed since it started
refuse the record. Directory and negative-lookup protections remain intact.
An `rsync -a` keeps an old mtime but cannot keep ctime.

**Check.** Stat every recorded directory, probe and file, then the output.
A consumed source row checks current bytes through the stable-file hash owner
even when tokens match and after aging. A metadata row with a changed token and
a recorded digest falls back to a digest comparison. A hit when the output still holds the binary the build published
does nothing else: there is no artifact store and no copy. Malformed JSON,
including valid JSON scalars and arrays, is a cache miss. The v9 record key
requires explicit metadata/checksum modes and typed native result/claim fields;
old epochs and malformed modes are misses. Captured tool, isolated-Python and actual bytecode inputs remain required.
There is no compatibility reader for retired contracts.

**Trimming.** A hit refreshes its record's mtime at most once an hour; records
unused for a week are deleted, at most once an hour.

## Measured

- macOS arm64, CPython 3.12.15, native dev, matched `bench_calls.py` body edit
  and retained project/cache/output: the first unchanged build after the edit
  changes from 2.334 s to 321 ms. All 20 subsequent ordinary CLI observations
  explicitly hit the record: p50 314 ms, nearest-rank p95 333 ms, maximum 341 ms.
  Source-byte validation is a measured trade: prior steady cutoff p50 was
  281 ms, and this body compile takes 2.978 s against the earlier 2.541 s.
  The new compiler-source prime costs 6.378 s and is recorded separately.
  Emitted output matches CPython, with the same artifact size and digest as the
  previous source-pair result. These observations prove removal of a redundant
  post-edit build, not faster body compilation or the 100 ms/one-second targets.
  Other host/backend cells and WASM performance remain unverified.
- Recording adds bounded receipt/lifecycle state, serialized worker start/end
  events and consumed-byte facts, plus source hashing at publication/replay.
  Replay remains linear in recorded paths plus consumed source bytes; it hashes
  no additional tool/artifact bytes solely for this exception. Explicitly retained
  resolution caches now retain raw source bytes as well as decoded text. Temporary
  snapshots remain collectible. No new process, optimizer, disk cache or
  generated-program work is introduced.
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

Rust source, job counts, output paths and unrelated Cargo
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
   policy. Rust artifacts have no build-Python dependency; explicit source
   generators and differential oracles retain development-side Python custody.
   Unicode generation consumes every pinned CPython source cohort member,
   including pending versions, independently of guest target admission. Pure
   descriptors use complete `(major, minor)` coordinates; runtime and C-API
   consumers select only registered guest targets. Missing C-API providers and
   unsupported targets report the existing `SystemError`, while out-of-range
   code points retain their false predicate result. Source facts do not admit a
   guest version or establish target qualification.
5. A directory the build lists records the projection it used (its module
   candidates), not its token, so a new log or editor file beside the entry
   does not invalidate the record; Bazel's `GlobValue` cuts off the same way.
6. Recent source edits now record through consumed-byte evidence. Remaining
   weak metadata inputs keep the conservative window until their actual
   consuming authority can prove a stronger contract; filesystem precision alone
   does not justify relaxing it.
