# Cargo input authority

Status: file coverage guard (native runtime, backend compiler and JIT runner
publication), crate-declared build inputs and observed Cargo environment inputs
implemented (HF-300, HF-302, HF-371). Replacing guessed file roots with the
complete content record remains open. Owner: build speed and artifact identity.

## Problem

Molt builds four artifacts with Cargo: the native runtime staticlib, the WASM
runtime pair, the WASM CPython-ABI staticlib and the backend compiler (the proof
supervisor is a fifth). The standalone browser SIMD producer
(`tools/build_browser_simd.py`) also reuses the Cargo transcript boundary, but
publishes a static asset without a compiler/runtime generation receipt. Its
ordinary Cargo Fresh behavior cannot establish consumed-byte attestation either;
include it when applying the complete file-input policy to source-built assets.
To decide whether a stored compiler/runtime build is current,
Molt hashes every file under the crate roots it guesses from the Cargo
manifests (`cargo_source_closure._crate_source_paths`), plus the files outside
each crate that its build reads. The guess was wrong in both directions, and
slow:

- **It missed inputs.** The native runtime identity omitted `include/molt/molt.h` and the
  23 `include/molt/shared` headers that build scripts compile the C API shims
  against, `runtime/wasm_table_layout.inc`, and four sibling-crate files that
  `molt-runtime/src/builtins/mod.rs` compiles through `#[path]` in lanes whose
  feature closure leaves those crates out. These files now belong to the
  canonical source roots, so their changes invalidate that identity.
- **It counts products.** Tools can write build products inside crate roots.
  The walk includes those products, caches and publication locks even when
  Cargo did not read them. Content hashing scales with their total bytes.
- **Its environment is a fixed allowlist.** It misses variables that change the
  bytes Cargo builds: `COMPILER_PATH` and `GCC_EXEC_PREFIX` (declared by the
  `cc` crate in every native lane), and, by the review, `LIBSQLITE3_FLAGS`,
  `SQLITE_MAX_*`, `LZMA_API_STATIC`, `ISLE_SOURCE_DIR` and objc2's
  `UNSAFE_OBJC2_ALLOW_CLASS_OVERRIDE` (HF-302).

## Prior art

- **Cargo** fingerprints each unit from rustc's dep-info (files and
  `# env-dep:` variables) and the build script's `rerun-if-changed` and
  `rerun-if-env-changed` lines, compared by mtime. A build script that prints
  neither reruns when any file of its package changes. A directory named by
  `rerun-if-changed` is watched recursively, its own mtimes included.
  ([fingerprint](https://doc.rust-lang.org/stable/nightly-rustc/cargo/compiler/fingerprint/index.html),
  [build scripts](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rerun-if-changed))
- **Ninja** reads the depfile the compiler wrote during the previous build into
  its deps log and decides by stat alone.
- **Bazel** keys each action by the digests of its inputs, caches file digests
  by (path, size, mtime, inode, ctime), and rejects an undeclared inclusion that
  the compiler's `.d` output reveals.
- **Git** keeps a stat cache in its index; a file changed within one timestamp
  tick of the index write is rehashed (the racy-git rule).
- **rustc** incremental verification (`-Zincremental-verify-ich`) recomputes a
  reused result and fails on a mismatch.
  Its [red-green query algorithm](https://rustc-dev-guide.rust-lang.org/queries/incremental-compilation-in-detail.html#improving-accuracy-the-red-green-algorithm)
  separates an input change from an observable result change, preserving
  downstream results when recomputation gives the same fingerprint.
- **Bazel Skyframe** records dependencies through its evaluation API. Its
  [hermeticity and invalidation rules](https://bazel.build/reference/skyframe)
  explain why an unrecorded read invalidates incremental correctness, and why
  change pruning needs equal recomputed values.

The useful common rule is explicit dependencies and conservative reuse when
those dependencies cannot be established. Molt must preserve the distinction
between input observations, a reusable result and authenticated release evidence.

## Design

### What a build read (`molt.cli.cargo_build_reads`)

The authority is the build's own per-unit outputs, which Cargo reports in its
JSON messages:

1. For each local `compiler-artifact` unit, the rustc dep-info beside its
   outputs (`<crate>.d` or `<crate>-<metadata>.d`): its files and its
   `# env-dep:` variables.
   A build script's unit lives in `build/<package>-<metadata>/`. For a root
   artifact reported only uplifted, the reader binds a candidate's populated
   rustc output rules to the primary outputs in Cargo's
   [`compiler-artifact` message](https://doc.rust-lang.org/cargo/reference/external-tools.html#compiler-artifact).
   Several feature or profile units can share `OUT_DIR`; its recorded value
   narrows candidates when present. Actual output identity establishes the
   binding, including units that do not read `OUT_DIR`. Unhashed and hashed
   records share this candidate pool and the same output/ambiguity checks;
   an unhashed name alone does not establish identity. The combined WASM
   producer reports final archives/modules under the target root while its
   unhashed per-unit dep-info lives in the resource-selected build directory.
2. For each `build-script-executed` message, the script's `output` file beside
   its `out_dir`: the `rerun-if-changed` paths (relative to the package root)
   and the `rerun-if-env-changed` names. A script that declares neither counts
   its whole package, including an obligation to cover future additions there.
3. Registry and git units currently contribute their variables; Cargo.lock
   identifies the requested source archives, not the bytes of a mutable unpacked
   source tree. Complete file attestation must bind those consumed bytes too.

The uplifted dep-info beside a final artifact is not enough: Cargo's
`output_depinfo` recurses only through local dependencies, so a local package
that only registry crates reach (`vendor/rust/zlib-rs`) is missing, and it holds
no variables.

Paths under the build's target and build directories are outputs of build
scripts; paths under a toolchain root the plan identity covers are left to it.
Any other path outside the workspace means the build read another workspace's
sources (the wrong-source execution V1-01 reproduced with shared Cargo
intermediates), and it is refused. The dep-info grammar is Cargo's: only spaces
are escaped, and a token that ends in a backslash joins the next one.
[Cargo's parser](https://doc.rust-lang.org/stable/nightly-rustc/src/cargo/compiler/fingerprint/dep_info.rs.html#386-460)
decodes backslash, newline and carriage-return escapes in both environment
names and values; malformed escapes and dangling dependency joins refuse
publication. Candidate selection uses the same decoder. File URI authorities
remain part of UNC package roots. Build-script lines use Cargo's whitespace
trimming before directive recognition.

### Declared build inputs

A crate declares the files outside its root that its build script,
`include_str!` or `#[path]` modules read, in `[package.metadata.molt]
build-inputs`, relative to the crate as `cargo:rerun-if-changed` paths are.
The closure walker adds them for every crate it reaches, so the fact lives
beside the code that reads it and every product that builds the crate keys it.
Before this, the extras were hand-listed per product, and the JIT runner's key
missed `src/molt/backend_environment.json`, which `molt-ir`'s build embeds
(HF-371 review). A declared input that does not exist, or that escapes the
source root, is refused. Only the workspace's `Cargo.toml` and `Cargo.lock`
stay caller inputs.

### The coverage guard

Native runtime, backend compiler and JIT runner publication refuse a build with
a file or watched directory outside their identity roots
(`cargo_build_reads.uncovered_build_inputs`); the refusal names the paths and
the declaration that would cover them. Covering only a directory's present files is
insufficient: adding its first header could otherwise leave the identity
unchanged. The parser is itself part of the runtime tooling identity.

Admission and publication normalize their declared roots at the existing
`realpath` boundary, then build one operation-local immutable hash index.
Coverage checks the input itself and each ancestor against that index, using
the host's `Path` equality and hashing for drive/case semantics. It preserves
exact-root, descendant and watched-directory coverage, rejects sibling prefixes,
and adds no filesystem observation, process spawn or persistent cache. Every
Cargo publication consumer uses this same coverage owner.

For N observed inputs, R roots and maximum path depth D, the index stores R root
entries and queries perform at most N(D+1) hash-membership probes, instead of
N times R path-containment checks. Normalization and initial root hashing still
scale with root path bytes. Each query constructs and hashes at most D ancestor
paths, so its copying/hashing cost includes their component and byte lengths;
the probe count is not a linear-time claim for arbitrary path lengths. A fixed
1,000-input/256-root fixture with half its paths
uncovered took median 2.336 seconds before and 25.654 milliseconds after over
three uninstrumented observations per revision on macOS arm64/CPython 3.12.15.
Traced peak memory rose from 179,108 to 194,528 bytes in separate observations.
These are helper costs, not a full-build speedup or platform qualification.
Independent operation-count controls reject the former root scan in both read
admission and publication without timing thresholds. The original Cargo
transcript, watched-directory, publication refusal and source-stability oracles
remain in force; this changes membership lookup, not what the build must prove.

The existing native build owner has these transitions:

```text
planned -> Cargo completed -> input coverage established -> published -> admitted
                    |                   |                     |
                    +-------------------+---------------------+-> failure
```

Coverage failure cannot reach generation publication. Publication retains its
existing input-stability fence and atomic generation owner. Adoption validates
the current source/tooling identity; there is no second cache registry or new
guest check. The guard runs only after Cargo and reads bounded dep-info and
build-script outputs under the existing stable direct-file reader. Populated
rules must agree on their source reads. Directory declarations also enumerate
their current files.

For uplifted outputs, owned open handles establish same-file identity without
hashing. Copied outputs first require equal size, then the existing streamed
SHA-256 identity; Cargo's final output stays open throughout selection, and its
digest is computed at most once per selection. Every primary Cargo filename must
have a populated rustc output rule of its role with matching identity. An absent
linked-output rule cannot bind through matching metadata alone. External debug
and DLL import/export files follow
[Cargo's auxiliary/debug classifications](https://doc.rust-lang.org/stable/nightly-rustc/src/cargo/compiler/build_context/target_info.rs.html#428-553);
dep-info itself cannot bind a unit. Binary targets use Cargo's explicit
`executable` field, so dotted executable names keep their primary role. These
reads use the shared POSIX change-time fences and Windows write/delete exclusion.
Missing retired candidate outputs do not
match; unreadable, indirect or changing evidence refuses publication.

Several output-equivalent candidates are safe only when their normalized source
path sets and exact environment facts agree. The sorted dep-info pathname then
selects deterministic evidence for that single read closure. Different closures
remain ambiguous even when their output bytes happen to be equal. A lone
`OUT_DIR` match still needs positive output identity.

The cold copied-output cost is one final-artifact hash plus one hash for each
same-size candidate output, with bounded streaming buffers and no persistent
artifact digest cache. Different-size candidates cost metadata reads only. Direct
per-unit paths and same-file outputs need no content hash. This runs during Cargo
materialization; adoption of an already published generation and emitted guest
programs acquire no additional work. Its cost scales with inspected records,
compared artifact bytes and watched directory contents.

The file coverage guard covers native runtime publication; it does not
establish actual used bytes or release acceptance. Environment observation is a
separate part of the same reader, used by every source Cargo artifact owner.
The canonical tree identity includes directory names from its existing traversal,
so even an empty subdirectory addition invalidates the recorded source tree.
This adds directory labels to the index, with no extra traversal or file hash.
Cross-platform and native stdlib-lane execution qualification remains separate;
current bounded checks and remaining target cells belong to the V1 findings ledger.

### Observed environment inputs

`CargoEnvironmentInputs` owns the immutable observation. It records the host's
`NameRule`, a positive local-unit count and sorted unique variable names with
SHA-256 value digests; absence is `null`, distinct from the digest of an empty
value. Values never enter the receipt in plaintext. Windows names use the shared
nonexpanding fold; POSIX names remain exact. The invocation owner passes the
frozen effective `ProcessEnvironment`, after Cargo configuration and execution
policy, to both Cargo and the reader. No ambient environment default is allowed.

The reader requires Cargo's complete successful JSON transcript, including its
terminal `build-finished` message. Cargo JSON, dep-info and script readers use
LF/CRLF boundaries, preserving literal Unicode separators inside data.
Malformed JSON candidates, duplicate terminal
messages and records after completion refuse before publication; tool text is
not mistaken for protocol data. The execution boundary retains full stdout,
separately from bounded diagnostic excerpts.

The reader preserves unit ownership before collecting ambient names. A
`rerun-if-env-changed` declaration observes the Cargo invocation environment,
even when a script emits `rustc-env` for the same name. Rustc's generated values
come from that package's `build-script-executed.env` and actual `OUT_DIR`.
An `OUT_DIR` read selects its corresponding script context; conflicting relevant
contexts refuse publication. Cargo's closed crate-variable set is derived from
the unit's manifest and target metadata. Unknown `CARGO_*` names remain ordinary
ambient dependencies. Missing dep-info, stale ambient facts and ambiguous or
unproven unit-owned values refuse with package, target and variable diagnostics.
There is no blanket prefix exclusion. Command-injected variables that need
unavailable ownership context, including `CARGO_PRIMARY_PACKAGE` and
`CARGO_BIN_EXE_*`, are not claimed until that context is established.

These rules follow Cargo's distinction between
[invocation environment changes](https://doc.rust-lang.org/cargo/reference/build-scripts.html#rerun-if-env-changed),
[crate environment variables](https://doc.rust-lang.org/cargo/reference/environment-variables.html#environment-variables-cargo-sets-for-crates)
and [generated build-script output](https://doc.rust-lang.org/cargo/reference/external-tools.html#build-script-output).
The command owner also supplies the dep-info base explicitly: the repository
workspace for runtime/backend and the isolated supervisor workspace for its
crate. The allowed source fence can be wider than that relative-path base.

The one observation is embedded in each existing result receipt: the native
link manifest, WASM pair generation, CPython-ABI/backend artifact fingerprint,
and supervisor generation inputs. Native selection binds its manifest by hash;
codegen carries that original archive and manifest through final link. WASM and
backend source hydration, selected and raw-output reuse, transfer and final
admission verify the exact carried observation under the current frozen inputs.
A mismatch enters the normal explicit build/no-build policy. Metadata refresh
preserves the admitted observation and cannot replace or erase it. Missing or
older observation schemas never authorize source reuse.

A plan identity describes how to build; this result record describes what was
read. A changed environment need not change output bytes. Application cache
identity remains derived from admitted result bytes, following rustc's red-green
principle, rather than acquiring an environment digest that invalidates equal
results. Installed artifacts and historical/transport validation check the
record's shape and member binding without consulting a consumer's ambient
variables or requiring Cargo or a source checkout.

Capture runs once after successful Cargo execution, using the reader's existing
unit evidence. Warm verification hashes only the recorded present values:
O(names + value bytes), no process spawn, directory walk, artifact copy or new
sidecar. Build scripts and compiler tooling bear this cost; emitted user binaries
perform no observation or verification work. This is a correctness and cost
contract, not a measured Molt build speedup. Platform execution and release
qualification remain explicit in the V1 findings ledger.

### Complete file input record (remaining work)


The record replaces the guessed roots. It holds, for one build of one
artifact:

- every file the build read, with size and SHA-256;
- each watched directory's sorted recursive listing, so an added header that
  shadows another is seen;
- the implemented environment observation described above;
- a lower bound the plan supplies, not the record: Cargo.lock, the root
  `Cargo.toml` (workspace edition, inherited features, `[patch]`), every local
  package's manifest and at least one dep-info file per local unit. An empty
  record is refused.

A stored generation is current when its plan identity equals the current one
and every recorded input re-verifies now, through the host's stat-keyed digest
cache (`toolchain_identity.FileDigestCache`). The receipt names which files and
variables to check; it never vouches for their bytes. This is a proposed cost
reduction, not a measured implementation win. Windows must retain content
verification where its metadata does not support the existing cache contract.

The migration must establish these prerequisites before replacing tree identities:

- **Per-unit provenance.** Preserve the bound dep-info path, package/target/output
  identity, Cargo's `fresh` fact and build-script directory declarations before
  flattening paths. The current reader discards these facts. Require a bound
  record for each actual unit and the selected root target. The manifest graph
  supplies a conservative local-package lower bound; declared optional, dev and
  target dependencies are not proof that Cargo compiled those units.
- **Consumed bytes, including bootstrap.** Hashing current sources after Cargo
  succeeds cannot attest bytes consumed by a Fresh artifact. A prior trusted
  per-unit content record must match the current inputs and output; without one,
  bootstrap reuse refuses or needs proven rebuilding. A change-time predating
  the build does not close that gap. Cargo's mtime freshness is distinct from a
  content proof; see [racy Git](https://www.kernel.org/pub/software/scm/git/docs/technical/racy-git.html).
  `MOLT_CARGO_INPUT_*` currently forces the proof supervisor and artifact publisher,
  not every runtime, backend and transitive unit. It cannot be assumed to force
  arbitrary Fresh units. Nightly checksum freshness is not a stable toolchain
  contract.
- **Complete source custody.** Bind actual registry/git source bytes or establish
  authenticated immutable source custody; Cargo.lock alone does not attest a
  modified unpacked cache. Preserve watched empty directories and recursive
  listings. Apply lexical alias fences before resolving paths, since `realpath`
  erases the evidence. Retain stable-file handle and host-specific digest-cache
  protections rather than adding a second registry or a weak Windows stat cache.
- **A stable build interval.** A post-build Git inventory proves current source
  bytes, not which bytes a previously Fresh unit consumed. A frozen checkpoint
  or equivalent proven input-stability boundary must cover compilation and
  publication; a release inventory cannot substitute for that boundary.

These are remaining requirements, not implemented guarantees. The existing
coverage guard and environment observation remain in force while the complete
producer, adopter, cached verifier and release replay migration is open.

`MOLT_VERIFY_CARGO_INPUTS=1` hashes every recorded input on each check and
fails on a stat-cache disagreement, as rustc's `-Zincremental-verify-ich` does.

## Retired by the record

- The crate-root walk as an identity source (`_crate_source_paths`,
  `runtime_source_roots`, the source part of `RuntimeTreeIndex` captures) and
  the crate-declared build inputs.
- The backend source walks and the walk inside `CompilerSourceGeneration`.
The fixed build-script environment allowlists are already retired by the
observed environment migration. Typed Cargo/rustc, C-tool and configuration
selectors remain with their existing plan owners.

Link fingerprints walk link inputs, which are artifacts; they are unchanged.

## Migration

Each slice is one contract, migrated whole with its consumers and tests:

1. **Coverage guard (implemented)**: `cargo_build_reads`, native runtime,
   backend compiler and JIT runner publication, and crate-declared build
   inputs in place of the hand-listed extras.
2. **Environment observation (implemented).** Native runtime, WASM pair,
   CPython ABI, backend and proof supervisor produce and consume the same typed
   record in their existing receipts. Source admission re-verifies; installed
   admission does not consult source inputs. Codegen pins receipt facts through
   final link. No second optimizer, cache registry or end-user check is added.
3. **Complete file record (open).** Replace the guessed file roots across all
   these producers and consumers, preserving coverage, input-stability fences,
   no-build policy, transfer, installed and release contracts. The runtime
   family/member identities, backend compiler identity, output pipeline, phase
   timing, cache layout and release source-tree verification must migrate as one
   contract. The record does not supply its own coverage proof, and changed
   inputs producing equal bytes must preserve downstream output identity.

## Development fixture cost

Custody-only tool fixtures use independent copies of one small native image,
owned lazily by the pytest session through `tests/executable_test_support.py`.
They retain the real executable and SDK identity validators; tests of tool
behavior still supply actual tools or explicit subprocess protocol fixtures.
An existing-only or empty image batch never requests the donor, and mutating
one copy must leave both its siblings and the donor unchanged. The lightweight
session binder does not import runtime construction or launch a compiler.

The measured macOS arm64 trade is one 93 ms guarded Rust invocation to build a
462,024-byte image instead of repeatedly copying and hashing the 18,084,128-byte
interpreter. Across three alternating observations per input, synthetic SDK
construction (seven independent images, receipt construction and real install
validation) took medians of 6.9 ms and 56.8 ms respectively. Those are fixture
costs, not compiler or guest performance results. Windows uses its existing
small native interpreter launcher; Linux uses the same lazy compiled donor
policy. Platform execution qualification remains explicit. No fixture code or
development verification enters emitted programs.

## Review history

2026-10-17, adversarial review of the first draft, which read the uplifted
dep-info beside the final artifact. It found: local packages that only registry
crates reach are missing there; environment inputs are missing everywhere;
Cargo's mtime freshness can disagree with a content record; a record must not
choose its own coverage; directories need their listings; the root manifest is
an input; racy rows make release identities timing-dependent; and the
make-style grammar misreads Cargo's on Windows. The design above answers each.
Real builds then found what the drafts had not: Cargo reports a build script
and a root staticlib only as uplifted copies, and molt-runtime compiles
sibling-crate files through `#[path]`.
