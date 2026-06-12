# Friend Benchmarks

Last updated: 2026-06-12

Friend-owned suites are configured by `bench/friends/manifest.toml` and executed with `tools/bench_friends.py`.

## Run

```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench_friends.py \
  --manifest bench/friends/manifest.toml \
  --dry-run
```

## Current State

- The published friend summary at `docs/benchmarks/friend_summary.md` was generated on 2026-02-12.
- All suites present in that February snapshot were skipped due to unavailable runnable lanes or adapter requirements.
- Treat that published summary as stale until a fresh run updates `docs/benchmarks/friend_summary.md`; later local runs, including enabled `tinygrad_off_the_shelf` evidence, remain local-only until regenerated with `tools/bench_friends.py --update-doc`.
- `tinygrad_off_the_shelf` is the canonical upstream tinygrad compatibility/perf
  lane. It is enabled and pinned to upstream commit
  `a83710396c991272241e40da94489747c2393851`. The `tinygrad` runner executes
  upstream `test/test_tiny.py` with `CHECK_OOB=0 DEV=CPU TYPED=1` through an
  isolated `uv --with typeguard` environment plus runner-local
  `PYTHONPATH={suite_root}` and `PYTHONDONTWRITEBYTECODE=1` so source custody
  stays clean; the CPython runner uses the same isolated `typeguard` dependency
  and bytecode-write ban while staying on
  `tools/tinygrad_off_shelf_adapter.py`, which is only a public-API workload
  driver. The Molt runner is executable by default with the full-stdlib
  static-package command. Current local evidence reaches the backend daemon and
  then trips the process RSS guard (`molt-backend --daemon` at 12.005 GB after
  435.5s; summary `tmp/memory_guard/friends_tinygrad_molt_sqlite_profile.json`),
  proving the blocker is backend compile memory rather than manifest skip or
  CLI profile propagation. Native TIR optimization now consumes uncached user
  functions in bounded op/count batches and applies each batch immediately; the
  next runner proof reached `2602` uncached user functions in `41` bounded
  batches, with a 9.77 GB peak single backend process, then exposed aggregate
  process-tree RSS from overlapping daemon plus hidden one-shot fallback. The
  CLI now treats full daemon request admission as the ownership boundary: after
  a full request is sent it fails closed on lost outcome instead of restarting
  the daemon or launching that hidden second backend compile. The follow-up
  runner proof (`bench/results/friends/20260612T203111Z/`,
  `tmp/memory_guard/friends_tinygrad_molt_daemon_custody.json`) no longer trips
  the outer memory guard (`violation=null`, no orphaned groups, 4.92 GB peak
  process-tree RSS); it fails explicitly with `Backend daemon compile failed:
  backend daemon died while request was in flight` after stdlib batch `35/35`
  and user-function TIR batch `8/41`. The later Molt-only rerun
  (`bench/results/friends/20260612T205850Z/`) stayed below the configured
  memory caps, recorded only protected host/control-plane exclusions in the
  sentinel log, and failed after 208.19s with `Backend daemon compile failed:
  backend daemon returned empty response`, so the current blocker is daemon
  crash provenance plus daemon-side large-user-compile memory custody.
- Fresh runs write ignored local artifacts under `bench/results/friends/`.
  Git checkout caches live under `bench/friends/repos/`; both roots are owned by
  the canonical cleanup allowlist.
  Do not commit one-off result bundles; publish durable summaries through
  `docs/benchmarks/friend_summary.md` only when the run is meant to become
  project evidence.
- `numpy_off_the_shelf` is the canonical upstream NumPy compile/probe lane. It
  is enabled and pinned to upstream NumPy commit
  `c81c49f77451340651a751e76bca607d85e4fd55` (the peeled `v2.4.2` commit).
  The `source_audit` runner verifies the pinned source tree as custody-only
  evidence, the `cpython` runner executes an isolated `numpy==2.4.2` public-API
  baseline through `tools/numpy_off_shelf_adapter.py`, and the `c_api_scan`
  runner executes the canonical `molt extension scan` directory source audit
  over `{suite_root}/numpy` with `--fail-on-missing`, using symbol statuses that
  separate `runtime_backed`, `source_compile_only`, `fail_fast`, and `missing`
  C-API usage. Non-workload runners are excluded from speedup metrics. The
  `molt` runner attempts the same adapter through
  `MOLT_EXTERNAL_STATIC_PACKAGES=numpy`, explicit
  `module.extension.exec` capability, and all-loaded-`numpy.*` module-origin
  custody. The Molt runner must fail loudly until source-recompiled `libmolt`
  extension package build/import custody and NumPy C-API symbol closure are
  complete.
