# Friend Benchmarks

Last updated: 2026-03-19

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
