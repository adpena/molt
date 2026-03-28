# Cloudflare Demo Hardening Design

## Goal

Make the Cloudflare Workers demo build, deploy, and verify reliably on the
latest supported toolchains, then enforce production live-endpoint validation
 against the real Cloudflare worker as a release gate.

## Current Grounded Status (2026-03-28)

Reproduced facts from this session:

- The current production endpoint at
  `https://molt-python-demo.adpena.workers.dev` is not aligned with
  `examples/cloudflare-demo/src/app.py`.
- Production serves behavior that is stale or from a different artifact:
  - `/diamond/21` and `/fizzbuzz/30` return 404-style output even though those
    routes exist in source.
  - `/fib/100`, `/primes/1000`, `/pi/100000`, and `/generate/1` return
    `HTTP 400` with body `Error`.
  - `/bench`, `/sort`, and `/sql` intermittently fail with Cloudflare
    `error code: 1102`.
  - `/sql` can emit leading NUL bytes in the body when it does respond.
- The current generated split-runtime bundle from `molt build ... --profile cloudflare --split-runtime`
  emits a `wrangler.jsonc` module-worker config with explicit `rules`,
  `no_bundle`, and `find_additional_modules`. The live verifier now targets
  this shape directly.
- The generated `worker.js` in the current repo does not match the behavior
  currently observed in production, proving artifact-contract drift between
  generated output and live deployment.

## Problem Statement

The Cloudflare demo currently fails in three coupled layers:

1. **Artifact contract drift**
   Generated Cloudflare artifacts are not expressed in the current Wrangler
   module contract, and production appears to be running a stale or divergent
   worker bundle.

2. **Runtime endpoint unreliability**
   Multiple documented endpoints fail in production, including `/generate/N`,
   and some outputs are malformed.

3. **No trustworthy deploy gate**
   Deploy success is not currently tied to live production endpoint validation,
   so stale deployments and runtime regressions can ship undetected.

## Non-Negotiable Requirements

1. Use current supported dependencies and toolchains for the Cloudflare deploy
   path, especially Wrangler/module-worker compatibility.
2. Preserve one canonical generated artifact contract for local dev, deploy,
   and verification.
3. Treat production deploy as incomplete until live endpoint verification
   passes against the real Cloudflare worker URL.
4. Store reproducible evidence under canonical roots only (`logs/`, `tmp/`,
   `target/`).
5. Do not paper over runtime failures with endpoint-specific hacks or
   test-only branches.

## Recommended Approach

Adopt a production-first hardening program with three coordinated tracks:

### Track A: Canonical Cloudflare Artifact Contract

Update Cloudflare bundle generation so `molt build --target wasm --profile cloudflare --split-runtime`
emits artifacts compatible with the latest Wrangler module-worker model.

This includes:

- a module-worker-compatible config contract;
- explicit module attachment rules for generated JS/WASM assets;
- deterministic metadata for deploy and verification tooling;
- removal of legacy assumptions that only work with older Wrangler behavior.

This track establishes a single source of truth for:

- local `wrangler dev`;
- production `wrangler deploy`;
- post-deploy verification.

### Track B: Runtime Endpoint Parity, Input Hardening, And Output Integrity

Harden the demo runtime path so every documented endpoint in
`examples/cloudflare-demo/src/app.py` behaves consistently under:

- direct CPython execution of the source app;
- generated Cloudflare bundle under current local dev tooling;
- deployed production worker.

Endpoint hardening scope includes:

- documented route parity;
- correct numeric/argument handling;
- adversarial input handling for every variable-bearing endpoint;
- explicit bounds enforcement and fail-closed validation;
- parser hardening for query/path-driven surfaces;
- no malformed body prefix bytes;
- no Cloudflare `1102` runtime failures for supported routes;
- deterministic route identification between generated artifact and observed
  production behavior.

`/generate/N` is an explicit must-pass route, but it is not treated as a
special-case fix. The whole endpoint surface must be reliable.

Variable-bearing endpoint scope includes both:

- **Externally reachable request surfaces**
  path parameters, query parameters, and any request-body inputs now or later
  accepted by the demo.

- **Internal consumer surfaces**
  any parser, formatter, or dynamic rendering path that consumes those inputs,
  including SQL query parsing and generated-content formatting.

### Track C: Production Deploy And Live Verification Gate

Extend deploy tooling so production release is gated by a real live-endpoint
verification suite against the actual Cloudflare worker URL.

The verification suite must:

- run automatically after deploy;
- probe the full endpoint matrix;
- validate status code, content type, and body sentinels;
- detect stale route behavior;
- detect malformed output such as NUL-prefixed bodies;
- capture logs/artifacts under canonical roots;
- fail the deploy flow if any production verification step fails.

## Alternatives Considered

### 1. Runtime-only patching

Fix `worker.js` and endpoint behavior without changing the generated Cloudflare
artifact contract.

Rejected because it leaves the latest-Wrangler incompatibility unresolved and
does not address the proven drift between generated artifacts and production.

### 2. Deploy-wrapper-only hardening

Improve deploy and post-deploy verification while minimizing bundle/runtime
changes.

Rejected because the current generated bundle already fails the latest local
toolchain contract, so deploy-only hardening would still rest on unstable
artifacts.

### 3. Full-stack hardening with production live verification

Upgrade the artifact contract, runtime compatibility, and deployment gate
together.

Accepted because it is the only approach that closes the entire failure loop.

## Architecture

The Cloudflare demo path should become an explicit three-stage pipeline:

1. **Build**
   `molt build` emits the worker bundle, config, and machine-readable metadata.

2. **Validate**
   Local tests verify the generated bundle on the current toolchain and compare
   endpoint behavior against source expectations.

3. **Deploy + Verify**
   Production deploy publishes the exact generated bundle and immediately runs a
   live verification sweep against the real worker URL.

The architectural principle is that build output, deploy input, and
verification expectations must all derive from the same generated contract
rather than being reconstructed by wrapper-local heuristics.

## Components

### `src/molt/cli.py`

Responsibilities:

- generate current-toolchain Cloudflare artifacts;
- emit machine-readable metadata for deploy/verify consumers;
- fail clearly when generated Cloudflare config is invalid for the selected
  toolchain.

### Cloudflare bundle output

Responsibilities:

- represent the exact module worker contract;
- attach JS/WASM assets compatibly with current Wrangler;
- provide deterministic asset layout for local dev and deploy.

### Demo application surface

Responsibilities:

- preserve documented endpoints;
- produce valid text/HTML output with no corruption;
- behave consistently across source execution and deployed worker execution.

### Deploy and verification tooling

Responsibilities:

- deploy the exact artifact set;
- verify production immediately after rollout;
- archive evidence under canonical artifact roots.

### Tests and docs

Responsibilities:

- encode the artifact contract and endpoint parity rules;
- keep docs aligned with the actual deploy procedure.

## Data Flow

1. Build command emits Cloudflare bundle plus metadata.
2. Local validation proves the bundle is accepted by the latest toolchain.
3. Local endpoint sweep validates the route matrix on the generated bundle.
4. Deploy publishes that exact bundle to the real Cloudflare worker.
5. Production verification probes the live URL and asserts:
   - expected status codes;
   - expected content types;
   - expected body sentinels;
   - no stale route behavior;
   - no `1102`;
   - no output corruption.
6. Deploy is marked successful only if the production verification suite passes.

## Error Handling

Required failure classes and responses:

- **Build-contract error**
  Generated Cloudflare config/bundle is incompatible with current toolchain.
  Response: fail build/deploy with explicit contract diagnostics.

- **Local-runtime validation error**
  Bundle starts but endpoint matrix deviates from expected behavior.
  Response: fail pre-deploy validation.

- **Production stale-artifact error**
  Live worker behavior does not match the generated artifact contract.
  Response: fail deploy verification and record route-by-route evidence.

- **Production runtime error**
  Live worker returns `1102`, malformed output, or unexpected error payloads.
  Response: fail deploy verification with endpoint-specific evidence.

## Testing Strategy

### Contract tests

Add tests that verify generated Cloudflare config and bundle metadata conform to
the latest supported module-worker expectations.

### Local integration tests

Add tests that validate the generated bundle under the current Wrangler/toolchain
contract and prove that the demo starts locally.

### Endpoint parity tests

Add route-matrix tests covering at least:

- `/`
- `/fib/N`
- `/primes/N`
- `/diamond/N`
- `/mandelbrot`
- `/sort?...`
- `/fizzbuzz/N`
- `/pi/N`
- `/generate/N`
- `/bench`
- `/sql`
- `/demo`

These must validate both success behavior and output integrity.

### Fuzzing and abuse-hardening tests

All variable-bearing endpoints must be fuzzed and hardened. This includes:

- numeric path parameters;
- query-string parsing;
- SQL query input handling;
- comma-separated list parsing for sort-style endpoints;
- generated-content formatting paths;
- malformed UTF-8 / percent-decoding edge cases where applicable at the HTTP
  boundary or parser boundary.

The fuzzing program should be production-minded, not synthetic benchmark
theater. It should emphasize:

- invalid types and malformed encoding;
- oversized values and boundary extremes;
- route confusion and delimiter abuse;
- parser ambiguity and truncation cases;
- denial-of-service shaped inputs that could trigger excessive CPU, memory, or
  output growth.

For security and operational correctness, hardening must also define endpoint
behavior for rejected inputs:

- explicit validation outcome;
- bounded work;
- deterministic status/body behavior;
- no crashes, hangs, `1102`, or malformed partial responses.

### Production live verification

Add a production sweep that runs after deploy against the real worker endpoint
and writes machine-readable results plus human-readable logs under canonical
artifact roots.

## Acceptance Criteria

The work is complete only when all of the following are true:

1. The generated Cloudflare bundle is accepted by the latest supported Wrangler
   module-worker path.
2. Local validation of the generated bundle passes.
3. The documented endpoint matrix passes locally.
4. Production deploy publishes the intended artifact set.
5. Production live verification passes for the real worker URL.
6. `/generate/N` passes in production as part of the general route matrix.
7. Variable-bearing endpoints are fuzzed with adversarial input coverage and
   remain bounded, deterministic, and crash-free.
8. No malformed output corruption remains.
9. Docs and operational instructions are updated in the same change.

## Risks

- Latest Wrangler behavior may require a generated artifact contract change that
  affects existing demo docs and deploy assumptions.
- Production verification introduces real-account dependencies and secret/env
  requirements that must be explicit and deterministic.
- Current production may be running a stale or divergent worker path; the deploy
  workflow must prove artifact identity after rollout rather than assume it.
- Fuzzing may expose latent parser/runtime issues outside `/generate/N`; the
  implementation plan should assume multi-endpoint fixes rather than a single
  endpoint patch.

## Out Of Scope

- General Cloudflare platform support for arbitrary user apps beyond the demo
  and current deploy contract.
- Broad wasm runtime refactors unrelated to demonstrable Cloudflare demo
  reliability issues.
- CI account provisioning policy beyond what is needed to run authenticated
  production deploy verification.
