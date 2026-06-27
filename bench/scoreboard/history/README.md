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
  benchmark_tool_blob || suite_hash || host_class)` (`tools/perf_history.py`).
- `index.json` records `{board, identity, identity_class, git_rev, generated_at,
  authoritative, board_state, path}` per entry, sorted by `generated_at`.

## How the gate uses it

`tools/perf_history.py --gate` finds the **most recent authoritative** history
entry of the **same identity-class** (same tool + suite + host; git_rev may
differ — that comparability is the point) and compares the candidate board
against it:

- a cell that gated `PASS` in the baseline and `FAIL` now is an **error**
  regression → the gate exits nonzero (CI fails);
- a cell still passing but materially slower (> 5%) is a **warn** drift
  (advisory, does not block);
- a within-threshold delta is **not** flagged (the no-false-positive rule).

## Authority (Rule 2)

Only **authoritative** boards (clean tree == origin/main, tool unmodified,
measured quiescent) ever become a baseline. A non-authoritative board (a
contended CI runner, a dirty local tree) is recorded for the trail but never
selected as the regression reference — a noisy source cannot certify a target.
The history therefore starts empty and **arms itself** on the first authoritative
`main` run of `.github/workflows/perf-gate.yml`; until then the regression gate
reports "no baseline" and is silent (the absolute CPython floor in
`tools/perf_board.py` is what catches a brand-new red).
