# Toolchains (macOS, Linux, and Windows)

## Recommended baseline
- CMake + Ninja
- LLVM/Clang/LLD/MLIR/Polly (for LLVM and MLIR backend development)
- A complete LLVM distribution with `llvm-config` matching the Rust
  `inkwell` feature pinned through `molt.llvm_toolchain` from
  `runtime/molt-backend-native/Cargo.toml`.
- One SDK prefix owns LLVM, Clang, LLD, MLIR, Polly, and TableGen. Molt projects
  that SDK identity into each binding's required path shape and rejects split
  identities or a mismatched major/minor before a build starts. `llvm-config`
  may live outside that prefix
  only when its own `--prefix` result proves the same SDK identity, as in
  Debian/Ubuntu's versioned `/usr/bin/llvm-config-<major>` layout. In that
  layout `LLVM_SYS_<ver>_PREFIX=/usr` is the llvm-sys executable-search root,
  while `MOLT_LLVM_PREFIX` and the MLIR/TableGen prefixes remain
  `/usr/lib/llvm-<major>`.
- Every companion executable must report the exact patch release owned by
  `config/llvm_toolchain_releases.toml`; matching only the major/minor is not
  sufficient for an accepted SDK.
- WebAssembly tools come from the host wasi-sdk asset owned by the same
  manifest. One provisioned SDK owns `wasm-ld`, `llvm-nm`, `VERSION`, and
  `share/wasi-sysroot` at the SDK's LLVM producer release, which is pinned
  independently of the backend release above. A separately discovered linker
  or package-manager sysroot is not an equivalent release toolchain.
- Rust (for runtime components + WASM + package implementations)
- Tooling and tests use the exact CPython patch in `.python-version`. Guest
  semantics target Python 3.12 and later; the host tooling pin is a separate contract.
- Cargo-hosted DX helpers: `wasm-tools`, `wasm-pack`, and `cargo-edit`
  (`cargo-upgrade`) for dependency sweeps.

## WASI C and C++ ownership

The pinned wasi-sdk supplies one complete C ABI: headers, libc, the binary128
print/scan archive, compiler-rt, and command/reactor startup objects. The finite
Python/Rust transport is declared in `src/molt/wasi_c_abi_protocol.txt`. Cargo
uses the selected raw `wasm-ld` with stable `linker-flavor=wasm-ld` and
`link-self-contained=no`. Library artifacts have no command or reactor CRT;
the corresponding executable producer owns startup. C/C++ driver commands
retain explicit target/sysroot and `--no-default-config`. C++ additionally
retains its lexical `clang++` role or explicit `--driver-mode=g++`; shared
compiler image bytes do not make C and C++ final-link defaults interchangeable.
The shared protocol also declares Rust's link mode. Its Python producer is
checked with the pinned stable rustc before the Rust build-script decoder;
rustc's unstable `wasm-lld` spelling is not the stable `wasm-ld` option.

`RuntimeCargoPlan` retains the selected managed SDK generation. Shared and
relocatable specifications, link arguments, and fingerprints project its finite
receipt facts; there is no separate link-input state or unused Rust-builtins
archive input. Configuration and toolchain getters are pure. Mutable external
Cargo configuration, native/Rust tools and resources retain admission and actual
execution fences. Generated static libraries and export-response files retain
capture, post-link checks, format validation, and atomic publication.
Pure `wasm32-unknown-unknown` Rust requires no C SDK. Explicit C-provider builds
carry the C ABI projection even when their final application is freestanding.

The same retained C ABI projection supplies ordered native-library search roots
for target Rust dependencies, before user search roots. The selected libc and
compiler-rt directories therefore reach std linkage and proof capture as well
as Molt build scripts. SDK roots retain managed receipt identity and portable
path labels; they do not enter mutable recursive resource capture. User search
roots keep their existing byte and membership fences. Build scripts validate
this complete context and retain formatter-before-libc and whole-archive
ordering; they do not introduce a second SDK search projection or global CRTs.
The standalone unknown-target CPython ABI retains its own bundled libc obligation;
it is not interchangeable with the runtime's unbundled downstream archive policy.

The default DX environment exports WASI Rust flags only for `wasm32-wasip1`.
An explicit unknown-target C-provider environment uses the existing
`python -m molt.llvm_toolchain --verify-wasm --wasi-rust-target
wasm32-unknown-unknown --github-env <path>` projection. Native flags are untouched.
For direct Cargo, global `RUSTFLAGS` or `CARGO_ENCODED_RUSTFLAGS` override Cargo's
target-specific flags; callers selecting those lanes must carry the complete
projected WASI context. The shared Rust decoder rejects an incomplete effective
context. A runtime Cargo plan instead completes its selected effective lane
before capture and execution.

Rust codegen options have one token authority in `molt.rust_toolchain`:
`-C value`, `-Cvalue`, `--codegen value`, and `--codegen=value` share the
same semantic projection. Short-option clusters retain preceding no-argument
switches (for example `-gC...` and `-vC value`). Underscores and hyphens in
codegen keys are equivalent; text after the first `=` remains opaque. Missing
or empty codegen operands and `-C=value` are refused.
The pinned Rust outer option arity consumes each raw value first, including
paths that begin with `-C` or `--codegen`. Codegen operands retain their exact
bytes; shell parsing belongs only to the
existing textual environment boundary. Cargo's own options and its `--`
delimiter remain separate from the forwarded Rust lane and Rust's end of options.
Proof receipts retain the observed command and relative-tool argument spans.
Runtime flags normalize spelling before resource custody and identity; aliases
cannot select a linker, backend, or response file outside that custody. A `-L`
value has an explicit kind only when its prefix names a known Rust search kind;
otherwise the whole value, including `=`, is an `all` search path. Those literal
paths receive the same capture and SDK-prefix ordering checks.
Interrupted-process inventory marks malformed or truncated arguments incomplete.

The runtime target coordinate appends its reference-types/SIMD directive only
when it differs from the last actual target-feature directive. Later SDK search
and external-libc options do not cause repeated policy growth. Re-resolving a
plan's environment therefore preserves its recipe and identity across target,
`RUSTFLAGS`, and `CARGO_ENCODED_RUSTFLAGS` selection. This policy does not parse or
rewrite opaque linker arguments or add guest instrumentation.

Development Rust link capture uses Cargo's admitted artifact selection.
Cargo-level `--crate-type` replaces selected manifest kinds; forwarded rustc
crate types add to them. Forwarded-only explicit kinds reuse the existing Cargo
metadata selector: discovery locates the finite workspace/selected manifest and
source inputs, then one authoritative query runs inside their generation fence.
The final Cargo transcript and digest own the selected target kinds; receivers
rederive that selection and reuse/armed custody verify the captured input bytes
without invoking Cargo. This conditional second query is limited to ambiguous
forwarded kinds; explicit Cargo overrides do not query manifest metadata. `lib`, `rlib`, and `staticlib` are archive-only;
a successful probe for those kinds must print no linker command. Link-producing
kinds require their own captured process images, independent of the host
proc-macro unit. Unspecified Cargo output kinds retain the std capability probe.
Telemetry v4 and unit v2 bind the exact producer command, artifact selection and
per-unit image membership through reuse, armed capture and persisted receivers;
these facts do not add guest code or change runtime content keys.

Native-extension linking admits SDK roles against original paths, then carries
those roles and content identities through private snapshots and import
rewrites. Provider planning finishes before capture closes. A split app forces
its captured formatter once before lazy libc; the combined link resolves it
from the relocatable runtime. Object/member definitions and canonical runtime
imports are subtracted before compiler-rt discovery. An unresolved dormant
archive member can defer an absent SDK; an eager obligation requires it.
Requested provider families are filtered before archive inspection.

Source-extension Meson setup, build-tool probes and generator materialization
use the existing fresh build directory as their working directory. It is
disjoint from authored source and retained on failure: compiler detection may
create or replace implicit linker outputs even during a version query.

`wasm.test.control-flow` depends on the WASM host/shared runtime and executes independent binary128
FILE/stdout/string witnesses and a real Meson C++ library/ABI witness. Its
`wasi-clang` proof identity selects the SDK's compiler, helpers, and resources;
native `clang` retains the backend's separate LLVM policy. Before live custody
is armed, proof selection captures the finite receipt and actual helper images,
and registers broad watches for SDK `lib` and `share/wasi-sysroot`. After arming,
one resource capture per selected generation checks bytes and portable topology
against the receipt. All admitted tool roles share that capture. The existing
custody CAS stores its inventory once; receivers expand it and require its exact
`FrozenFile` projection, required target/role membership and process-image
closure. Closing byte checks and live mutation watches retain their existing
semantics, including new directory members and transient changes. A tree digest
alone does not replace them. Native identities retain their existing digest
contract and carry no SDK field. These development captures and checks add no
guest instrumentation. Required Cargo library,
installed-package, and hosted-platform acceptance remain separate proof cells.

## Toolchain state selection

`molt.dx.canonical_toolchain_root` owns mutable tool-state selection for generic
pinned tools, WASI SDK, managed native LLVM, Binaryen and DX/proof consumers.
The compiler source owns release manifests; the invoking guest project does not
select another manifest or turn an installed compiler into a development checkout.

A valid explicit `MOLT_TARGET_ROOT` selects the root, including a root that is
not yet provisioned. Admission rejects invalid selected paths, and missing tools are diagnosed at the
selected root without searching another installation.
Without an explicit root, installed bundles and wheels use
`<MOLT_HOME>/target-root`, taking the existing platform default home when
`MOLT_HOME` is unset. Development retains the checkout/hosted/scratch custody
rules. A known damaged installation fails admission rather than silently becoming
a development checkout. The shared `molt.default_paths` module owns home paths.

Selection does not create directories, probe writability, provision tools, scan
the installation tree or execute a tool. Explicit provisioners and read-only
consumers use the same selector and child environment. Existing explicit
per-tool selectors retain their precedence and validation. Source identity,
checkout custody and Cargo output remain separate authorities; tool state is
outside installed source. These operations belong to the compiler and development
apparatus and add no work to emitted guest programs.

Artifact-only guards preserve explicit selectors and bind an installed default
tool root before adding artifact/cache defaults. Actual managed-tool consumers
own root admission and provisioning. Guard artifact setup therefore leaves an
unused managed root unvalidated when an independent external tool was selected.

## CI toolchain admission

`.github/actions/setup-project` is the shared CI provisioner on Linux, macOS,
and Windows. It installs the pinned uv release, then `provision-python.sh`
installs the exact `.python-version` CPython into the job's ephemeral custody
root. It selects through uv's managed interpreter API and validates CPython,
the exact patch, the managed installation root, and both `python` and `python3`
aliases before publishing any Python environment outputs. It does not depend
on the Actions Python catalog or an assumed Windows executable layout. Python
setup requires uv; later consumers inherit `UV_PYTHON_DOWNLOADS=never` and the
verified `UV_PYTHON` executable. An unavailable archive or invalid alias fails
setup before repository Python code executes.

Rust setup resolves one complete installation plan through
`tools/check_rust_toolchain.py`. The `pinned` role includes the stable channel,
components, and targets in `rust-toolchain.toml`, plus normalized caller
additions. The separate `sanitizer-nightly` role uses the dated nightly in
`config/rust_nightly_toolchain.txt` and its explicit components and targets.
`provision-rust.py` performs one explicit installation, verifies compiler and
Cargo versions, selected sysroot and tool paths, component inventory, and
host/target standard-library files, then selects the default. Installation or
validation failure stops setup without destructive cleanup or retry.
`RUSTUP_AUTO_INSTALL=0` is exported before consumers start, so a missing or
partial installation cannot trigger competing repairs during proof fanout.

Proof commands that build Rust declare both `rustc` and `cargo`, including the
Python binding and runtime-artifact partitions. The existing proof executor
fingerprints the union of declared tools before scheduling commands; Rust
fingerprints share its serialized `rustup` domain. Cargo configuration precedence
is owned by `molt.rust_toolchain`; runtime plans, queued proofs, and standalone
fingerprints consume it without importing the CLI. The core SDK/LLVM authorities
likewise own finite receipt selection and helper-byte capture; guarded proofs
project those facts into their process-image schema and armed resource custody.
This developer setup has no
emitted guest runtime cost. Archive availability, aliases, native binaries,
and target libraries still require real cold-provision validation on each
supported runner OS and architecture; simulated setup tests are not that proof.

## Python tooling source ownership

`src/molt/cli/python_source_closure.toml` declares the compiler/tooling graph's
ordered Python search roots separately from its admitted source roots. The
repository namespace search location supports qualified `tools.*` imports;
it does not admit temporary trees, build artifacts, tests, or other repository
directories as tooling source. `LocalPythonModuleResolver` enforces the same
canonical source boundary for named imports, seed identities, namespace portions,
and complete inventories needed by unknown relative import anchors. Symlink
aliases outside that boundary are not local sources. Cycles and enumeration
failures inside the admitted domain remain errors, never partial coverage.
The manifest bytes and resolved inventory topology participate in closure
identity, so changing root order or source ownership invalidates dependent caches.

## Host temporary-directory custody

Guard scratch and WASM tool stages share `molt.temporary_artifacts` directory
allocation. POSIX allocations retain private `0700` permissions. Windows
allocations inherit the admitted parent's ACL instead of installing CPython's
user-only `0700` DACL, which excludes restricted child tokens. Pytest's numbered
and xdist-provided base directories use the same Windows mode projection.
`OwnedTemporaryDirectory` fences cleanup to the directory generation it created;
replacement directories are not cleanup authority. Linker staging, optimizer
transactions, facts-scanner snapshots, and WASM profiling/pipeline helpers use
this owner rather than independent host-default temporary-directory lanes.

## macOS
- Install Xcode CLT: `xcode-select --install`
- Homebrew recommended: `brew install llvm mlir cmake ninja pkg-config`
- Provision the manifest-owned wasi-sdk for `wasm32-wasip1` builds (see WASM
  targets below); do not combine a Homebrew `wasi-libc` sysroot with an
  independently installed linker.

## Linux (Ubuntu/Debian)
- `sudo apt-get install -y cmake ninja-build pkg-config llvm clang lld mlir`
- WASM tools come from the same manifest-owned wasi-sdk provisioner as on
  macOS and Windows, not from distribution packages.

Hosted CI does not maintain a parallel package script. The local
`.github/actions/setup-llvm` action has two projections of the same
`molt.llvm_toolchain` authority. `profile=full` installs (Linux only, through
the commit-pinned Debian installer) and verifies the complete
LLVM/MLIR/LLD/Polly SDK. `wasi=true` provisions the exact host wasi-sdk through
`tools/provision_wasi_sdk.py` on every release host: Linux, macOS, and Windows
on x86-64 and arm64. `profile=wasm` is that SDK alone and requires `wasi=true`.
`config/llvm_toolchain_releases.toml` owns the WASI SDK release, its LLVM
producer identity, the provenance release, and each host asset's URL, byte
size, SHA-256, and archive root; every asset URL must belong to the provenance
release, and the asset set must equal the shipped release matrix.

The provisioner selects only the current host coordinate and reuses a cached
archive only when its size and digest match. Before bounded extraction it
rejects non-portable paths, portable-name collisions, root escapes, links
outside the archive root, and special nodes. It then publishes one
identity-addressed installation,
`<toolchain root>/toolchains/<archive root>-<asset record digest>`, atomically
and only after the C/C++ compilers, archive tools, linker, `llvm-nm`, VERSION,
headers, and libc are present.
The v2 receipt binds the exact host asset record to a digest over every SDK path,
file byte, and link target. The same authenticated tree walk derives finite
identities for the six required tools, optional `llvm-strip`, five C ABI members,
and the two proof resource subtrees; role aliases resolve through its captured
link graph. Rendering those facts never rereads live member bytes.

The provisioner owns append-only managed generations. Ordinary compilation reads
the finite receipt and does not rescan SDK contents or rerun SDK tools merely to
project identity. Supported consumers must not edit or remove an active generation;
this contract is not OS-enforced immutability or protection against its owner.
Actual archive parsing, source snapshots and compilation still consume their
required bytes. Provisioning and explicit verification detect SDK drift instead
of silently adopting edited bytes as another valid release.

Only the provisioner upgrades a v1 receipt: it verifies the existing tree once
against the old aggregate and exact selected asset, derives v2 facts from that
capture, and atomically replaces only the receipt. It does not redownload,
reinstall, repair or rewrite SDK payload files. Ordinary consumers require v2
and diagnose an old receipt with the provisioning command; there is no legacy
consumer fallback. Provisioning reuse and `--verify-wasm` still verify the managed
generation. CI caches only the digest-keyed archive.

`--verify-wasm` recomputes the tree identity, proves the exact SDK `wasm-ld`
and `llvm-nm` releases, and projects `MOLT_WASM_LD`, `MOLT_LLVM_NM`,
`MOLT_WASI_SYSROOT`, `WASI_SYSROOT`, and `WASI_SDK_PATH` to every consumer in
the job. Target-qualified Cargo C/C++ compiler and archive selectors use that
same SDK in place, preserving adjacent resources and libraries. Target C/C++
flags disable implicit `clang.cfg` configuration; the admitted target and sysroot
remain authoritative. Native PATH, compiler selectors, and flags are unchanged.
WASM archive inspection reads WebAssembly objects in-process (see below) and
runs only an `llvm-nm` at the SDK's LLVM release for bitcode members, from
`MOLT_LLVM_NM` or the custody-provisioned SDK. A reader in the selected managed generation retains
that installation and its finite role fact; its generation and content bind the
symbol cache without image rehashing or version probes. An explicit external
reader retains actual executable capture, exact-version admission, alias and
mutation fences. Object/archive byte custody, parsing and cache validation are
unchanged. There is no Rust-toolchain or ambient native `nm` fallback. Build and readiness paths discover a provisioned SDK but never
install one.

Native symbol inspection reads ELF (32- and 64-bit, both byte orders),
Mach-O (thin and universal), COFF (including bigobj and PE images), COFF
short-import members and WebAssembly relocatable objects in-process, through
`src/molt/native_symbol_table.py`. Archive members come from the
`static_archive_identity` framing (GNU, BSD and COFF variants and long-name
tables; thin archives are refused). The reader runs no subprocess and has no
wall-clock bound, so its facts do not depend on the host's `nm` or its load.
It classifies each global symbol as `llvm-nm -g --no-llvm-bc` does, so it reads
the native symbol table of an object that embeds `__LLVM,__bitcode`. A universal
Mach-O input yields the slice for the target architecture. Truncated, overlapping
or inconsistent tables and unrecognized formats fail with a typed artifact
error.

Only LLVM bitcode (raw or wrapped, detected by magic) needs an external reader.
An object or archive with a bitcode member walks the managed `nm` ladder
(`llvm-nm`, then `nm`) and admits each candidate by its `--version` banner. Only
`llvm-nm, compatible with GNU nm` (LLVM's tool, which is also Xcode's `nm`) can
read bitcode; a `GNU nm (GNU Binutils ...)` banner or any other banner fails that
candidate's admission with the banner it printed. The reader runs as
`llvm-nm -g`, with its bitcode reader enabled, and only the tables of bitcode
members become facts. The symbol-facts cache key names that reader only for
artifacts that contain bitcode.

`MOLT_LLVM_NM` selects one executable, not a shell command. A selected path or
PATH-resolved name must retain its lexical role. Mutable external readers pass
resolved-content custody before probing and retain execution/cache-reuse checks.
Quoted paths preserve spaces and native separators without admitting arguments.

Optimized linked WASM builds require Binaryen. Managed installations come from
`config/binaryen_releases.toml`, provisioned by `tools/provision_binaryen.py`.
The manifest owns each host archive and extracted-tree identity, not
`config/tool_releases.toml`'s standalone validator executables. Managed admission
checks the live executable digest before running it and requires the exact tagged
release version. An explicitly selected external release may report the upstream
numeric-only version or its matching release tag; cache and publication identity
retain the exact reported spelling. Manifest, receipt and executable version
records share an ASCII decimal release grammar. Hosted WASM CI
uses `.github/actions/setup-binaryen` alongside pinned `wasm-tools` provisioning
and passes its exact `wasm_opt` output as `MOLT_WASM_OPT` to the proof partitions.
Link fingerprints require optimizer identity only when optimization is selected;
missing tooling is a provisioning error, not permission to omit optimization.

Rust via rustup:
- `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh`

## Windows
- Install Visual Studio Build Tools (MSVC) or full Visual Studio.
- Install LLVM/Clang: `winget install LLVM.LLVM`
- Provision the manifest-owned Windows wasi-sdk asset for WASM builds; its
  verified SDK root holds `wasm-ld.exe`, `llvm-nm.exe`, and the sysroot.
- The LLVM backend specifically needs `llvm-config.exe`; some Windows LLVM
  installers include `clang`/`wasm-ld` but omit `llvm-config`. Those installs
  are useful for native/WASM linking but are not a complete Rust LLVM backend
  toolchain. Build a matching MSVC LLVM/Clang/MLIR developer prefix with:
  `python -m tools.bootstrap_llvm` (or the equivalent direct-script entry
  `python tools/bootstrap_llvm.py`; the exact patch release, URL, size, checksum,
  and provenance come from `config/llvm_toolchain_releases.toml`).
  The bootstrap command prints `MOLT_LLVM_PREFIX`, `LLVM_SYS_<ver>_PREFIX`,
  `MLIR_SYS_<ver>_PREFIX`, `TABLEGEN_<ver>_PREFIX`, and `LLVM_CONFIG_PATH`; all
  name the same verified prefix.
  Release source archives are accepted only when their SHA-256 matches
  `config/llvm_toolchain_releases.toml`; that manifest also owns the exact
  canonical build type. Extraction is archive-bound and rehashes the extracted
  tree before reuse. Extracted sources and installed prefixes share one
  transactional publisher: exclusive process lock, unique staging prefix,
  durable phase journal, and deterministic rollback or completion on startup.
  Canonical path identity is never deletion authority: every nonempty source or
  build tree must carry a valid self-consistent tool-owned marker, otherwise the
  bootstrap fails closed without modifying it. There is no legacy reset lane.
  The CMake cache is keyed by the release record, extracted-source digest, and
  a digest of the architecture/target/project/build configuration. Canonical
  project, target, and build-type sets are exact (no extras) and are bound into
  the published attestation with the release-manifest digest. Publication
  occurs only after the host linker,
  LLVM-C, Clang, LLD, Clang resource headers, LLVM/MLIR/LLD/Polly libraries, and a real
  C++ compile-link probe all pass. Cached validation projects that attested
  proof and forces full hashing whenever NTFS ChangeTime is unavailable.
  Drive letters and directory names do not establish toolchain custody. Source,
  build, download, prefix, and environment selections retain the same content,
  alias, receipt, and publication checks on every host. Unlisted development releases require an explicit
  noncanonical prefix, source URL, and SHA-256; their source/download/build
  custody is derived beside that prefix and cannot overlap canonical managed
  custody.
- Install CMake + Ninja: `winget install Kitware.CMake` and `winget install Ninja-build.Ninja`
- Ensure `clang`, `llvm-config`, `cmake`, and `ninja` are on PATH.
- Run source LLVM builds from an x64 Visual Studio developer shell, or let
  `tools/bootstrap_llvm.py` activate `VsDevCmd.bat` from an installed Build
  Tools instance.

## Generated Python formatting

The Ruff exclusions in `pyproject.toml` apply equally to directory scans and
explicit pre-commit filenames. Generated projections are changed only through
their owning generator. Generators that intentionally format Python text pass
`--no-force-exclude` to Ruff for their output path; routine hooks do not rewrite
or reinterpret that generated authority.

## Direct file read custody

`molt.toolchain_identity.open_stable_regular_file` owns direct-file admission,
reading and closing checks through the native descriptor authority in
`molt.file_hashing`. Windows admits concurrent readers but excludes writers and
deletion for the descriptor's lifetime. Existing writers prevent admission;
write or delete conflicts fail the transaction. POSIX retains no-follow opening
and before/after path, handle and content-change metadata checks.

Runtime tree capture enumerates names, then each bounded worker admits, hashes
and closes one file. Size and digest come from that same owned read. There is no
detached metadata pass or second content hash, and live descriptors are bounded
by the existing worker limit. Name enumeration is not an atomic filesystem
snapshot: byte observation starts when each file is admitted.

Content digests identify bytes. Detached `StableRegularFileVersion` values and
their later verification compare metadata observations; equal values do not
prove continuous nonmutation while the handle was closed. Windows can assign
identical ChangeTime values to distinct writes, including same-size writes with
restored mtime. Retaining an observed token does not extend an owned read's
write exclusion beyond its context.

`read_stable_regular_file` is the content-admission boundary for a previously
captured digest: it hashes the returned bytes inside the same owned read and
rejects a mismatch, including equal-metadata substitutions. This adds one
in-memory SHA-256 and no second read or descriptor. Callers that need both a new
identity and its bytes use `capture_stable_regular_file` in one read. Metadata
polling remains distinct; it does not perform implicit whole-tree rehashes.
Executable digest and native header capture share one owned descriptor; it is
closed before a version subprocess runs. The later version-probe fences remain
metadata observations and do not claim continuous execution identity.

Mapped WASM symbol parsing and observed artifact copies compare the digest of
their actual mapped buffer or copied stream through the same content check.
Runtime generation staging also checks supplied identities while copying.
Native custody hashes and parses an archive under one owned handle; warm
archive admission hashes current bytes before reusing parsed member semantics,
and extracted members are rehashed before reuse. A supplied digest never
authorizes different bytes merely because metadata matches.

WASM fact, linking-symbol and structural caches reuse parsing or validation
results only after current member content matches. Cold structural validation
retains the owned handle through the external validator. Final binding checks
hash both members and the pinned receipt. These content-admission boundaries
explicitly request hashing; ordinary version and tree-generation polling still
compare metadata only and retain the detached-token limits above.

Managed LLVM SDK verification retains a separate explicit policy:
`content_policy="full"` hashes the content manifest; `"cached"` hashes after
recorded path/size/mtime/ChangeTime drift or unavailable Windows ChangeTime.
The cached policy can miss equal-metadata substitutions and therefore is not
fresh byte attestation. Detached executable/version fences likewise do not
prove which bytes a later subprocess loaded. These limits are distinct from
the owned content-read boundary above and must remain explicit in acceptance
or execution-identity claims.

## Python runtime identity

Editable PEP 610 file locations admit only empty or case-insensitive localhost
authorities. Their percent-decoded filesystem bytes, Windows drive admission,
absolute-path and real-directory checks are independent of the host Python minor
version. Invalid or multiply encoded URLs fail with the environment-identity
diagnostic; remote authorities and path indirection are not source custody.

Python environment identity captures one immutable loader snapshot in
`molt.python_native_locations`: the OS-loaded executable, loaded paths, loader
aliases, non-file loader contracts, and observed Mach-O CPU identities. PSAPI
identifies the Windows executable; dyld image zero identifies the macOS main
image. Linux requires the first loader image to agree with `AT_PHDR` and the
kernel mapping's device/inode identity; ambiguous explicit-interpreter launches
fail closed. Configured CPython base/venv launchers remain content-bound inputs,
not invented loaded importers or providers. The receipt designates an observed
executable component and binds that designation into its closure digest.
Dependency capture consumes and
rechecks that same snapshot. On macOS, universal images are read through the
exact slice already selected by dyld, never the first or generic matching slice.
Missing files require an explicit shared-cache contract; census or slice changes
invalidate the capture. This does not relax exact-target binary admission or
claim support for another architecture. Synthetic fixtures project the selected
executable from their realized environment, not the test runner's executable.

## Distribution boundary

LLVM/MLIR is a developer and source-build dependency. Shipped Molt binaries
must package the optional MLIR backend executable and the redistributable
runtime libraries it needs, with platform/architecture gating at package build
time. Binary-only end users must not need Cargo, CMake, Ninja, TableGen, or an
LLVM SDK. A source checkout builds the standalone backend once on first MLIR
use through the same manifest-pinned toolchain authority.
The manifest-owned wasi-sdk follows the same boundary: developers and source
builders need its verified tools and sysroot when producing WASM artifacts;
end users running shipped native or WASM binaries do not.

## MLIR diagnostics

The standalone backend exposes bounded, opt-in developer telemetry without
changing release artifacts or the binary-user dependency boundary:

- `MOLT_MLIR_TRACE_FUNCTIONS=1` reports each function as lowering begins.
- `MOLT_MLIR_ONLY_FUNCTION=<name>` isolates one function from a captured
  SimpleIR module for diagnosis.
- `MOLT_MLIR_OPT_LEVEL=O0|O1|O2|O3` selects the progressive-lowering pipeline
  level for a diagnostic run.
- `MOLT_MLIR_DUMP_DIR=<path>` writes each function's MLIR before verification,
  so a verifier failure retains the exact input that produced it.

These controls are diagnostic projections of the same backend and pass
pipeline. They do not authorize a second compiler path, partial artifact
publication, or a relaxed verifier.

WASM targets:
- `rustup target add wasm32-wasip1 wasm32-unknown-unknown`
- `cargo install wasm-tools --locked`
- `cargo install wasm-pack --locked`
- Build, run, doctor and queue preflight never install Rust targets. Readiness
  and link inputs use the selected compiler in Molt's source directory, not a
  guest project's Rust pin or another toolchain's target directory. Missing
  standard libraries stop compilation with an explicit setup command; run that
  command yourself to authorize the installation. C/C++ source extensions
  require only the inputs declared by their target plan, not an unrelated Rust
  standard library.
- Provision the host wasi-sdk explicitly with
  `uv run --python 3.12 python tools/provision_wasi_sdk.py`. It installs under
  the [selected toolchain state root](#toolchain-state-selection)
  (or explicit `--toolchain-root`) and prints the
  install prefix. `uv run --python 3.12 python -m molt.llvm_toolchain
  --verify-wasm --wasi-sdk <install> --format json` verifies it and reports the
  `wasi_sysroot`, `wasm_ld`, and `llvm_nm` paths to export as
  `MOLT_WASI_SYSROOT`, `MOLT_WASM_LD`, and `MOLT_LLVM_NM` (or set
  `WASI_SDK_PATH=<install>/sdk`). WASM symbol inspection finds the
  custody-provisioned SDK without an export.
- Every `wasm-ld` consumer (runtime WASM builds, final links, `tools/wasm_link.py`
  and `molt doctor`) resolves the linker through
  `molt.llvm_toolchain.resolve_wasi_sdk_tool`: `MOLT_WASM_LD`, else the SDK named
  by `WASI_SDK_PATH`, else the custody-provisioned SDK. There is no `PATH` or
  rustup search, and Molt never installs a linker; when none is selected the
  error names `tools/provision_wasi_sdk.py` and the selectors.

## Cargo workspace truth custody

The canonical Rust truth runner separates network custody from execution. Root
`Cargo.toml` and `Cargo.lock` own the ordinary compiler/runtime workspace,
including the dependency resolution used by trybuild's offline child. The runner
performs one locked fetch, one locked package-metadata query, and one locked
`--workspace --tests --no-fail-fast` traversal, all with the root manifest.
Intentionally isolated MLIR, fuzz, bootstrap, and probe workspaces are not added
to this traversal. A host target-runner hook gives each
`resource_enforcement` test a fresh process so process-global address-space
limits cannot poison sibling tests or convert an exact failure into an
unattributed SIGABRT. The outer Rust proof receipt uploads the runner's nested
receipt containing root-scoped phase return codes, exact observed red identities,
and the suite-honesty verdict. Binary receipts remain under `binaries/root`, bound
to the run, source snapshot, and exact executable bytes; Cargo metadata and
compiler artifacts own package/target identity and complete binary coverage.
Prefetch or metadata failure publishes a terminal receipt without starting tests.

`molt.cargo_workspace` projects declared members for the harness and structural
checks. Lock validation additionally follows local dependency manifests (including
excluded helpers, target/dev/build dependencies, and workspace inheritance), so a
cached lock check cannot overlook a changed input. Cargo remains the dependency
resolver. Add ordinary crates explicitly to the root members list; independent
workspace exclusions are not a second ordinary runtime workspace.

Backend compiler receipts and compiler-only object-cache identities use the
same conservative root-package projection of `Cargo.lock` from
`molt.cli.cargo_source_closure`. It retains all reachable package records,
checksums, sources and dependency edges, including optional, development and
target edges; Cargo still resolves features and owns whole-lock validation.
Unreachable rows and their stale dependency references do not invalidate the
compiler. Reachable missing, malformed, duplicate or ambiguous identities fail
closed. Git references match the Cargo source selector without requiring its
precise revision fragment; path packages take precedence for source-less
references. The reached package's full source, including revision, remains an
input. Runtime build identity retains its complete lockfile input.

Compiler and runtime source topology, runtime Cargo feature expansion and lock
projection share one stable TOML reader. Each use admits current document bytes;
the operation transaction reuses parsing only by content hash. Reachable
manifests are traversed directly, including nested path dependencies. No
process-wide manifest/stat stamp, hand-enumerated manifest list, graph cache or
profile-feature cache can preserve obsolete dependency or feature selection.
Unreadable or invalid reached documents fail admission instead of becoming an
empty dependency graph. Runtime raw-lock semantic identity is unchanged.

Developer compiler identity uses the same resolved Cargo plan as execution:
effective flags, configuration bytes, selected profile controls, tool and resource
content, C build inputs and selected LLVM prefix are admitted before lookup.
The Cargo plan owns the shared tool/resource projection. Runtime builds augment
it with their required build-Python admission; Cargo-only compiler builds do not
probe a runtime generator interpreter or inherit its runtime closure.
Resource directories are traversed as graphs: a resolved directory already on
the active ancestor path is a backedge, not a second resource subtree. This
admits distro layouts such as LLVM's `build/Debug+Asserts -> ..` without infinite
unfolding. Non-cyclic directory aliases retain their lexical file selections;
file-content mutation, added resources and alias retargeting that changes the
selected resources remain fenced by the same capture and verification authority.
Compiler Cargo commands use `--locked` and execute that exact plan without
changing wrappers on retry. One source-fingerprint operation owns reusable plan
admission; subsequent operations recapture live inputs. A clean Git HEAD is
never cached across operations. After Cargo, fresh source and lock identity plus
configuration/tool/resource custody must match before alias materialization,
feature-probe receipts or artifact receipts can publish. Actual Cargo rebuilds
also retain a source/lock generation fence: no-follow file observations,
directory membership and change times reject observed generation changes.
This includes restored bytes and mtime when the filesystem exposes a new change
time; metadata equality alone does not prove continuous nonmutation.
That live fence is not a portable semantic key or receipt, and warm receipt hits
do not capture it. Identity rejection is a structured build/run/deploy failure.

Installed compiler admission verifies its release source/package once per
immutable operation and verifies executable bytes and feature/profile selection.
The shared run/deploy wrapper owns this scope without caller pre-admission.
Cold builds perform one fresh post-child source admission before publishing the
wrapper manifest; warm cache hits perform one mandatory source admission and no
child build. Both compiler and tooling keys reuse each admission. This does not
create cross-operation trust or eliminate the installed source inventory check.
Wrapper and daemon keys reuse that admitted release identity, including its
tooling and runtime inventory, without developer Rust discovery or Cargo probes.
Daemon selection also commits to executable content and request codegen/runtime
ABI controls; source mtimes are not a separate freshness authority.

Root profiles are the only profile authority: compiler `release` retains unwind
support; shipping native runtimes use `release-output`/`release-size`, and WASM
uses `wasm-release`. Public guest `release` requests native `release-output`
and WASM `wasm-release`; `dev` requests `dev-fast` for both. A selected
`MOLT_RELEASE_CARGO_PROFILE=release-output` remains a physical WASM profile
choice as well as a native one. `MOLT_WASM_CARGO_PROFILE` takes precedence over
`MOLT_RUNTIME_BUILD_PROFILE`, which takes precedence over the requested WASM
profile. Prebuilds, installed-cell selection and proof receipts use the same
request/resolution authorities. Select profiles explicitly rather than changing
policy by launching Cargo from a different directory.

Host and guest profiles are independent. The CLI defaults to the production
`release` compiler for both guest `dev` and guest `release`; guest runtime Cargo
overrides never select the host compiler. Compiler developers may explicitly use
`MOLT_BACKEND_PROFILE=dev` (host `dev-fast`) or a host-specific
`MOLT_{DEV,RELEASE}_BACKEND_CARGO_PROFILE` in a source checkout. Packaged compilers
are immutable release inputs: unsupported features or host-profile overrides
fail with a diagnostic rather than rebuilding/replacing the installed binary.
Installed runtimes follow the same rule. `release-compiler-source.json` declares
typed runtime cells (native staticlib, native-link closure and archive-derived
callable projection; WASM shared/reloc generation with the full CPython C-API
surface) keyed by target, runtime Cargo
profile, stdlib tier, features and SIMD/freestanding. `molt.cli.installed_runtime`
selects exactly one cell, admits bundle bytes through the canonical native-link
and WASM-generation receipts, requires their `RuntimeBuildIdentity` to agree
with the key, and retains one content-addressed generation under
`MOLT_HOME/installed-runtime`. Each build operation admits that generation once;
code generation and final link compare stable-file metadata observations and
re-derive the cell selection, failing on an observed generation or selection
change. These detached comparisons have the limits described under direct file
read custody above; they do not establish continuous nonmutation. Byte readers
that consume a captured digest validate it against the actual returned bytes.
Native code generation admits the cell's callable projection (signed bytes,
canonical encoding, named for the retained archive's digest) and never runs a
symbol reader.
Installed builds, doctor, setup and `molt update` never resolve a runtime Cargo
plan, probe or install Rust, consult `MOLT_WASM_RUNTIME_DIR`, or substitute
another cell. A platform wheel installs the same bundle in its scheme's
`share/molt/distribution`; the CLI binds its site-packages package to the
signed source inventory. An installed CLI never adopts a nearby checkout;
source builds are an explicit `MOLT_SOURCE_ROOT`. Commands needing compiler inputs
select them through `molt.source_root.compiler_source_root()`: the executing package's
source root or its installed distribution, with an explicit override taking
precedence. Guest/project discovery never selects compiler sources. The separate
cwd-based compiler resolver and its cached guesses are removed, so build,
extension, maintenance and developer commands cannot silently use different
compiler trees. User-project discovery is a separate live path authority:
`MOLT_PROJECT_ROOT` selects the project, while `MOLT_SOURCE_ROOT` selects
compiler inputs. Package commands select their project through user-project
discovery and reject sealed compiler trees. Project environments, lock validation
and vendoring outputs follow the selected user root; a Python-only project does
not acquire a compiler Cargo dependency. Runtime Cargo builds are a
source-checkout workflow and the release producer `tools/release/runtime_cells.py`,
whose cells are admitted with the installed receipt rules and whose
`compile.sources` must equal the release snapshot's runtime source tree
identity (hashed once per producer or bundle operation). In a checkout,
`molt update` refreshes the rustup-managed pinned toolchain; installed
maintenance only provisions pinned auxiliary tools.
The selected binary identity flows through cache keys, daemon/one-shot execution,
native linking, build diagnostics, and the installed-consumer receipt.

Source builds publish every backend feature variant, including native, to its
own executable with a content-bound receipt. Cargo's unqualified output is only
publication input; selecting it for native would let a WASM build replace the
native compiler and force a rebuild on the next target switch. All variants use
the same publication lock, atomic copy, and admission rules. Process discovery
recognizes these variant names without treating a name as ownership evidence.
After Cargo completes, a rejected compiler feature probe fails the build with
its original diagnostic and publishes no new provenance. The CLI does not
rerun the unchanged Cargo plan to conceal that failed outcome.

For source-checkout reuse, `MOLT_SKIP_RUNTIME_REBUILD=1` disables compiler and
runtime source builds, including Cargo-based provenance refresh. It does not
skip source, configuration, content, manifest or feature admission. Compatible
local artifacts and validated cache hydration remain available; unavailable or
invalid artifacts produce an explicit rebuild-policy failure without invoking
a Cargo build. Installed distributions retain their immutable admission rules regardless
of this developer setting.

`tools/release/native_build.py` owns snapshot builds and receipts for the
production compiler, launcher and worker; `build_compiler.py` projects its CLI.
The source snapshot supplies the Rust channel, and developer profile/CPU flags
and wrappers cannot change native release policy. Private Cargo homes and
configuration-free build roots exclude ambient Cargo configuration. Darwin
SDK/tool selection and activated Visual Studio roots are pinned before tool
identity admission; LLVM's ATL check belongs only to LLVM bootstrap. The installed
consumer runs the shipped native launcher for both guest profiles and binds each
build/run command and compiler/launcher identity into admission. The same
executable entry point serves direct invocation and package-manager links; there
is no separate shell/batch launcher policy. Source and transport fixtures do not
establish release acceptance.

The native launcher embeds the bootstrap from the same frozen source revision.
It executes the CLI exclusively from `source/src`; neither a copied bootstrap nor
a second installed Molt wheel is an execution authority. The separately published
wheel is independent of the native bundle. The bootstrap verifies locked inputs
and the stdlib-only default-path authority before reusing its home selection.
Environment generations are keyed by source/dependency and interpreter identity
under `MOLT_HOME`, outside the installation prefix. Normal launches use uv's
offline synchronization check without modifying the environment. Explicit
`molt setup --install-cli-dependencies` authorizes frozen, exact dependency-only
sync, including removal of unrequested packages only inside that private
environment. It explains the source, destination and scope; it never installs
Python/toolchains, edits PATH or changes another installation. The installed CLI
never invokes Rust toolchains. Ambient project and uv
resolver/install overrides cannot select another dependency closure. There is
no exported-requirements resolver or independent Molt venv/locking implementation.
Python re-entry (including the REPL) inherits only the selected source import
root. Package-manager interpreter bindings are explicit; a broken binding fails
without falling through to a different interpreter. Setup/doctor report active
source/Python and PATH ambiguity using the shared executable search authority;
they neither infer package-manager ownership nor automatically remove a copy.
Python import-graph analysis uses the same mutable `MOLT_CACHE`/platform-cache
authority as other compiler caches, with separate project namespaces. It never
stores graph hints in compiler source directories. Cached requests remain
validated against source bytes, import policy and analysis implementation.
All commands share sealed-source dependency admission. Installed compilation,
extension builds and dependency consumers do not re-resolve the
compiler's Python dependencies with ambient project configuration. The shared
native/WASM runtime Cargo plan (source checkouts and release production only)
always uses `--locked`, independent of guest
determinism settings. Installed metadata identifies the bundled source commit
and compiler toolchain, not a containing guest Git repository or toolchain.
The native launcher resolves the bundle from its executable, including package-
manager symlinks. Windows paths use lossless Win32-compatible canonical spelling
where possible, so the same source root remains usable by Python, Cargo and C
toolchains; namespace prefixes are never stripped when that would change path
identity.
Installed dependency-update commands cannot rewrite sealed source inputs; use
the package manager or a new bundle to upgrade the compiler. Package SBOMs do
not infer a built artifact's Rust toolchain from a probe on the packaging host;
build/release receipts own that provenance.

Development dependency symbols have one Cargo-native owner:
`[profile.dev.package."*"] debug = 0`. The wildcard covers non-workspace
dependencies, including external path dependencies and future additions; it
does not classify packages by a name prefix. Workspace members keep the parent
profile's debuginfo unless explicitly overridden. Cargo merges named package
settings per field, so the Cranelift-codegen/regalloc2 optimization overrides
inherit the wildcard's symbol policy. `dev-fast` and Cargo's built-in `test`
profile inherit `dev`; release-derived profiles and isolated workspaces do not.
Do not mirror dependency lists or implement another profile resolver in tooling.
Cargo input/cache identity includes the complete root manifest.

Compiler implementation crates extracted from `molt-backend` retain its
development policy (`opt-level = 1`, `debug = 0`), including the shared IR,
optimization passes, codegen ABI, publication, and native/WASM/text backends.
The named overrides live only in the root `dev` profile and are checked by
`tests/cli/test_backend_manifest_contract.py`; do not duplicate them in
`dev-fast` or infer them from crate-name prefixes. New independent crates and
excluded workspaces need their own measured policy, not automatic inclusion.
Shared compiler crates can also serve runtime and host consumers, so verify
those consumers when changing their profile policy.

Debuginfo, optimization, debug assertions, and overflow checks are separate
settings. Dependency `debug = 0` does not disable assertions or overflow checks,
but Cargo also projects debuginfo into build scripts' `DEBUG` environment input.
Consequently a symbol-policy change needs real consumer verification; it is not
guaranteed to be storage-only. Measure cold build cost separately from runtime
speed before introducing new per-package optimization overrides.

## Platform Pitfalls
- **macOS SDK/versioning**: Xcode CLT must be installed; if linking fails, confirm `xcrun --show-sdk-version` works and set `MACOSX_DEPLOYMENT_TARGET` for cross-linking.
- **macOS arm64 + Python 3.14**: uv-managed 3.14 can hang; install system `python3.14` and use `--no-managed-python` when needed (see `docs/spec/STATUS.md`).
- **Windows toolchain conflicts**: avoid mixing MSVC and clang in the same build; keep one toolchain active.
- **Windows LLVM backend**: official, winget, and Chocolatey LLVM binaries may
  omit `llvm-config`; do not treat them as satisfying `llvm-sys` until
  `llvm-config --version` reports the required major/minor.
- **Windows path lengths**: keep repo/build paths short; avoid deeply nested output folders.
- **WASM linker availability**: `wasm-ld` and `wasm-tools` are required for linked builds; use `--require-linked` to fail fast.
- **WASM debug section placement**: linked-output normalization orders and merges
  standard sections while preserving custom bytes and relative order. Custom
  sections remain after their preceding standard sections, so name and DWARF
  metadata stay after the declarations they describe. Already ordered modules
  retain their original bytes. Normalization rejects section renumbering when
  relocation metadata is present; it cannot preserve section-index relocations.

## Build-Python operation custody

Source runtime builds retain one live isolated build-Python admission for the
build operation. Native code generation/final linking, shared and relocatable
WASM publication, and standalone WASM CPython-ABI publication use the same
`BuildPythonAdmission` authority. Standalone producers close their own scope;
the CLI build output scope closes the admission on success and every failure.
Installed runtime cells continue to bypass build-Python capture.

Each boundary independently reselects the interpreter and checks its captured
executable metadata and native-loader environment. The selected interpreter
retains `PythonFileCaptureContext`; reuse compares no-follow metadata
observations, complete directory membership, import-root selection
including absent archives and startup selection files (`pyvenv.cfg`, `._pth`,
and `pybuilddir.txt`), and the original loaded-native-image census. A
failed verification or exited/closed session revokes admission. These metadata
comparisons have the detached-token limits described above. Reads from captured
file nodes independently validate their bytes against the node's digest.
A retained JSON
receipt is never sufficient to admit the next boundary.

The existing guarded interactive command owner holds the capture process and
its inherited streams. The protocol emits one runtime receipt, accepts bounded
verification requests, and closes on EOF. One-shot captures use the same
capture context and verifier. Source, Cargo/tool configuration and artifact
publication checks remain fresh at their existing boundaries; only successful
live runtime verification permits reuse of the immutable semantic projection.

### Native C/C++ processes in development proofs

A proof command that compiles native C or C++ declares `cargo_native_units`
in `tools/proof_plan.toml`, with explicit build roles and source languages, for
example `{ target = ["c", "c++"], host = ["c++"] }`. This is an operation
requirement, not a property of every Rust dependency. Equal target and host
triples share one archiver and resource inventory while retaining every declared
role and the union of their languages. Host-only C++ selects no unused C driver.
Runtime-only WASI builds use their managed SDK closure and do not acquire a
native build obligation. Native runtime commands that accept the full stdlib
profile carry both languages because full builds compile simdutf; the MLIR
TableGen proc macro requires host C++. Pure Rust, C-only LLVM wrappers and
native micro-runtime operations retain their narrower requirements.

The existing Cargo selector owns CC/CXX/AR precedence: target spelling,
underscored target spelling, HOST_/TARGET_ spelling, then the unqualified
variable. Each language consumes its own CFLAGS or CXXFLAGS. Native defaults
come from the same target plan as ordinary Cargo; cross selections require
explicit tools. Full Cargo native proofs currently require each native unit's
target to equal the Rust compiler host: paths alone do not establish cc-rs's
target-specific default flags. Host C/C++ units in cross Rust builds remain
valid; source-extension commands retain their separate explicit target authority.
Selected source compilers are independent of the Rust linker and bindgen's
CLANG_PATH. Unresolved Cargo configuration overrides, package-relative resource
paths and unsupported compiler wrappers fail before capture. Pre-arm binding
publishes captured physical RUSTC/CARGO paths and target-qualified CC/CXX/AR
selections, so the payload does not reselect them through rustup or ambient
defaults.

Rust capture records each selected language's actual frontend and
preprocessed-assembly driver phases, the independently selected archiver,
mutable resource inputs, and helper images. GCC helpers such as cc1plus may
reside outside the driver's directory; their reported commands supply the image
paths. MSVC cl/lib retains its in-process frontends. Native source-extension
C/C++ roles use this same phase authority; freestanding WASM has a frontend
phase but no GNU assembly unit. Managed WASI compiler roles keep their existing
SDK image authority.

Build roles, languages and helpers are validated through image projection,
armed capture, persisted CAS receipts and reuse. The current plan, command
envelope and Rust selection schemas reject earlier C-only records; omitted or
substituted languages cannot be accepted by resealing a receipt. Warm reuse
checks current selection and bytes without rerunning driver phases. Rustup
component selection is resolved before a cache lookup; unchanged proxy bytes do
not pin an old override. Shared C/C++ resource and Node package contents are
inventoried once after watcher arming; discovery retains selected roots and
finite resolver/manifest facts. New members between discovery and arming enter
that complete inventory. Full receipts require the inventory, and live watches
cover subsequent membership changes. The same captured files enter the Cargo
cache key and live mutation guard. Build-script-specific flags may select
additional children; they must satisfy actual child custody, never a guessed
helper allowlist. These are development-proof costs and do not add checks or
instrumentation to emitted guests. A resource-guarded benchmark alone does not
claim this full process closure.

### Compiler host hash seeds

The CLI preserves normal CPython startup semantics and does not restart Python
to select a hash seed. Compiler IR serialization and backend cache payload
identities must be independent of the host seed. Installed CLI execution keeps
Python `-I` isolation; ordinary `PYTHONHASHSEED` remains an interpreter-launch
experiment control where CPython permits it. The former Molt seed override and
restart sentinel are retired with explicit environment migration diagnostics.
Wrapper build reuse still includes `PYTHONHASHSEED` in its broader environment
identity; cross-seed wrapper reuse is not implied by backend payload determinism.


### Required release lanes and installed products

`config/release_acceptance_matrix.toml` is the single declaration of executable
release lanes. The shipped `molt.release_lanes` reader validates it against the
Cargo and verified-subset authorities. Each logical lane carries backend, target,
guest profile, runtime Cargo profile and compiler Cargo profile. LLVM has native
target semantics; it is not a third runtime target. Declaration is not compiler
capability or execution evidence. Installed readiness requires both the delivered
compiler features/profile and matching runtime cells.

Release runtime production projects those lanes through the installed cell keys,
deduplicating shared native/LLVM runtime products while preserving stdlib tiers,
source-extension features and hosted/freestanding WASM variants. Each physical
request supplies its actual Cargo profile. The producer clears ambient profile
selectors inside its existing isolated build scope; unsupported feature or source
location overrides remain errors. User compilation retains explicit installed
cell selection and never compiles a replacement runtime.

The installed consumer proof carries the complete logical lane record for every
Python coordinate. Platform-wheel verification uses the same product producer and
receiver for the first declared reference Python, rather than a separate native
smoke artifact. Sealed post-uninstall replay retains backend/profile-distinct
product IDs. Missing lanes, wrong profiles, compiler substitution and duplicate
products are refused. Consumer proof v8, replay v2 and performance shard v2 replace
their older internal shapes together; selected profile observations alone still
do not establish authenticated compiler/cache origin or used-byte performance
evidence. Those stronger release requirements remain separately enforced.

The compiler production feature tuple includes `llvm` and retains the base
`llvm22-1` pin alongside `inkwell/llvm22-1-force-static`. Source and release builds
share one selected-SDK admission: `llvm-config`, selected headers and the complete
`--libnames --link-static` archive closure remain under resource custody through
publication. `--libdir` and `--includedir` must refer to the selected SDK. Native
build v3 records this location-neutral input identity and source policy; candidate
admission and the signed SBOM consume that record. Static LLVM linkage does not
claim static system C/C++ linkage; the existing target compatibility audits own
that boundary. Installed readiness checks delivered compiler/runtime capability
and does not require source-development SDK tools. Full SDK provisioning uses the
existing Linux package and non-Linux source-build owners. None of these source
contracts constitutes actual six-host execution or release acceptance.
