# Perf board history (doc 64 Phase 4)

Durable, content-addressed history of the five projected perf boards, and the
substrate for the **board-vs-history regression gate** (the second
Performance-Constitution triage axis: "any previously-green benchmark that
regressed").

## Layout

```
history/
  <board>/                 # cpython / backend / profile / pypy / codon
    index.json             # queryable trail: every recorded entry for this board
    <gitrev>_<id>.json     # one recorded board snapshot, content-addressed
```

- `<board>` is one of the five projections emitted by `tools/perf_board.py`.
- `<id>` is the first 12 hex of `board_identity = sha256(git_rev ||
  methodology_sha256 || suite_hash || host_class)` (`tools/perf_history.py`).
- `index.json` records `{board, identity, identity_class, git_rev, generated_at,
  authoritative, board_state, path}` per entry, sorted by `generated_at`.

## How the gate uses it

`tools/perf_history.py --gate` finds the **most recent promoted** history
entry of the **same identity class** (methodology version from
`config/perf_methodology.toml` + suite + host; git_rev may differ, and that
comparability is the point) and compares the candidate board against it:

- a cell that gated `PASS` in the baseline and `FAIL` now is an **error**
  regression → the gate exits nonzero (CI fails);
- a cell still passing but materially slower (> 5%) is a **warn** drift
  (advisory, does not block);
- a within-threshold delta is **not** flagged (the no-false-positive rule).

## Promotion (Rule 2)

CI never writes here: its checkout is discarded. A baseline enters history only
through an explicit, reviewed promotion of a perf-gate board:

```bash
gh run download <perf-gate run id> -n cpython-floor-scoreboard-native -D /tmp/perf-native
python3 tools/perf_history.py --promote /tmp/perf-native/bench/scoreboard/plane_native/cpython_*.json /tmp/perf-native/bench/scoreboard/plane_native/backend_*.json /tmp/perf-native/bench/scoreboard/plane_native/profile_*.json
```

`--promote` refuses a board that is not authoritative (a clean tree at a
revision on origin/main, measured quiescent), whose methodology is not locked to
its version, or whose revision is not an ancestor of origin/main. Land the new
files like any change. A class with no promoted baseline makes the gate report
UNARMED (a GitHub warning in CI); the absolute CPython floor in
`tools/perf_board.py` still catches a brand-new red.
