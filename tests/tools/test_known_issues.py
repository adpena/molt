"""tools/known_issues.py groups open rows by v0.1 class; unclassified rows hold."""

from __future__ import annotations

import pytest

from molt.release_matrix import TIER1_CELLS
from tools import known_issues

_LEDGER = """\
# Ledger

## Open: release blockers

| ID | Finding | Evidence | Next |
|----|---------|----------|------|
| HF-1 | Blocks the witness. More detail here. | E1. | N1. |
| HF-2 | Fails only on Windows. | E2. | N2. |
| HF-3 | Later work. | E3. | N3. |
| HF-4 | Nobody classified this. | E4. | N4. |

### Family dependency order (2026-10-29)

| Family | Stratum | HF-459 axes | Cause | Keys |
| --- | --- | --- | --- | --- |
| A. Alpha | engine | loop/per_op | cause a | HF-1, HF-2, HF-3, HF-4 |

| Row | V1 class |
| --- | --- |
| HF-1 | release_blocking |
| HF-2 | release_blocking |
| HF-3 | post_1_0 |
| HF-4 | release_blocking |

| Row | v0.1 class |
| --- | --- |
| HF-1 | v0_1_blocking |
| HF-2 | tier2 |
| HF-3 | post_v0_1 |

## Fixed after the handoff

| ID | Defect | Fix |
|----|--------|-----|
| HF-0 | Zero. | Fixed. |
"""


def _section(document: str, heading: str) -> str:
    return document.split(heading, 1)[1].split("\n## ", 1)[0]


def test_render_groups_rows_and_holds_unclassified_rows() -> None:
    document = known_issues.render(
        _LEDGER,
        source="a fixture",
        coordinates={cell.id: None for cell in TIER1_CELLS},
    )
    held = _section(document, "## Open rows that hold v0.1 (2)")
    assert "**HF-1**: Blocks the witness." in held
    assert "More detail" not in held
    assert "**HF-4**: Nobody classified this." in held
    assert "HF-2" not in held and "HF-3" not in held
    assert "**HF-2**" in _section(document, "## Tier 2 (1)")
    assert "**HF-3**" in _section(document, "## After v0.1 (1)")
    assert "HF-0" not in document


def test_render_names_each_tier1_cell_and_its_proof() -> None:
    first = TIER1_CELLS[0]
    document = known_issues.render(
        _LEDGER,
        source="a fixture",
        coordinates={cell.id: None for cell in TIER1_CELLS},
    )
    table = _section(document, "## Tier 1 cells")
    assert table.count("| `tier1-") == len(TIER1_CELLS)
    assert f"none: Molt does not admit target Python {first.python}" in table


def test_render_refuses_an_invalid_ledger() -> None:
    broken = _LEDGER.replace("| HF-3 | post_v0_1 |", "| HF-3 | someday |")
    with pytest.raises(ValueError, match="unknown class 'someday'"):
        known_issues.render(broken, source="a fixture", coordinates={})


def test_summary_keeps_the_first_sentence_within_bounds() -> None:
    assert known_issues.summary("One thing. Two `things`.") == "One thing."
    long = "x" * 400 + "."
    assert len(known_issues.summary(long)) == 240
