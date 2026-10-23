# Native stdlib constituent object reuse

The shared stdlib archive remains the reachability, runtime admission and final
publication authority. An archive miss compiles only incompatible constituent
objects, then passes both restored and newly emitted ordinary objects through
the existing native archive validator/assembler and archive publisher.

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
bind it. An import that leaves a constituent's closed job unchanged therefore
keeps that object. Adding another retained body from the same source can change
the batch and require recompilation, even when its existing bodies did not
change. The executing backend file's SHA256 and the effective Cranelift
target/shared/ISA flags plus host TIR SIMD facts are additional inputs. LLVM
builds bind the host triple, CPU and features used by its target-machine owner.
Transport paths and timestamps are never content identity. The existing CLI
runtime-codegen binding still owns file verification and worker environment
inheritance; constituent reuse does not relax or replace that admission.

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

## Module production versus application reachability

Source grouping does not yet establish independently compiled modules.
`prune_and_partition_native_stdlib` eliminates unreachable functions before
forming cached stdlib jobs. A source group containing `f` can therefore become
a different object containing `f` and `g` after an application-root change.
Published Python functions may already both be rooted by their initializer;
this cost applies when the retained body set actually changes, not to every
additional attribute call. Frontend native-support slicing also removes
definitions and imports before lowering. The backend cannot reconstruct a
complete provider module from those sliced inputs.

The existing frontend producer now carries a required `ModuleBodySelection`:
`complete_source` or `native_support` with nonempty canonical binding roots.
Discovery, source-scan merging, AST selection, scoped lowering, worker transport,
and persisted scan/analysis/lowering records consume this same selection.
Application import requests may still supply optional support roots; admission
converts them to the typed selection. An absent serialized selection is never
interpreted as complete source. Current cache schemas reject retired records,
and lowering replay checks the selection against the request before decoding
function bodies or refreshing the record. A support selection containing every
definition still differs from complete source: it removes unrelated imports,
top-level statements and decorators.

This is source-production authority, not a certificate that every body is
target-lowerable or that the resulting provider closes over its dependencies.
It adds no body retention, new process, filesystem object, optimizer or object
cache. Record identity changes once with the schema; steady-state selection
comparison is proportional to the selected roots. The next migration must
transport the admitted lowered module inventory and its existing reference/ABI
dependencies before partitioning. Only then can application reachability select
final sections without changing the producer inventory. V1-07 remains open;
the typed selection alone establishes no compile-time or reuse improvement.

The intended boundary is one admitted, closed module producer independent of
application export selection, with application roots selecting final sections.
Reuse the existing module payload, projected ABI, reference edges, object cache
and linker; no body-only cache key or second compiler pipeline is needed.
This follows [rustc's separation of stable module units from volatile instances](https://doc.rust-lang.org/nightly/nightly-rustc/src/rustc_monomorphize/partitioning.rs.html)
and [ThinLTO's separate module summaries and cached backend work](https://clang.llvm.org/docs/ThinLTO.html).
Neither mechanism permits reuse after a consumed body, signature or imported
dependency changes.

Moving the current elimination pass alone would change admission. Provider
closure validation rejects references to application-defined functions, even
when the referring body is uncalled. The application resolver currently also
consumes the incoming body set. Retaining more bodies can introduce new
resolver roots, unavailable intrinsics or code that is not lowerable on the
selected target. A complete migration must supply target-lowerable closed
provider units first, derive resolver and feature demand from the application
reached view, and migrate both partition/externalization producers together.
Module initializers and published/address-taken functions remain real roots.

The decisive witness uses one unchanged provider with `f` and `g`: applications
selecting `f`, then `f` and `g`, must reuse its object while producing correct
outputs and final symbol sets. Body and ABI changes must invalidate reuse;
dead unavailable-intrinsic and provider-to-application edges must preserve the
declared admission boundary. Verify actual native linking and the distinct
LLVM/WASM export/table contracts before expanding the claim. Measure cold
lowering, object count, memory and disk costs alongside warm reuse: eagerly
compiling all source modules or emitting one process/object per function is
not an accepted substitute. This producer migration remains open under V1-07.

Even zero- and one-body archives use this same path. An empty partition emits
one real native object containing the ABI anchor. The former direct-only
stdlib compiler path is removed.

## Admission and publication

The canonical archive and sidecars live in
`<cache>/shared-stdlib/<archive-key>/stdlib.a`. Constituent generations live in
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
return errors. Admission copies validated bytes to the job's private ordinary
object file, so existing archive assembly and cleanup retain custody. Cache
errors never silently fall back to compilation. Standalone backend invocations
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
