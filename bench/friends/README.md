# Friend Benchmarks

Last updated: 2026-06-11

Friend-owned suites are configured by `bench/friends/manifest.toml` and executed with `tools/bench_friends.py`.

## Run

```bash
UV_NO_SYNC=1 uv run --python 3.12 python3 tools/bench_friends.py \
  --manifest bench/friends/manifest.toml \
  --dry-run
```

## Current State

- The published friend summary at `docs/benchmarks/friend_summary.md` was generated on 2026-02-12.
- All listed suites were skipped in that snapshot due to unavailable runnable lanes or adapter requirements.
- Treat friend-suite status as stale until a fresh run updates `docs/benchmarks/friend_summary.md`.
- `tinygrad_off_the_shelf` is the canonical upstream tinygrad compatibility/perf
  lane. It is disabled until `repo_ref` is pinned to an immutable upstream
  commit, but its CPython and Molt runners are already wired through
  `tools/tinygrad_off_shelf_adapter.py` so dry runs expose the intended compile
  and benchmark commands.
