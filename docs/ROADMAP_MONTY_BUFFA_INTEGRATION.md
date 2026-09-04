# Monty and Buffa integration

**Status:** active research and integration frontier
**Authority boundary:** this roadmap does not define permissions, runtime tiers,
target support, ABI, or release claims.

Molt may reuse Monty's interpreter and sandbox ideas and Buffa's protobuf
implementation, but it must not duplicate Molt's existing compiler/runtime
authorities. The invariant is one resolved policy and one target/conformance
matrix across AOT native, WASI, browser, Cloudflare Workers, and any future
interpreter lane.

## Existing authorities

| Concern | Canonical authority |
| --- | --- |
| Built-in grants, permission tiers, audited operations, and target gates | `runtime/host_capabilities.toml` |
| Generated Python, Rust, browser, and documentation projections | `tools/gen_host_capabilities.py` |
| Policy algebra and package attenuation | `src/molt/capability_policy.py` |
| Strict v2 manifest envelope, resources, audit, I/O, mounts, and digest | `src/molt/capability_manifest.py` |
| Runtime enforcement and audit events | `runtime/molt-runtime` and `runtime/molt-runtime-audit` |
| Protobuf wire support | `runtime/molt-runtime-protobuf` |
| Snapshots | `runtime/molt-snapshot` |
| Compiler/runtime ABI | generated manifests under `runtime/`; never this roadmap |

The capability manifest intentionally has no Monty-specific section. Execution
strategy cannot grant host authority. If interpreter/AOT tier selection becomes
a product feature, it must have a separate typed execution-plan authority that
references the immutable resolved policy digest.

## Integration sequence

1. **Policy parity.** Adapt Monty host callbacks to consume Molt's resolved
   policy and generated operation IDs. Unsupported target cells fail before
   execution. No second grant registry, manifest parser, or broad compatibility
   token is allowed.
2. **Conformance.** Admit Monty tests as differential corpus inputs. Every
   mismatch is minimized into a replayable CPython-version/OS/architecture/
   execution-target receipt and fixed at the shared semantic authority.
3. **Buffa audit wire format.** Define one checked-in protobuf schema covering
   the complete runtime audit event, generate its projection, and provide a
   lossless production conversion. Benchmark binary size, encode/decode time,
   allocation count, and JSONL comparison before selecting a shipped sink.
4. **Optional interpreter lane.** Prove snapshot, exception, object ownership,
   resource, and audit-digest parity before introducing call counters or an
   atomic interpreted-to-compiled replacement. Tier-up thresholds and cache
   policy belong to the execution plan, not the permission manifest.
5. **Edge hosts.** Prove identical resolved policy payload/digest and actual
   denial behavior in WASI, browser, and Cloudflare Workers. Host callbacks are
   explicit capabilities; they are never ambient escape hatches.

## Acceptance matrix

A feature is complete only when one source revision has replayable receipts for
the applicable cells across:

- CPython 3.12, 3.13, and 3.14 semantics;
- Windows, macOS, Linux, and shipped architectures;
- native, WASI, browser, and Cloudflare Worker execution;
- allowed, denied, attenuated, invalid-policy, unsupported-target, and runtime
  reinitialization cases;
- deterministic output, audit policy digest, resource enforcement, performance,
  and binary-size budgets.

Monty and Buffa upstream revisions must be provenance-pinned. New upstream
behavior enters through adapters and differential evidence, never copied policy
tables or package-specific compiler/runtime shortcuts.
