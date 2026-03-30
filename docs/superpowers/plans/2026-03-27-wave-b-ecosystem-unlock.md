# Wave B: Ecosystem Unlock Residual Plan

> Audited on 2026-03-30. The original plan overstates how much is still missing: `six` coverage and much of the targeted stdlib intrinsic tranche already landed. The remaining closure work is the actual third-party ecosystem gate.

## Audit outcome

- Already landed:
  - `tests/differential/basic/import_six.py`
  - focused stdlib coverage for the originally targeted `functools`, `itertools`, `operator`, `math`, and `json` tranches.
- Still incomplete:
  - there is no focused `click` differential import/decorator regression in `tests/differential/basic/`;
  - there is no focused `attrs` end-to-end regression in `tests/differential/basic/`;
  - any runtime/frontend issues exposed by those packages are therefore still unclosed.

## Parallel tracks

### Track B1 - `click` import and decoration lane (depends on Wave A exit gate)

- Add a focused `tests/differential/basic/import_click.py` regression that proves import plus decorator wiring succeeds without invoking the full CLI machinery.
- Fix only the backend/frontend/runtime behavior that the test exposes.
- Validation:
  - `MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_click.py --jobs 1`

### Track B2 - `attrs` end-to-end lane (depends on Wave A exit gate)

- Add a focused `tests/differential/basic/import_attrs.py` regression and one deeper `attrs` behavior exercise.
- Fix only the concrete runtime or lowering gaps surfaced by those tests.
- Validation:
  - `MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_attrs.py --jobs 1`

### Track B3 - Ecosystem regression consolidation (can run while B1/B2 are underway, completes after them)

- Keep `six`, `click`, and `attrs` on one small differential matrix.
- Any missing intrinsic or reusable runtime primitive found during B1/B2 must land as the real fix, not as a package-specific shim.
- Validation:
  - `MOLT_DIFF_MEASURE_RSS=1 MOLT_BACKEND_DAEMON=0 uv run --python 3.12 python3 -u tests/molt_diff.py tests/differential/basic/import_six.py tests/differential/basic/import_click.py tests/differential/basic/import_attrs.py --jobs 1`

## Exit gate

- `six`, `click`, and `attrs` all have focused differential coverage.
- The small ecosystem matrix passes without package-specific hacks.
- If the full gate passes, delete this plan on the next audit pass.
