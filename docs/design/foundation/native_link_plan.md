# Native link planning

`src/molt/cli/native_link_plan.py` is the canonical native link policy
authority. A link is planned once as an immutable tuple of:

- resolved target OS, architecture, and object format;
- selected linker and its demonstrated capabilities;
- optimization policy (dead stripping, function identity, relocations,
  post-link stripping, and BOLT intent); and
- the exact command used by the fingerprint and executor.

Main executables and source-built native extensions consume the same dead-strip
and function-identity flags. User and upstream link arguments are inputs to the
plan, not permission to weaken its correctness invariants.

## Function identity

Molt runtime behavior can observe function-pointer identity. Until code objects
carry a separate stable identity independent of machine addresses, identical
code folding is disabled explicitly where the selected linker supports it:

| Format | Policy |
| --- | --- |
| COFF | `/OPT:NOICF` |
| Mach-O | `-no_deduplicate` |
| ELF with lld or mold | `--icf=none` |
| ELF system linker | no ICF flag; the default linker policy is no folding |

## BOLT

BOLT is a release-only Linux ELF post-link stage on x86_64 and aarch64. The
ordinary linker emits relocations and retains symbols. The pipeline merges all
PID-scoped training fragments, strips the optimized candidate, validates that
final candidate, and only then atomically publishes it. A failed finalization
therefore preserves the previously published artifact. Each BOLT run starts
from a fresh ordinary link because the link-input fingerprint does not describe
workload profile data.

Release stripping is target-aware: exact-host targets may use the resolved
native strip tool, while a foreign OS or architecture requires `llvm-strip`.
Missing or failed target stripping fails the release build instead of silently
publishing an artifact outside the plan.

Unsupported targets, missing tools, invalid optimized images, failed training,
and failed stripping are diagnostics, never silent skips. Custom training
commands must use `{binary}` to name the instrumented image.

The command is executed once. A selected linker failure or invalid output fails
the build; there is no post-failure retry through a different linker.

Every native rebuild links to a private candidate. Ordinary and BOLT builds
share one finalizer that applies target stripping when planned, validates the
final candidate, and atomically publishes it. A failed link, strip, or
validation therefore cannot overwrite or delete a previously valid output.

Fail-closed source guards keep the identity flag literals in this authority,
require executable and source-extension consumers to use the shared plan/policy,
and reject the return of post-failure linker retry lanes.
