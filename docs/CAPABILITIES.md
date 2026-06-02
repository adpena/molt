# Molt Capability System

Molt uses a strict **capability-gating** system to control access to sensitive operations (OS, I/O, Network, Process) and to ensure portability between Native and WASM targets.

## The Principle

By default, a compiled Molt binary has **zero** capabilities. Any access to the outside world must be:
1.  **Declared** in the build manifest (`molt.toml` or CLI flags).
2.  **Verified** by the compiler at build-time.
3.  **Enforced** by the runtime at request-time.

## Current Capabilities

| Capability | Scope | Description |
| --- | --- | --- |
| `net` | Sockets, DNS, HTTP | Required for ASGI shims and network access. |
| `websocket.connect` | WebSockets | Allow outbound WebSocket connections. |
| `websocket.listen` | WebSockets | Allow WebSocket listener endpoints (planned). |
| `fs.read` | Filesystem | Read-only access to specific paths. |
| `fs.write` | Filesystem | Write access to specific paths. |
| `env.read` | Environment | Read environment variables. |
| `env.write` | Environment | Write environment variables. |
| `db.read` | Database | Allow database reads via `molt-worker`. |
| `db.write` | Database | Allow database writes via `molt-worker`. |
| `time.wall` | System Clock | Wall-clock access for `time.time`/`datetime`; monotonic/perf_counter use deterministic timers. |
| `time` | System Clock | Legacy alias for `time.wall`. |
| `random` | Randomness | Allow nondeterministic randomness (planned). |

## Built-in Profiles

Profiles are convenience aliases you can pass to `--capabilities`:

| Profile | Expands to |
| --- | --- |
| `core` | *(empty set)* |
| `fs` | `fs.read`, `fs.write` |
| `env` | `env.read`, `env.write` |
| `net` | `net`, `websocket.connect`, `websocket.listen` |
| `db` | `db.read`, `db.write` |
| `time` | `time` |
| `random` | `random` |

## Using Capabilities in Code

In Molt-compiled code (or shims), you check for capabilities using the `molt.capabilities` module:

```python
from molt import capabilities

def my_handler():
    # Throws PermissionError if "net" is not granted
    capabilities.require("net")
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
- `deny` removes capabilities from the global allowlist.
- `effects` is an allowlist for package effect annotations.
- `packages` provides per-package allow/deny/effects; package allowlists must be a subset of the global allowlist.
- `fs.read`/`fs.write` are derived from the `fs.read`/`fs.write` path lists.

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

- **Native**: Capabilities are enforced by the `molt-runtime` via standard OS call wrappers.
- **WASM**: Capabilities are enforced by the Host Interface (WIT). If a capability is missing, the host will trap or return an error to the guest.

## Security & Verified Binaries

The capability manifest is hashed and embedded into the binary's provenance metadata. This allows auditors to verify that a binary cannot perform unauthorized I/O without needing to decompile it.
