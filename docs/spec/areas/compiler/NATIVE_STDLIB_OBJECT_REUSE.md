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
remain exact. The executing backend file's SHA256 and the effective Cranelift
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
