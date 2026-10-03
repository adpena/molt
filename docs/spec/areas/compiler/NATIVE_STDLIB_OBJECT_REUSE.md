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

Stdlib batching orders symbols by a domain-separated SHA256 and starts with one
radix bucket. A bucket splits on further hash bits only when the existing
function-count or operation budget requires it. Siblings never absorb overflow
from another bucket. Ordering within each final bucket is by exact symbol name;
input traversal order is irrelevant. There is no fixed high worker-count floor
or module-name classifier. A budget change is allowed to change batches. A
reachability change can split or coalesce its bucket but does not repack all
subsequent functions. A single oversized function remains one bounded compiler
input under the existing megafunction authority.

Even zero- and one-body archives use this same path. An empty partition emits
one real native object containing the ABI anchor. The former direct-only
stdlib compiler path is removed.

## Admission and publication

Constituent generations live under `native-stdlib-objects-v1` beside the shared
archive, partitioned by their complete input digest. The existing stdlib
publication lock owns each input generation. A single atomic envelope retains
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
