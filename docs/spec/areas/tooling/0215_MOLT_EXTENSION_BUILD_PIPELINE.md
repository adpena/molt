# Molt Extension Build Pipeline
**Spec ID:** 0215
**Status:** Partial (cross-target + verify/publish policy integration +
build-admission sidecar custody and deterministic build-artifact publication
for admitted external packages, runtime load-time metadata enforcement + CI
native/cross-host matrix lanes landed, including verify-policy and
wasm static-link artifact contract checks)
**Owner:** tooling + runtime
**Goal:** Define the build, packaging, and validation pipeline for C-extensions
recompiled against `libmolt`.

---

## 1. Principles
- `libmolt` is the primary C-extension compatibility path.
- Extensions must be recompiled; no CPython ABI compatibility.
- Capability and determinism policies apply at build and load time.
- Build outputs are reproducible and verifiable.

---

## 2. CLI Surface
### 2.1 `molt extension build`
Purpose: compile a C-extension against `libmolt` and emit a Molt-compatible wheel.

Status: Implemented (initial).

Flags (implemented):
- `--project <path>` (default: cwd)
- `--out-dir <path>` (default: `dist/`)
- `--molt-abi <ver>` (default: `[tool.molt.extension].molt_c_api_version` or `MOLT_C_API_VERSION`)
- `--target <native|wasm|wasm-freestanding|triple>` selects the typed target
  plan. Native targets emit deterministic `.molt.a` static archives; wasm32
  targets emit relocatable `.molt.wasm` objects.
- The inner target resolver requires an explicit nonblank target; only the CLI
  boundary defaults an omitted target to `native`. Native host triples and
  object-format policy share `native_link_plan`; WASM source extensions admit
  only `wasm32-wasip1` and `wasm32-unknown-unknown`. This policy classifies
  artifacts, not an assertion of compiled OS/architecture/ABI conformance.
- Recorded native target facts are checked against the artifact's exact triple,
  never the inspecting host. Native linking requires exact extension target
  identity even when no cross-target option was supplied; matching object
  formats alone cannot establish architecture or ABI compatibility.
- `--capabilities <file|list|profiles>` (override extension capability metadata)
- `--deterministic/--no-deterministic`
- `--json` / `--verbose`

Compiler commands share one typed driver/language authority. MSVC-ABI targets
use the clang-cl C/C++ driver family; incompatible explicit drivers are rejected
before configuration. GNU-style native and WASM drivers retain their own argument
grammar. Compile emission, dependency files, replay normalization, deterministic
path maps and persisted language validation consume the same dialect selection.
Replay replaces all compiler-owned target, SDK, dependency and side-output
selectors, including forwarded frontend selectors, while preserving language,
runtime-library and optimization semantics. Forced headers resolve relative to
the recorded compilation directory. Precompiled-header/module inputs require
explicit custody and are rejected until that capability is implemented.

Outputs:
- `.whl` tagged with `py3-molt_abi<major>-<platform_tag>`.
- `extension_manifest.json` sidecar (ABI/capability metadata + checksums).
- For `--target wasm`/`wasm32-*`, a standalone `.molt.wasm` static-link
  artifact whose sidecar declares `runtime_linkage = "static_link"`,
  `artifact_kind = "wasm_relocatable_object"`, and `object_closure` symbol
  custody.
- For native targets, a deterministic `.molt.a` static archive whose sidecar
  declares `runtime_linkage = "static_link"`, `artifact_kind = "static_archive"`,
  the exact target triple, object closure, and explicit link requirements.

Native symbol evidence has one typed reader in `cli/backend_cache.py`, shared
by application caches, shared-stdlib closure, extension object inspection, and
external providers. Missing tools, failed reads, malformed output, and partial
archive inspection are errors, never empty symbol tables or reusable negative
facts. Diagnostics identify the artifact and bounded tool-attempt details;
extension builds return them through the normal text/JSON error surface without
publishing a wheel or manifest. Successfully inspected empty symbol tables remain
distinct from unavailable evidence. Weak undefined symbols are not providers or
required strong dependencies. Symbol normalization follows the artifact target,
not the inspecting host; persisted facts and validation tokens bind that target
and the versioned symbol contract.

### 2.2 `molt extension audit`
Purpose: verify that an extension declares capabilities and matches the expected ABI.

Status: Implemented (initial).

Flags (implemented):
- `--path <wheel|manifest|dir>`
- `--require-capabilities`
- `--require-abi <ver>`
- `--require-checksum`
- `--require-loader-kind <token>`
- `--require-runtime-linkage <token>`
- `--require-artifact-kind <token>`
- `--require-artifact-file`
- `--require-object-closure`
- `--require-python-export <dotted-name>` (repeatable)
- `--require-callable-export <dotted-name>` (repeatable)
- `--json` / `--verbose`

### 2.3 `molt extension produce-set`
Purpose: reproduce a configured, package-owned extension set from one upstream
Meson graph and atomically publish its complete sealed package root.

Status: Implemented for verified scientific-stack sets.

Flags:
- `--package <name>` and `--module-set <name>` select a typed set from
  `config/scientific_stack_versions.toml`.
- `--source <path>` must be the exact configured upstream commit; recursive
  submodules are initialized at their pinned commits and then verified.
- `--build-root <path>` must be absent or empty, preventing metadata from a
  prior Meson configuration from entering the transaction.
- `--target native|wasm|wasm-freestanding|<triple>` and
  `--abi-tier cpython-abi` select a complete CPython/ABI/target variant.
- `--json` emits the machine-readable publication result.

Each configured set names one project dependency group. Before acquiring the
package publication lock, the producer derives an immutable environment address
from the dependency-group name and ordered requirements, the complete
`uv.lock` digest, the base-Python identity, the uv executable identity, and the
provisioning-schema version. It
serializes provisioning at that address, runs `uv sync --frozen
--no-default-groups --group <group> --no-install-project` directly at the final
address after atomically recording exact provisioning intent in sibling custody
outside uv's mutable environment directory. The
resolved distributions and declared requirements are verified before an atomic
manifest replacement publishes the complete attestation. An exact provisional
record alone authorizes recovery of an unattested partial root under the same
address lock; malformed, unrecorded, or final-attested roots are never mutated.
The sibling record is removed only after publication; a crash between those
steps accepts the complete root and removes the exact stale record. This
attestation-atomic protocol avoids
relocating uv environments whose Windows console launchers bind their creation
path. An ambient `.venv` is never accepted or mutated. The
producer transparently re-executes its typed command in safe-path mode under
the attested Python, with `PYTHONPATH` replaced by exactly the invoking
worktree's `src`, user-site/PYTHONHOME injection disabled, and the attested
environment's `Scripts`/`bin` directory first on executable `PATH`; the
environment itself therefore has no editable-worktree identity and is reusable
by sibling worktrees.

Target metadata binds both Meson machine files: `meson.cross` for the extension
target and `meson.native` for programs executed on the build machine. Both use
the same tool resolver and content-attested family; a native build reuses its
resolved family, while a cross build records a separate native-machine family.
Both machine files bind C and C++ explicitly; a missing role is an admission error,
not permission for Meson to rediscover a compiler. Direct C-only extension builds
outside the Meson metadata surface do not require an unused C++ compiler.
Meson never selects build-machine compilers from ambient `CC`/`CXX` or `PATH`.
Both machine files bind C/C++ compile and link argument arrays, so ambient
Meson flag variables cannot supply another machine configuration. Package setup
arguments may select project options or build type, not replace machine files,
compiler/linker arguments, install prefix or dependency search paths.
Configuration, tool probes, generators and object replay remove implicit command
overrides (`CL`, `_CL_`, `CCC_OVERRIDE_OPTIONS`, and Meson `CC_LD`/`CXX_LD`, including
build-machine variants) from child environments. SDK search inputs such as
`INCLUDE` and `LIB` remain available; this does not claim hermetic SDK custody.
Schema v4 includes both file digests and machine identities in the canonical
sidecar and package seal. Seal validation reconstructs both machine files from
the same command/path projection used by the producer and requires canonical
UTF-8/LF bytes; rehashing an inconsistent machine file cannot admit it.
Historical metadata without build-machine custody must
be reproduced; it is not silently upgraded or admitted as current evidence.

One invocation performs one real Meson setup, consumes the unchanged
`intro-targets.json`, `compile_commands.json`, `intro-installed.json`, and Ninja
generator commands, builds every configured module deterministically through
`molt extension build`, audits its ABI/artifact/object/export custody, stages
Meson's real installed Python files, and publishes only after the exact set is
complete. The destination is version-keyed under
the canonical Molt custody root at
`package-seals/<package>/<version>/variants/cpython-<version>/<abi-tier>/`
`<target-triple>/<seal-name>`. The set manifest independently attests the same
CPython/ABI/target coordinate, and every extension sidecar contains an explicit
link-requirement object even when its argument/input lists are empty. Publication
uses same-volume exclusive directory installation. Replacements require the
expected incumbent identity; the producer also pins its observed seal digest
under the destination lock before building. The shared publication journal binds
both incumbent and candidate seal/identity digests. Failures after retirement
restore the exact incumbent and preserve quarantined candidate evidence; partial
sets are never published. A replacement can temporarily leave the canonical name
absent between retirement and installation; this is not an atomic directory swap.

### 2.4 Detached candidate attestation and registered promotion

`molt extension attest-set-candidate` uses the same build arguments and build
implementation as `produce-set`, plus `--output <path>`. Output must be under the
canonical `package-candidates` root, disjoint from source/build roots and outside
`package-seals`. It needs a registered package-set contract, but not a registered
candidate identity. It never acquires canonical publication custody. Its complete
bundle contains only `candidate-seal/` and `candidate-attestation.json`; the report
is recomputable from the sealed bytes and records that publication was not performed.

After reviewing and registering that identity, run
`molt extension publish-set-candidate --candidate <bundle>`. This command has no
source/build/toolchain arguments and never rebuilds or reexecutes the producer.
It verifies the report and exact candidate against the current registry before
acquiring the destination lock. Replacing an incumbent requires both
`--expected-incumbent-seal-sha256` and `--expected-incumbent-identity-sha256`.
Only identical seal **and** canonical identity are a no-op; a changed seal with
the same semantic identity still installs the exact admitted candidate.

`source_extension_set_validation` owns one pipeline: recorded structural facts,
immutable receipt, then current-registry admission. Historical incumbents use the
same structural checks with explicitly pinned hashes, not today's package build
contract. Identity computation consumes typed snapshots and performs no filesystem
discovery. Relocation rebinds receipts only after verifying the identical seal
hash and inventory; an immutable receipt does not make its backing filesystem immutable.
Sealed support entries contain only destination paths and checksums bound to that
inventory; producer-side source remapping is not admitted. Execution metadata is
parsed once for both content identity and direct-function export checks. Artifact
inspection is enclosed by stable file/change-time custody, and WASM symbol facts
are bound to the exact inspected bytes, including when content is restored after
a transient mutation. Native and WASM object/archive symbol-fact caches also bind
the ordered reader commands and executable content generations; every invocation
fences the lexical entrypoint and resolved executable before and after execution.
One shared parsing-protocol identity also binds both cache forms. GNU/BSD
`archive(member)` and LLVM `archive:member` empty-member diagnostics are matched
against the exact input before parsing delimiters, including Windows drive paths.
Successful archives may contain empty members; unknown diagnostics, malformed
members and nonzero partial symbol output never become complete symbol evidence.

Candidate journals survive bundle installation and are completed or recovered
under the candidate-name lock. Shared publication recovery handles both producer
and promotion transactions. Terminal aborted records are historical evidence, not
perpetual authority over future canonical contents. Unjournaled transaction roots
are preserved with an explicit review diagnostic, never deleted by name alone.
The `molt.file_publication` owner supplies cross-platform file/directory barriers,
exclusive installation and quarantine moves for CLI, package-seal and proof-CAS
consumers. Post-commit durability/cleanup failures are reported without claiming
that a successful namespace commit rolled back.

Physical retirement uses that same authority: an exclusive same-parent rename
removes the live leaf before any recursive reclamation can destroy its journal.
The bounded retired name encodes the exact caller scope and no-follow root
device/file/type identity; it is not a second deletion journal or registry.
Candidate recovery reclaims only its attestation scope under the candidate lock;
publication recovery reclaims only its destination's produce/promote scopes
under the publication lock, before reading live journals. Private package-store
staging/copy and recorded commit candidates use the same primitive and replay.
Recovery touches only already-retired identities, never a new live generation at
the former spelling. No unrelated scope, malformed name, indirect root, special
entry, zero identity or replaced identity grants reclamation authority.

Windows uses the existing no-replace write-through rename; Linux/macOS use their
no-replace rename followed by a same-parent durability barrier. Unsupported
platforms fail closed rather than emulating exclusive rename. Reclamation
cannot begin before that barrier succeeds. A later deletion/barrier failure
raises `RetirementError` with `namespace_committed`, `phase`, and
`retired_path`; direct retirement also identifies its former `source_path`.
Callers report committed retirement and retain scoped retry custody, not a
preserved live transaction or fictitious rollback. The next scoped recovery
retries the parent barrier and physical reclamation without loading a partially
deleted journal. Package scratch finally blocks retain both primary publication
failure and secondary retirement failure. A tombstone may remain after a crash
until its owning recovery next runs; successful recovery leaves no tombstone.
Caller locks/private namespace ownership are required throughout; encoded
identity detects substitution but is not permission to mutate an unowned parent.
Producer success is emitted only after the shared publication-transaction
completion consumer returns. A cleanup failure returns structured non-success
with `publication_committed=true` and `cleanup_complete=false`, plus the
retirement phase/path for the failing residue. The current transaction's
`namespace_retirement_committed` is true only when the exception's direct
`source_path` matches that transaction; prior-residue failures leave it false. It cannot print an earlier
ok result and then reduce cleanup failure to a finally warning. The outer finally
only releases its existing lock; it neither retries physical cleanup nor
overwrites primary failure evidence. Promotion and recovery use the same
publication completion scope authority. Promotion reports `publication_committed`
as false before entering its publication call, null when that call fails with an
unknown outcome requiring recovery, and true immediately when it returns
successfully, including subsequent verification/rebinding/cleanup failures. An
unknown outcome retains its transaction-root recovery pointer; path existence
does not decide whether publication committed.

These artifact/custody checks do not establish compiled native/WASM conformance:
execution claims still require replayable target/version/OS/architecture receipts.

### Selected-Python content authority

`molt.python_environment_identity` is the shared isolated probe for runtime builds,
source-build provisioning and proof execution. Its runtime/environment validators
own portable file-node, import-root, distribution and loaded-native-dependency
identity. Source-build recipe schema 5 binds the selected uv-lock group closure,
CPython runtime closure and realized environment. The lock closure includes root
project dependencies as well as the selected group, matching uv's actual sync
semantics. Marker-selected dependency and extras activation use a least fixed
point; receipt validation rejects unreachable packages and ungrounded extras.
Unrelated lock groups do not change that recipe. The runtime-build outer v2
schema is unchanged by this cut.

The path-only locator runs before proof watches are armed. Native files outside
environment roots receive exact-file watches, never broad system-directory watches.
Runtime closure v5, PE/ELF loaded-dependency policies v3 and Mach-O policy v4
attest all observed native file components and loader-provided virtual contracts,
not just runtime-root reachability. PE eager imports, ELF `DT_NEEDED` (including
lazy symbol binding), and Mach-O required/reexport/upward loads remain mandatory.
PE delay and Mach-O weak/lazy declarations are recorded separately with their
importer and kind; a matching loaded basename never proves that importer's
optional binding. Components use resolved full-path identity on every OS;
configured launchers remain content-bound roots but cannot impersonate loaded
importers or providers, even with equal basenames. Observed PE/ELF loader names
must be unambiguous; distinct macOS framework images may share a `Python`
basename. The OS-loaded executable designation is an observed component bound
into the closure digest, not inferred from the configured base launcher.
Direct, `@loader_path`,
`@executable_path`, and importer-local `LC_RPATH` bindings resolve only to the
observed file-object census. Inherited dyld run-path stacks are not inferred and
fail closed. Dyld shared-cache contracts carry canonical absolute install paths;
an equal basename at a different path cannot satisfy a dependency. Optional
targets already in the census retain independent byte custody, without invented
dependency edges. Components are ordered by loader filename and file-node index,
preserving distinct file components without a second identity authority. The
parser fences capture with a second loader census and retains that fence through
outer custody publication.
This is a snapshot of observed files and declarations, not an attestation of
future dynamic loads: later files require renewed admission and exact watches.
After capture, every absolute file in the custody envelope must be covered; only
then may the full child-executable policy be bound and payload execution begin.
Capture timing and worker telemetry are queryable but excluded from semantic hashes.
Relative portable nodes do not replace the absolute frozen-file custody used for
replay and final rehashing. Capture v2 binds each semantic file-node JSON pointer
to a zero-based index in the sorted absolute-file inventory. Multiple authorities
can reference one physical generation without duplicated hashing or ambiguous
content-count inference; distinct nodes inside one inventory cannot collapse to
one custody index. Absolute-path validation uses the path's own Windows/POSIX
grammar, independent of the receipt reader's OS. Windows device namespaces,
reserved names and Win32-stripped spelling aliases are outside that grammar.
Unicode-normalized semantic paths resolve through collision-checked host names;
normalization never rewrites physical lookup paths. Complete node/package
inventories live in the CAS;
compact receipts carry digests, counts and policy-required external-source facts.
Failed capture drains watches without claiming that a payload ran.

`PythonFileCaptureContext` shares handle/change-time-bound hashes across runtime
and environment inventories. Hashing has bounded workers and pending work; parser
bytes are read on demand rather than retaining every source/bytecode/native image.
Each public capture closes its file-mutation fence. A freshly prepared stable
identity is not redundantly reopened before its immediate bind, but bind still
rejects same-size, restored-mtime mutation and the mandatory final fence verifies
all bound paths with bounded workers. Runtime-build capture explicitly requests
four workers. Directory capture retains membership, object, access-mode and
timestamp checks;
directory storage allocation size is not semantic identity. Windows can change
that reported size during read-only enumeration. File lengths remain exact,
and failed snapshots report the specific differing metadata fields and values.
Each root retains its compact membership fingerprint through outer publication,
with its original exclusions/pruning, so later additions and topology changes
cannot escape by leaving the previously captured regular files untouched.
An internal directory symlink such as virtualenv `lib64 -> lib` is represented as
a same-root directory alias and is not traversed, leaving the ordinary target
tree as the single content authority. External directory aliases, malformed
targets, access drift and alias cycles remain rejected.
Receipt-owned tool selection uses the same tree/access/link semantics, including
Unicode host-name resolution.
Requirement versions, console entry points and Meson/Ninja/pkg-config discovery
come from the realized distribution inventory, not ambient metadata rescans.
One executable selector uses a distribution's declared console launcher when
present, otherwise its unique platform-named installed payload. Native commands
may live in the scripts directory without console-entry-point metadata. Missing
or ambiguous ownership and changed content fail closed; no PATH search or module
wrapper chooses a different executable. Ninja invokes the exact verified command
whose hash it records, while retaining the locked distribution version separately
from the executable's reported version.
Meson receives that same Ninja path explicitly, overriding ambient selection;
native executable config tools and Python console entry points share discovery
and cross-file projection. Version probes use the shared stable-executable fence.
Subsequent Ninja dispatch and final Ninja/config-tool manifests revalidate the
captured file generation instead of blessing a replacement with a new hash.
Selection and provisioning share one recipe computation per request; requirement
resolution and tool lookup retain one validated inventory instance.

Environment v6 owns exact declared external import regions, including Git-ignored
source and data. Runtime and external imports share one minimal-root forest;
overlapping regions are captured once. Owned absolute-directory `.pth` declarations
bind editable distributions to their active roles. Custom executable/editable
mapping finders are outside the verified capability subset. Reviewed upstream
startup hooks are admitted by exact content provenance, live origin/code and
explicit environmental conditions, never by filename or class name alone. This
is input/import custody, not an operating-system sandbox or a guarantee about
arbitrary future file I/O. The bootstrap policies and provenance belong to
`molt.python_external_custody`; unknown templates require review rather than
post-failure fallback. Distribution-owned startup versions and reviewed wheel
URL/hash identities are checked against `uv.lock`; dependency upgrades must carry
independently verified provenance while preserving exact member/RECORD ownership
and live inactivity gates. Full environment capture is not a location operation.

Native dependency admission currently proves the **loaded import closure**, not
all possible future lazy imports. Its capability vector gates CPython >=3.12,
OS, architecture, ABI and linkage explicitly; unresolved non-contract libraries
fail with diagnostics. Synthetic PE/ELF/Mach-O and receipt tests do not establish
live OS/architecture parity. Full capture and guarded-process tests are marked
slow and routed separately from the bounded unit lane by `tools/proof_plan.toml`.

These compilers, generators, and source-producer environments are maintainer and
source-build tooling. End users running shipped Molt binaries do not need uv,
Meson, Cython, Ninja, LLVM, or this producer environment unless they explicitly
request a local source rebuild.

---

## 3. ABI Tags (Proposed)
- `molt_c_api_version`: semantic version for the `libmolt` C-API (e.g., `0.1`).
- Wheel tags add `molt` ABI markers (e.g., `molt_abi0` + target triple).
- `molt` runtime rejects extensions with mismatched ABI tags.

---

## 4. Extension Metadata
Extensions declare Molt metadata in `pyproject.toml`:

```toml
[tool.molt.extension]
molt_c_api_version = "0.1"
capabilities = ["fs.read", "net"]
determinism = "nondet"
```

Required fields:
- `molt_c_api_version`
- `capabilities`

Optional:
- `determinism` (`deterministic` or `nondet`)
- `effects` (explicit effect contract for FFI boundary)

---

## 5. Build Flow
For a configured multi-extension package set, `molt extension produce-set` owns
steps 1-6 as one transaction and retains the upstream Meson graph as the sole
source/target/generator authority. Configuration names modules, Meson targets,
export ownership, and each module's explicit capability contract only; it does
not mirror source lists, include paths, generated headers, or Cython flags. An
empty capability array is an explicit least-authority contract, not missing
metadata.

1. Resolve `libmolt` headers and link flags.
2. Compile C/C++ sources with pinned flags for reproducibility.
3. For native targets, fold the extension objects into a deterministic
   `.molt.a` archive. The final application link consumes it through the typed
   ELF archive-group, COFF `/WHOLEARCHIVE`, or Mach-O `-force_load` plan.
4. For wasm targets, emit a wasm32 static-link `.molt.wasm` object, read its
   linking-section symbol table, typed import interface, and function exports
   in one in-process inspection, and reject missing declared `direct_symbol`
   callable exports. Linking symbols and imports are distinct facts: undefined
   data/global/table/tag symbols remain in the linker closure even when no
   function import has the same name, while imports retain their module and
   external kind instead of being flattened into linker names.
   Source-recompiled roots such as NumPy/SciPy must publish `python_exports` or
   `callable_exports`; package-root imports such as `numpy` require a
   `python_exports = ["numpy"]` owner rather than child artifact ancestry.
   Source-plan WASM builds resolve one compiler authority before compiling:
   `MOLT_WASM_CC`, then `MOLT_CROSS_CC`, then `zig cc`, then
   `clang + WASI_SYSROOT/WASI_SDK_PATH`. The selected compiler must compile a
   tiny WASI probe including `<errno.h>` before any upstream package objects are
   built, so missing sysroot/toolchain custody fails in seconds instead of
   after a broad NumPy/SciPy compile.
5. Run symbol audit and ABI tag validation.
6. Emit wheel + standalone artifact + `extension_manifest.json`.

Final-link requirements are a closed typed set: checksummed static inputs,
bare system providers, and explicitly admitted semantic options. Output modes,
tool selection, search/sysroot paths, response files, secondary outputs, and
unsealed scripts are not representable.

Captured upstream image commands are not final-link requirements. The shared
`source_extension_link_arguments` grammar separates their output destinations,
shared-image mode, diagnostics and image optimization controls from dependencies.
Meson projection consumes only that closed, target-dialect-checked policy class;
Molt owns its static artifact and the consuming executable's link profile. The
complete original `producer_link_args` remains in the source-plan digest and
receipt, including consumed controls. Publication validation indexes the sealed
Meson targets once using the live planner's selector authority, then requires
the receipt's target identity and ordered producer arguments to match that
checksummed target exactly. Rehashing an incomplete or reordered receipt cannot
replace the producer record. Explicit `extra_link_args` cannot use this
projection to take output authority. Unknown options, search paths, response
files, export scripts and ABI selectors are never silently dropped. Explicit
retention overrides (`/OPT:NOREF`, `--no-gc-sections`) and folding controls
remain unrepresentable until a typed closure/loading policy can preserve them.
Driver `-Wl,` lists and paired `-Xlinker` operands use the same grammar for projection
and dependency admission; literal comma-bearing paths require `-Xlinker`.

The host interpreter's Python link provider is a separate, attested role—not an
external target dependency. Extension-set schema 7 stages and checksums Meson's
`intro-dependencies.json` alongside target and compile metadata. An exact
`python` system dependency may consume the selected Windows interpreter's
file-node-owned import library only after its DLL and machine identity are
validated. The runtime-library and stdlib roles establish its base-prefix
layout; an equal basename elsewhere grants no authority. Ordered operand
occurrences, interpreter closure, import-library content, target Python version,
ABI tier and target triple remain in the source-plan receipt. Publication replays
the same projection from sealed metadata; rehashing a forged provider cannot
replace these facts. No filesystem roots are broadened and unrelated operands
remain subject to final-link admission. Other Python provider layouts, debug or
free-threaded host providers fail explicitly until their ABI contract is modeled;
dependencies with no Python link operand need no Windows-specific layout.
The shared compact-manifest validator also rejects reintroduction of the consumed
provider as an equal-content final-link input (including cyclic groups), or as
an ambiguous bare lookup for that interpreter library. A renamed checksummed
copy cannot evade the content check.

Extensions are recompiled against Molt's static ABI headers on both native and
WASM targets. Object and publication validation reject residual COFF
`__imp_Py*`/`__imp__Py*` obligations: importing CPython DLL data or functions is
not equivalent to referencing Molt's static C API. This provider projection
does not bypass the external-member dependency gate for lazy static targets.

Meson source folding uses one ordered linker-operand projection and only
metadata-declared static-library outputs. Exact output paths outrank basename
fallback; ambiguous basenames fail instead of selecting multiple targets.
Archive suffixes, including `.a` and `.lib`, do not independently establish
source ownership. Source loading, generated-input scheduling, exclusions, and
final-link handoff consume this identity together. Nested linker groups retain
operand order and meaningful repeats; compiler flags and linker executables
are not linker operands. Conflicting mirrored metadata is rejected.

Object pruning consumes module initialization, declared direct callable exports,
retained linker symbols, and forced source members through one dependency graph.
Forced membership is a compile-unit plan/digest fact, not an archive basename
exception in the final-link parser. Missing forced objects and ambiguous admitted
definitions fail explicitly. Retained symbols supplied externally remain linker
requirements. Mixed external providers/inputs and lazily folded source closures
require external member dependency facts. The existing typed link-requirements
authority rejects that combination before compilation when those facts are
absent; archive paths, search-name libraries, and default-library syntax cannot
create separate admission rules.
Forced folding into ELF extension archives is also rejected: its lazy final
archive group cannot preserve arbitrary forced members without a per-artifact
loading policy. COFF and Mach-O use their existing forced archive loading;
WASM consumes the selected relocatable object directly. These admission contracts
do not establish emitted-program conformance on an unexecuted target.

### 5.1 Known eager Python-import authority

Build, set publication, and resealing derive `runtime_python_import_modules`
from the complete checksummed owned input closure: every object source and its
declared header/dependency inputs. `source_extension_runtime_imports.py` owns
the lexical scanner; `source_extensions.py` binds its results to manifest
input custody. Missing, unreadable, or checksum-mismatched inputs fail
derivation; scanning only the available subset cannot publish fresh facts.
Successful derivation replaces stale facts rather than unioning them.

The persisted field is always a sorted, unique array of canonical dotted
module names, including explicit `[]` when no known eager literal imports are
found. `python_module_names.py` owns that name/list codec. Admission consumes
the persisted array without rescanning C/C++ inputs for Python-import roots;
missing, null, malformed, or noncanonical fields require rebuilding or resealing.
An attested root is not discarded merely because no Python file is present:
the graph still needs to resolve native sibling and self-module ownership.

These are known eager literal facts, not a C evaluator or a completeness claim
for arbitrary dynamic imports. Nonliteral names, object expressions, relative
package contexts, and runtime-dependent calls retain their existing runtime
semantics and capability policy. A complete owned-input scan does not prove
that all possible Python imports have been statically closed.

### 5.2 Retained compilation-input custody

`source_extension_input_custody.py` owns input resolution, retention, and
manifest projection. Each source and declared dependency is retained at
`provenance/compiled-inputs/sha256/<first-two-hex>/<sha256>` under the output
root. Byte-identical inputs share one address regardless of original filename
or source/build directory. Manifests declare the canonical `input_custody`
descriptor; `sources` exactly projects object-source order, and every source
and dependency reference is digest-derived and relative to that manifest.
Each projection rebinds object-closure identity to its actual references.

Sidecars and embedded wheel manifests therefore describe the same retained
bytes in their own relative namespaces. Wheels include the complete retained
input closure and package initializer bytes, with artifacts kept at their
module-relative package paths. The artifact checksum is bound before producing
any manifest view. `extension_wheel.py` owns canonical member emission and complete
RECORD regeneration: identical retained members are emitted once, conflicting
bytes fail, each retained member is checked against its input digest before ZIP
publication, and manifest replacement updates checksums and sizes. Publication
uses the shared `atomic_io.py` authority. Wheel inputs are validated before
atomic ZIP replacement; validation failures preserve the previous wheel.

An extracted wheel can be resealed from its embedded manifest and retained
inputs after the original checkout/build inputs are deleted. Resealing
revalidates those bytes, derives fresh eager-import facts, and projects custody
into the new output root. Resolution follows explicit manifest-relative paths
or bound input roots, never basename guesses, ancestor searches, or historical
checkout locations. Missing retained bytes require repair, not an alternate
source search.

---

## 6. Determinism + Security
- Build pipeline is reproducible when `--deterministic` is enabled.
- Configured set production requires deterministic artifacts, exact current
  ABI/tag/target/linkage, exact configured exports, checksummed nonempty object
  closures, real Meson-installed package roots, and a transaction manifest
  whose module/checksum inventory matches every published sidecar. Admission
  rejects missing, extra, stale, or legacy sibling artifacts.
- Extensions must declare capabilities and are blocked without explicit approval.
- `molt verify` checks wheel metadata and capability policies before load.
- Runtime import/load boundaries enforce extension metadata presence and
  validation (`molt_c_api_version`/`abi_tag`, declared capabilities, and
  checksum integrity for extension payloads; wheel checksum is validated for
  archive-backed loads). Successful checks are cached with path+manifest
  fingerprints so replaced artifacts are revalidated on the next import/load.
- Build-time external package admission enforces the same sidecar direction for
  `MOLT_EXTERNAL_STATIC_PACKAGES`: source-recompiled package roots such as
  NumPy/SciPy require at least one package-local native/static artifact
  candidate before module-graph discovery, and their package `__init__.py`
  sources are native runtime support custody rather than source-closure
  authority. Static import closure uses the shared `static_truth` guard
  primitive, including short-circuit boolean pruning, so dead
  `TYPE_CHECKING`/constant guarded imports and dynamic probes do not become
  package graph edges. Reachable package/subpackage artifacts
  (`.so`/`.pyd`/`.molt.wasm`/`.o`/`.a`) must have nearby
  `extension_manifest.json` metadata with matching module, extension path,
  checksum, ABI, target, platform, capabilities, `python_exports`
  entries that map source-recompiled package-level imports to the owning native
  artifact, and
  optional `callable_exports` entries that publish Python-visible callables.
  Each callable export declares `module`, `name`,
  `binding` (`module_attr` or `direct_symbol`), `abi`, optional `effects`,
  optional `deterministic`, optional `provider_module` for `module_attr`, and a
  required native `symbol` for `direct_symbol`. Direct `molt.object_call_v1`
  exports also require an explicit non-negative `arity`; call sites never infer
  the native signature. Other ABIs obtain their payload arity from
  `runtime/native_callable_abi.toml`. Bootstrap-only `molt.pyinit_module_v1`
  is not a public callable-export ABI. A `module_attr` export without
  `provider_module` is backed by the extension module itself and must name a
  callable present in the admitted extension source `PyMethodDef` table. A
  `provider_module` export derives checksummed upstream `.py` provider support
  source into `support_files`; package-internal provider imports are scanned as
  a bounded reachable closure instead of admitting the whole external package
  tree. Explicit `support_files` remains the escape hatch for additional
  non-provider support files and cross-package alias support files. If reachable
  support source imports a child module under the same
  source-recompiled native package, that child must be owned by compiled source
  custody or a declared native artifact; package visibility alone cannot create
  synthetic native package modules. Missing-child diagnostics search both the
  admitted package root and sealed sidecar provenance such as `sources` and
  `build.include_dirs` so staged artifacts can still point at the exact
  upstream `.pyx`/C/C++ source that needs target-specific source-plan custody.
  The `abi` token must be one of the canonical native callable ABI contracts:
  `molt.object_call_v1` for positional boxed object-call dispatch,
  `molt.object_callargs_v1` for the canonical CallArgs-builder handle used by
  keyword, star-arg, and C-extension call-protocol dispatch, or
  `molt.forward_f32_v1` for the unary bytes-backed Float32Array/browser lane.
  The validated callable export map owns publication and the native ABI, not
  Python call-site dispatch. Direct-symbol exports publish ordinary compiled
  wrapper functions whose bodies use `invoke_ffi`; object-call wrappers bind
  their declared positional-only arguments, and CallArgs wrappers bind normal
  `*args, **kwargs` before constructing the ABI payload. Call sites evaluate the
  live callable once and use normal call/binding operations, so captured
  references survive rebinding and replacements are called normally. Import
  assembly matches source/native owners by canonical module identity, never a
  sanitized init-symbol spelling. Distinct body owners that collide at linkage
  fail before source-body mutation; registry aliases own no body symbol. Local
  direct-symbol wrappers publish before provider initialization can call back
  into the module. Both export kinds publish once during initialization, not on
  cached re-entry, preserving later user rebinding. A failed generated
  publication deletes its owned cache entry through the shared module-cache
  authority (including registry and `sys.modules` projections); pre-publication
  and cached-hit failures do not delete an existing module.
  Import
  visibility through `known_modules` cannot create Python `module__function`
  symbols for native packages. WASM lowers reachable `direct_symbol`
  object-call exports into deterministic `molt_native` imports and direct call
  edges; `molt.forward_f32_v1` uses the same import namespace with one
  Float32Array payload and a boxed bytes result so browser hosts and linked wasm
  objects can share one callable-export contract. The corresponding
  `invoke_ffi` IR stores callable identity only in native callable metadata; its
  `args` vector is the ABI payload, never a synthesized Python callee/module
  attribute. `module_attr` exports retain the actual provider or PyMethodDef
  callable and use the normal runtime call protocol; no spelling-based FFI
  substitution occurs. `module_attr` + memory-buffer ABIs still fail
  closed because pointer/byte-buffer calls require an addressable
  `direct_symbol`. Split-runtime browser packages project only
  remaining `app.wasm` `molt_native` imports into `manifest.json` at
  `abi.browser_embed.native_callables.symbols` with the canonical ABI signature
  (`molt.value... -> molt.value` for positional object-call,
  `molt.callargs -> molt.value` for callargs object-call, and
  `bytes.float32 -> bytes.float32` for `forward_f32`). Source-recompiled static-link artifacts
  passed to `wasm-ld` are link custody, not browser host-callable imports; once
  their symbols are resolved into `app.wasm`, the browser manifest table is
  empty for those symbols. The browser embed rejects packaged `molt_native`
  imports absent from that manifest table or whose signature does not match the
  ABI token. Any imported native symbol missing staged artifact-plan custody
  fails packaging before delivery. Admission also proves direct-symbol custody
  for static-link
  artifacts:
  `wasm_relocatable_object` artifacts must export the declared function symbol,
  and `static_archive` artifacts must list it in
  `object_closure.defined_symbols`. Sidecar object-closure schema v4 carries
  each translation unit's canonical `language` (`c`, `cpp`, `objc`, or `objcpp`).
  The existing Meson language fact and explicit language switches are normalized
  once at the producer boundary; direct sources infer language only from their
  original filenames. Compiler role, C++ header selection, and Cython generation
  consume this typed fact. The compiler receives exactly one canonical
  `-x <language> -c <source>` clause. Command custody rejects missing,
  contradictory, repeated, and after-source selectors. The original compilation
  operand remains in the command when source custody relocates its retained
  bytes. Digest-addressed paths and declared language values never supply missing
  command evidence.
  Source-plan units are identified by their owning Meson target and producer
  object output, not their source filename. Schema v4 requires `producer_unit`
  (`target_id`, canonical build-root-relative `object`) on every source-plan
  object, and binds it into both closure and compact-unit content identities.
  Repeated source bytes may back distinct SIMD/language/flag variants; duplicate
  producer outputs, conflicting commands, and command/JSON output disagreements
  fail admission. A shared operand-span grammar preserves forwarded compiler
  arguments across output/target/language selection, replay and receipt
  compaction; a backend operand is not a driver selector. Unsupported frontend
  input/output overrides fail explicitly rather than changing object custody.
  A source group can consume only its own target's object roots,
  never another target's same-source command. Forced versus lazy membership is
  target-owned. Actual retained-object undefined/defined symbols are the sole
  C-API reachability authority; source text never simulates preprocessing or
  overrides compiled facts. Supported declarations are projected through the
  selected ABI tier's owned header closure, shared with ABI generation; missing
  local SDK includes fail closed and system headers remain target-toolchain owned.
  Capsule/generated-name metadata is derived from each
  object's checksummed source/dependency closure, with a shared byte cache.
  Direct and source-plan builds both record compiler depfiles. Cython regeneration
  shares equivalent original/language/ordered-input
  requests, while distinct requests own separate outputs. Ninja ownership is
  queried from the output's own generator command, without transitive commands;
  whether an upstream generated C file already exists cannot bypass standalone
  regeneration. Non-Cython generators remain unchanged. Earlier schemas must
  be rebuilt from original producer metadata, not restamped.
  The closure also carries
  separate canonical linker and import boards. `defined_symbols` and
  `undefined_symbols` are the exact reachable linking-section facts.
  `wasm_imports` is the exact sorted set of `{module, name, kind}` receipts from
  the binary import section. Function receipts are validated against the
  module-qualified signature table generated from `wasm_abi_manifest.toml`;
  the same spelling in two modules is therefore not interchangeable. Known
  imports with the wrong module, external kind, or function signature fail
  admission, as do unknown imports. Non-function descriptors remain a typed
  interface-hardening frontier and must not be inferred from linker names.
  The ABI board classifies non-`Py*`
  `undefined_symbols` as project-defined through `defined_symbols` or
  runtime-backed through `runtime_symbols` only when the symbol is present in
  the generated WASM runtime/link import surface. Unknown runtime claims and
  generated runtime imports missing signed top-level `runtime_symbols` custody
  fail admission. Per-object `runtime_symbols` are forbidden: semantic
  consumers use the finalized top-level projection covered by
  `closure_sha256`. Toolchain provider archives are inventoried lazily only for
  unresolved symbols eligible for libc, compiler-rt, or libc++ classification.
  C-API symbols, project definitions, generated external-link/runtime symbols,
  and unknown `molt_`/`__molt` runtime names never trigger host sysroot reads;
  they remain governed by their existing board authority and fail closed there.
  Generated runtime/link authority takes precedence over archive collisions.
  The C/API board classifies `required_c_api_symbols` and `Py*`/NumPy
  `undefined_symbols` as runtime-backed, source-compile-only, project-defined,
  fail-fast, or missing; undefined C/API symbols cannot contain
  source-compile-only NumPy inline/macro APIs, fail-fast symbols, or unknown
  gaps. For `wasm_relocatable_object`, shared artifact admission independently
  compares the binary's linking-section definitions and unresolved symbols with
  `object_closure.defined_symbols` and `object_closure.undefined_symbols`, and
  compares the binary import section with `object_closure.wasm_imports`.
  Missing or stale facts in either board fail admission. C-API data relocations
  such as `PyExc_*` and `Py_None` are linker requirements even though they are
  not function imports; memory imports such as `__linear_memory` are import
  receipts and are not invented as linker undefineds.
  Each accepted C/API symbol is bucketed by reusable primitive class such as
  object/type lifecycle, module state, capsules, exceptions, refcount, memory
  allocation, buffer protocol, import system, call protocol, descriptors,
  unicode/text, bytes/bytearray, GIL/threading, code/frame/eval,
  iterator/mapping helpers, numeric scalars, Cython runtime helpers, or NumPy
  C-API. General ndarray storage and multi-buffer tensor ABI custody are
  separate contracts. `molt extension scan --json` emits the same
  `symbol_primitive_class`, `symbols_by_primitive_class`, and
  `primitive_class_counts` board used by sidecar admission so preflight scans
  and build-time custody cannot disagree about the missing primitive surface.
  Package-generated token-paste helpers are a separate custody fact, not a
  Molt runtime ABI claim: source-plan builds publish
  `build.source_c_api_scan.project_generated_c_api_prefixes`,
  `build.source_c_api_scan.project_generated_c_api_symbols`, and per-object
  `object_closure.objects[].project_generated_c_api_symbols`; public
  `molt extension scan --json` reports matching `project_generated_*` fields.
  Exact broad API-family prefixes such as `PyArray_` and `PyDataType_` are not
  accepted as generated-helper custody, and generated-prefix filtering only
  applies to symbols that would otherwise be missing. Runtime-backed and
  source-compile-only C/API facts remain visible requirements.
  Reachable native-artifact tree shaking is provider-closed: filtering to the
  user's graph, explicit imports, and runtime dispatch roots must retain every
  artifact that provides a capsule or same-package symbol required by a
  reachable artifact, transitively, including cycles. Symbol providers must
  have the same package root and target/Python/ABI/linkage variant and pass the
  ordinary artifact and manifest validation. WASM provider kind and function
  signature come from the inspected binary, not a symbol-name allowlist.
  Ambiguous providers, incompatible kinds/signatures, and collisions with
  canonical runtime/link/C-API ownership fail admission. Candidate inspection
  feeds one symbol index and the existing provider closure; do not independently
  reparse and revalidate every sibling for every consumer.
  Graph, wrapper-build, and backend object-cache identities include the
  validated artifact/manifest custody facts. WASM package
  admission fails closed before graph expansion when an admitted package
  contains native-source or host-extension markers but has no wasm32
  `static_link` `libmolt_source` artifact manifest; source roots alone are not
  linkable package evidence. Native builds publish the validated artifact,
  sidecar, package `__init__.py` chain, and runtime extension shim candidates
  into a deterministic `external_static_packages/<plan-digest>/` runtime root.
  Native binaries inject that staged root before runtime startup and include
  staged bytes in final link reuse fingerprints without adding runtime-loaded
  extensions to the linker command. Linked WASM builds pass staged
  `wasm_relocatable_object` and `static_archive` artifacts to `wasm-ld` as
  validated native object/archive inputs and include the staged artifact,
  manifest, and support-file bytes in the link fingerprint. Target modes
  without a runtime-custody consumer fail closed when external native artifacts
  are admitted.

  Artifact closure is not permission policy. Capability requests and host grants
  continue to resolve through the canonical capability-manifest and runtime
  authorities. A target or binary-interface fact may prove that an operation
  can be represented; it cannot grant filesystem, network, process,
  environment, clock, randomness, or extension authority. Conversely, a
  capability grant cannot make a missing target primitive or malformed artifact
  interface valid.

---

## 7. Integration Points
- Pact witness root selection resolves the versioned scientific-stack
  authorities and accepts exactly the canonical NumPy and SciPy seals. It does
  not scan worktrees or union historical SciPy per-module roots.
- `molt deps` should classify extensions as Tier B when `libmolt`-compiled.
- `molt build` rejects source-recompiled external package admission with no
  native/static artifact candidates before module graph discovery, rejects
  WASM native-source packages without staged wasm static-link artifacts, rejects
  WASM source-recompiled packages whose static-link artifacts do not publish
  `python_exports` or `callable_exports`, rejects
  reachable provider support-source imports whose native package child modules
  lack source or artifact custody, rejects
  reachable external package extensions with missing or mismatched sidecar
  metadata before backend dispatch, uses sidecar `python_exports` to bind
  package-level imports to native artifacts, threads sidecar
  `callable_exports` into scoped lowering/cache facts and `invoke_ffi` native
  callable metadata, routes supported direct-symbol object-call exports into
  backend native import tables, routes supported module-attribute object-call
  exports through runtime FFI dispatch without inventing native imports, fails
  closed when backend ABI dispatch for that metadata is absent, and publishes
  validated native artifacts plus sidecars and runtime shims into deterministic
  build artifacts for native runtime import custody.
- `molt extension audit` can require manifest public-export custody with
  `--require-python-export` and `--require-callable-export`, so stale
  source-recompiled artifacts fail before graph discovery with the exact
  publisher flag needed to republish the sidecar. It can also require
  `libmolt_source`/`static_link`/`wasm_relocatable_object`, standalone artifact
  presence, extension SHA-256 match, and object-closure metadata, so agents can
  prove the native bytes and sidecar the WASM linker will consume before
  entering expensive package graph or backend work.
- `molt verify` enforces capability allowlists for extension loads.
- CI runs an extension publish dry-run matrix (native + cross-target) covering
  `molt extension build`, `molt extension audit --require-abi`,
  `molt verify --extension-metadata`, and `molt publish --dry-run`
  for extension wheels (`linux native`, `linux cross-musl`, `macos native`).
- CI also asserts the wasm build contract (`molt extension build --target wasm*`
  emits a wasm32 `static_link` `wasm_relocatable_object` artifact and sidecar
  object-closure board).

---

## 8. TODOs
- TODO(tooling, owner:tooling, milestone:SL3, priority:P1, status:partial): expand cross-target extension build coverage for additional linker/sysroot variants and source-recompiled package publish readiness checks.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:partial): extend `molt verify` extension policy gates with signature/trust policy coupling and richer diagnostics.
- TODO(tooling, owner:tooling, milestone:SL3, priority:P2, status:planned): define canonical wheel tags for `libmolt` extensions.
