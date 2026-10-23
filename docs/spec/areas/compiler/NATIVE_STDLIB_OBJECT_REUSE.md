# Native stdlib constituent object reuse

The shared stdlib archive remains the reachability, runtime admission and final
publication authority. A program-key miss first looks the archive up by its
content: an unchanged partition transfers whole. Otherwise only incompatible
constituent objects compile, and both admitted and newly emitted ordinary
objects pass through the existing native archive validator/assembler and
archive publisher.

## Closed native object inputs

`NativeBatchObjectJob` carries its module context inline. `close_dependencies`
uses `NativeBackendModuleContext::object_dependencies` to select all definitions
and the generated defined-function reference edges, including indirect calls,
name-taking constructors and task targets. Canonical callable metadata adds
escaped callable targets; an application job also includes its resolver and
module-registry symbols. It retains exact external/import decisions for those
names. There is no body-only key and no whole-program context path to consult.

`project_object_dependencies` owns the projection of all eight context fields:
partition-source chains, function arity, return presence, closure membership,
task kind, task closure size, leaf membership and complete native linkage ABI.
The ABI includes source signature, parameter custody, machine parameter types
and return carrier. An exhaustive struct destructure requires new context
fields to receive a projection policy. The worker consumes exactly the context
that is hashed, rather than computing a smaller key over a larger live graph.
There is no additional opcode classifier or guessed companion-symbol rule.

The object input digest binds the complete named MessagePack job (including
ordered FunctionIR, extern declarations, literal bits, profile and target) and
the canonical versioned FunctionIR contract. It also binds every field of the
admitted shared-stdlib compiler manifest except its whole-program `cache_key`.
In particular the complete cache variant, compiler fingerprint, exact profile,
backend selection, runtime callable semantics and canonical codegen environment
remain exact. The manifest carries the object variant: the build configuration
with the codegen environment projected to the native-worker roles a job reads
(`native_object_environment_inputs`). The program context (entry module and
stdlib module set) only selects the functions of each job and never reaches the
worker, so it stays out of the manifest; whole-program keys and the archive key
bind it. A new import therefore keeps every unchanged constituent. The executing backend file's SHA256 and the effective Cranelift
target/shared/ISA flags plus host TIR SIMD facts are additional inputs. LLVM
builds bind the host triple, CPU and features used by its target-machine owner.
Transport paths and timestamps are never content identity. The existing CLI
runtime-codegen binding still owns file verification and worker environment
inheritance; constituent reuse does not relax or replace that admission.

## Linkage carriers

Before partitioning, the backend freezes one native linkage ABI row per
defined function (`NativeBackendModuleContext`). The source signature and
parameter custody come from the `FunctionIR`. The machine carriers (parameter
carriers and the inferred return type) come from the function's TIR lowering
and its value-range and representation facts, so they are a pure function of
one `FunctionIR` contract and the backend code.

The carriers live in the backend binary's TIR cache namespace
(`molt-passes::tir::cache`), keyed by the function's contract digest
(`linkage_carriers.rs`). An unchanged body costs one digest and one verified
read. A changed body has a new key; its row is computed on the request's
compiler pool and published. Compilation diagnostics bypass this reuse, as they
bypass every other backend cache. Go's build cache serves an unchanged package
by its action ID in the same way.

Before this, every build that reached the backend lowered every defined body,
the unchanged stdlib included: 5.6 s on one thread for a 91-line asyncio
program (3,612 functions, 3,592 of them stdlib). Reuse takes 0.24 s. In an
interleaved comparison of the same edit-rebuild (macOS arm64, load 9-11,
three builds each), build CPU fell from 22.5 s to 17.2 s and wall time from
27.8 s to 21.7 s (medians). Computed and reused rows link byte-identical
binaries, and computed rows hit all 171 constituents published from reused
rows.

## Stable bounded batches

Stdlib batching first groups functions by their existing exact `source_file`
identity. Missing source metadata forms one group; it is distinct from an empty
source path. Each group orders symbols by a domain-separated SHA256 and starts
with one radix bucket. A bucket splits on further hash bits only when the
existing function-count or operation budget requires it. Siblings never absorb
overflow from another bucket. Source groups and symbols are ordered
deterministically, independent of input traversal. No new module classifier or
worker scheduler is involved.

Adding an unrelated source cannot change existing compilation units. A budget
change can change batches, and a reachability change can split or coalesce
buckets within its source group. Keeping sources separate can increase cold
worker launches in exchange for avoiding cross-source recompilation. A single
oversized function remains one compiler input under the existing megafunction
authority.

Even zero- and one-body archives use this same path. An empty partition emits
one real native object containing the ABI anchor. The former direct-only
stdlib compiler path is removed.

## Archive reuse by content

The archive key binds the program (its entry module and stdlib module set),
because the CLI must find the archive before any backend work. Two programs
with the same stdlib partition therefore had different keys, and each new
program restored all constituents, assembled the archive and copied it into
the cache: 67 MB written three times for a 91-line asyncio program whose 171
constituents all hit.

The archive is a pure function of its ordered constituent inputs. Each input
key already binds the complete job, the program-independent manifest, the
executing backend bytes and the effective ISA (see "Closed native object
inputs"). So the backend first plans the constituent jobs and hashes them
(`PlannedStdlibArchive`, 0.23 s for 3,592 functions), then derives the
archive's content key from the ordered input keys
(`StdlibObjectCache::archive_key`). A content-addressed generation lives in
`<cache>/native-stdlib-archives-v1/<content-key>/stdlib.a` with the shared
sidecar contract: the key sidecar holds the content key and the manifest
sidecar the program-independent constituent manifest. When it is present and
admits, it transfers whole to the build path, which records the program's key
and manifest: no constituent admission, no assembly. Otherwise the plan
assembles, and the new archive publishes to both the content entry and the
program entry. Go's build cache serves an unchanged package archive by its
action ID in the same way; the program entry stays the CLI's lookup key, as
Go's action ID maps to an output ID.

Every archive transfer is a platform copy
(`molt_artifact_publish::copy_file_atomically`, Rust's `std::fs::copy`): a
copy-on-write clone on APFS, btrfs, XFS and ReFS writes no data, and other
volumes copy in the kernel. A transfer verifies the private copy against the
source's recorded digest before it replaces the destination; a mismatch is a
miss, as for any archive admission. That one read also leaves the copy's pages
cached for the CLI admission and the linker. A clone is a new file, so its
first read goes to disk: on the USB build volume that read costs about 1.8 s
for 67 MB, against 3-5 s for the writes it replaces. Sidecars of a transferred
copy record the verified digest instead of hashing the copy again.

Interleaved A/B, macOS arm64, the 91-line asyncio program as a new program
(new entry module) each build, warm constituents, `MOLT_BACKEND_DAEMON=0`,
host load 8.7-18.3 (four builds per arm): wall 23.2 s to 16.2 s and CPU 16.05 s
to 14.89 s (medians). The stdlib step fell from 0.9-2.9 s of constituent
restore plus 2.5-4.5 s of publication to 0.23 s of planning plus 1.8-2.0 s of
transfer. A content miss with warm constituents (the content store moved
aside) kept its wall time (23.4 s and 23.35 s); constituent admission fell from
0.9-2.9 s to 0.02 s. An edit of one program in one output directory reuses the
build path as before (13.8 s and 14.9 s). Every archive in these builds, cold,
transferred or assembled from mapped constituents, is byte-identical
(sha256 `181e8f015a7d259c...`).

## Admission and publication

The canonical archive and sidecars live in
`<cache>/shared-stdlib/<archive-key>/stdlib.a`. Content-addressed archives
live in `<cache>/native-stdlib-archives-v1/<content-key>/stdlib.a` with their
own lock stripe. Constituent generations live in
`<cache>/native-stdlib-objects-v1/<input-digest>/object.entry`, independently of
an archive generation. Persistent bounded lock stripes in `<cache>/locks/`
serialize each archive or constituent generation across publishers, readers
and retention. Explicit standalone archive paths retain their adjacent
publication lock. Compilation workspaces belong to guard or suite scratch;
only immutable entries and their publication stages live in the cache.
`MOLT_STDLIB_OBJ` names the required build-owned archive. The CLI hydrates it
under cache custody before local or daemon cache admission; a backend archive
miss produces it before publishing the optional `MOLT_STDLIB_CACHE_ENTRY`.
Rust cache reuse transfers archive and sidecars under the same cache stripe.
The linker consumes this owned generation directly, so eviction after the
transfer cannot invalidate an in-flight build. The optional cache destination
is request transport, not persistent daemon state or codegen identity. It must
use the canonical shared-stdlib archive path. Required output is admitted
outside that cache root or under its universally protected canonical home
(including the home symlink target); a selected environment-specific home or
bin exception alone does not promise custody across later maintenance runs.
Hydration never invalidates the destination on lock-admission failure, and
failed-copy cleanup remains inside destination publication custody.
A single atomic constituent envelope retains
schema, input digest, output digest, exact length and object bytes. Publication
uses `molt_artifact_publish`; object validation uses the same target/format
contract as archive assembly. There is no separate sidecar generation to mix.

Only a missing generation is a cache miss. Malformed content, digest mismatch,
I/O errors, publication failures and divergent outputs for identical inputs
return errors. Admission verifies a constituent under its lock and keeps a
read-only mapping of it (`MappedObject`); archive assembly reads the object in
place, so no private copy is written. Publication never edits a generation in
place. On POSIX a mapping keeps its generation's bytes when retention unlinks
the entry; on Windows retention cannot delete a mapped entry until assembly
drops the mapping (not yet run on Windows). Cache errors never silently fall
back to compilation. Standalone backend invocations
without an admitted compiler manifest explicitly report reuse unavailable and
use the same compilation/assembly path.

Each archive miss reports object hits/misses, reused/emitted bytes and admission,
codegen and assembly time through the existing backend log stream. Measurements
must include cold process count, budget utilization and total wall time as well
as warm reuse; no throughput claim follows from cache counts alone.

Native batch failure artifacts now retain a self-contained replay job. No
external module-context file or path rewrite is needed after temporary cleanup.

## Environment custody

`src/molt/backend_environment.json` is the membership authority shared by CLI
cache variants, daemon request reset/forwarding, and native batch jobs. It replaces
both handwritten environment lists. Common pass controls, native emitter controls,
runtime callable content identity, and selected compiler identity are serialized
in each job with unset and empty values kept distinct. The worker verifies its
inherited settings before emission; replay artifacts record required values and
null removals. Runtime callable bytes are admitted by the existing digest owner.

The embedded catalog participates in every backend source fingerprint, including
native and WASM binary admission. Its Python reader and catalog also participate
in the broad CLI tooling fingerprint; backend configuration does not enlarge the
frontend lowering semantic scope.

Compilation dumps, audits and pass instruments bypass frontend artifact replay,
daemon output reuse, whole-archive and constituent reuse, and optimized TIR reuse.
Every boundary consults the same catalog, with presence (including an empty value)
requiring compilation. Explicit TIR dump/stats options also bypass optimized TIR
reuse. Diagnostic runs may still publish semantic artifacts for ordinary requests.
Timing and artifact-directory settings alone permit hits. The emitter's
inline-exception flag is read per operation; no process-global memo can disagree
with a later request.
