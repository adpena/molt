# Molt Capability System

Molt uses capability gating to control host access and to keep native and WASM
policy behavior aligned. Capability grants are policy; linker symbols, typed
WASM imports, effects, and target support facts are evidence used to validate an
artifact, never alternate ways to grant authority.

## Authorities

The capability stack has one owner at each layer:

| Layer | Authority | Responsibility |
| --- | --- | --- |
| Built-in grant families | `runtime/host_capabilities.toml` | CLI profiles, runtime tiers, inheritance, and the ambientless explicit-policy tier |
| Generated projections | `src/molt/_host_capabilities_generated.py`, Rust runtime projection, browser projection | Byte-exact Python/native/WASM consumers; never edited by hand |
| Policy algebra and CLI input | `src/molt/capability_policy.py` | Profiles, token syntax, inline/list/file loading, allow/deny resolution, and per-package intersection |
| Manifest envelope | `src/molt/capability_manifest.py` | Strict versioned TOML/JSON/YAML loading, integrity digest, resources, I/O, and audit policy |
| Runtime checks | `src/molt/capabilities.py` plus runtime intrinsics | Query and require the resolved runtime grant set |

Do not add capability profiles, runtime tiers, package-scope parsing, or allow/deny semantics to
artifact manifests, linker closure, WASM import inspection, a backend, or a host
adapter. Those consumers submit facts to the policy authority; they do not
reinterpret policy.

`runtime/python_effects.toml` uses the word *capability* for compile-time proofs
such as `cannot_raise`. Those monotone effect projections are not host
permissions and cannot grant I/O; the two algebras stay deliberately distinct.

## The Principle

Sandboxed and manifest-custodied artifacts are default-deny: a host operation is
available only when the host grants its exact capability. A grant must be:

1. **Declared** through the capability policy (`molt.toml`, a capability
   manifest, or CLI input).
2. **Validated** before build, package admission, or execution.
3. **Enforced** at the native/WASM host boundary.

The runtime's `safe`, `standard`, and `full` tiers are explicit convenience
grant sets, not extra policy models. `molt build` and `molt run` both default to
the generated ambientless `none` tier; host authority exists only when a tier,
capability list, or capability manifest grants it. Explicit capabilities are
resolved from that same ambientless base before `deny` is applied, so a
convenience tier cannot reintroduce denied authority. Browser hosts follow the
same rule unless the embedding host explicitly supplies a tier.

This follows the useful boundary in
[Monty's security model](https://github.com/pydantic/monty/blob/main/docs/security.md):
the guest cannot manufacture host authority; host callbacks remain as powerful
as the host makes them; filesystem confinement belongs at the mount/descriptor
boundary; worker messages and snapshots are untrusted inputs; and resource
limits need host-level backstops. Molt keeps its existing compiler/runtime
policy and AOT extension/ABI model rather than embedding or duplicating Monty's
interpreter implementation.

## Generated registry

The exact built-in capability IDs, profile expansions, runtime tiers,
operation-to-capability requirements, and target/platform/architecture gates
are generated together in
[`host_capabilities.generated.md`](spec/areas/security/host_capabilities.generated.md).
Edit only `runtime/host_capabilities.toml`; the generator updates the Python,
Rust, browser, tests, and human-readable projections together.

## Using Capabilities in Code

In Molt-compiled code (or shims), you check for capabilities using the `molt.capabilities` module:

```python
from molt import capabilities

def my_handler():
    # Throws PermissionError if outbound connection authority is not granted.
    capabilities.require("net.connect")
    ...
```

## Build-Time Configuration

You grant capabilities during the `build` or `run` command:

```bash
# Granting network and environment access
molt build --capabilities net,env main.py
```

Alternatively, use a manifest file:

```json
{
  "allow": ["net", "time"],
  "deny": ["fs.write"],
  "effects": ["nondet"],
  "fs": {
    "read": ["/tmp/data"],
    "write": []
  },
  "packages": {
    "molt_test_pkg": {
      "allow": ["net"],
      "effects": ["nondet"]
    }
  }
}
```
`molt build --capabilities profile.json main.py`

Notes:
- `allow` accepts explicit capability tokens or built-in profiles (e.g. `net`, `fs`).
- Profiles expand first; `deny` then removes exact capabilities from the global allowlist.
- `effects` is an allowlist for package effect annotations.
- `packages` provides per-package allow/deny/effects. A package allowlist must be
  a subset of the resolved global allowlist; omission inherits the global set,
  while an explicit empty list grants none.
- Non-empty `fs.read`/`fs.write` entries request the corresponding broad
  permission. Path confinement is owned by virtual mounts and the host adapter;
  reducing a path list to a token does not itself create a filesystem sandbox.
- Capability tokens are namespace-extensible and syntax-validated so future
  platforms and ecosystem packages can add narrow tokens without editing a
  stale closed registry.

Tooling enforces capability/effect allowlists during `molt package` and `molt verify`.

## Memory and Resource Limits

Beyond capability tokens, a manifest can constrain *how much* a program may
consume (memory, time, allocations, recursion depth, and per-operation result
sizes) via a `[resources]` table — see `docs/RESOURCE_CONTROLS.md` for the full
schema. These limits are enforced by the in-VM `ResourceTracker`, shared by all
backends.

For memory specifically, a compiled binary can also cap itself at run time
through the ergonomic `MOLT_MEMORY_LIMIT` env var (human sizes like `64M`,
`2G`), which is an **alias** that resolves into the same single
`ResourceLimits.max_memory` enforcement path as the manifest-emitted
`MOLT_RESOURCE_MAX_MEMORY` — there is no parallel limit system:

```bash
# Cap the binary at 64 MiB; a runaway raises an uncatchable MemoryError
# instead of OOM-killing the host.
MOLT_MEMORY_LIMIT=64M ./my_app
```

Enforcement is two-layer: the precise in-VM tracker (deterministic, identical
across native/WASM/LLVM/Luau) plus, on native, an OS-level `RLIMIT_AS` backstop
that bounds anything the tracker cannot see. This protection is **opt-in** (no
default limit unless configured); capability-tier (deployment-profile) defaults
are deferred. A misconfigured limit fails loudly at init rather than being
silently ignored.

## Native vs WASM Parity

- **Native**: `molt-runtime` resolves the generated tier plus explicit grants and
  checks them at OS/runtime boundaries.
- **WASM**: the same generated tier identity and grants enter through the host
  environment; browser hosts default to the ambientless tier. Target support
  and host-import admission remain separate from permission grants.

## Security & Verified Binaries

The capability manifest is canonically hashed and embedded into provenance.
Loading, semantic decoding, and digest verification use one immutable byte
snapshot, preventing a path mutation from mixing policy A with digest B. The
embedded `sha256:` field is an integrity checksum, not signer authentication;
artifact trust must come from the repository's signer/trust-policy authority.
