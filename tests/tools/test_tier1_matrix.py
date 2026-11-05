"""The Tier 1 support cells: one TOML authority, one typed projection.

The oracle is the owner decision of 2026-11-05 (docs/design/v0_1_support_matrix.md):
Tier 1 is the Falcon OCR witness on macOS arm64 (wasm32-wasip1, wasm-release)
and the Django offload witness on Linux x86_64 (native Cranelift,
release-output), each for Python 3.12 and 3.15.
"""

from __future__ import annotations

import tomllib
from dataclasses import fields
from pathlib import Path
from typing import get_args

import pytest

from molt import release_matrix
from molt.release_matrix import TIER1_CELLS, Tier1Cell, Tier1Execution, Tier1Witness
from molt.verified_subset import tier1_coordinates
from tools import gen_release_matrix as gen

ROOT = Path(__file__).resolve().parents[2]


def _authority_with(tmp_path: Path, **overrides: str) -> Path:
    """A copy of the real authority whose first Tier 1 row takes the overrides."""
    text = (ROOT / "config" / "release_targets.toml").read_text(encoding="utf-8")
    document = tomllib.loads(text)
    first = document["tier1_cell"][0]
    head, marker, tail = text.partition("[[tier1_cell]]")
    assert marker
    row, _, rest = tail.partition("\n\n")
    for key, value in overrides.items():
        assert key in first, key
        row = row.replace(f'{key} = "{first[key]}"', f'{key} = "{value}"', 1)
    path = tmp_path / "release_targets.toml"
    path.write_text(head + marker + row + "\n\n" + rest, encoding="utf-8")
    return path


def test_generated_tier1_cells_are_the_owner_decision() -> None:
    claimed = {
        (cell.witness, cell.host, cell.python, cell.backend, cell.artifact_target)
        for cell in TIER1_CELLS
    }
    assert claimed == {
        ("falcon-ocr-workers", "macos-arm64", "3.12", "wasm", "wasm32-wasip1"),
        ("falcon-ocr-workers", "macos-arm64", "3.15", "wasm", "wasm32-wasip1"),
        ("django-offload", "linux-x86_64", "3.12", "native", "x86_64-unknown-linux-gnu"),
        ("django-offload", "linux-x86_64", "3.15", "native", "x86_64-unknown-linux-gnu"),
    }
    for cell in TIER1_CELLS:
        assert cell.guest_profile == "release"
        assert (cell.backend, cell.runtime_profile, cell.execution) in {
            ("wasm", "wasm-release", "workerd"),
            ("native", "release-output", "host"),
        }


def test_tier1_types_are_closed() -> None:
    assert set(get_args(Tier1Witness)) == {"django-offload", "falcon-ocr-workers"}
    assert set(get_args(Tier1Execution)) == {"host", "workerd"}
    assert {field.name for field in fields(Tier1Cell)} >= {"id", "witness", "execution"}


def test_generated_module_matches_the_authority() -> None:
    expected = gen.render_release_matrix(gen.load_release_target_authority())
    assert Path(release_matrix.__file__).read_text(encoding="utf-8") == expected


@pytest.mark.parametrize(
    ("overrides", "message"),
    [
        ({"host": "solaris-sparc"}, "host is not a release target id"),
        ({"python": "3.11"}, "no pinned CPython source"),
        ({"backend": "luau"}, "not an admitted release backend"),
        ({"witness": "pact"}, "unknown witness"),
        ({"execution": "browser"}, "unknown execution host"),
        ({"runtime_profile": "wasm-release"}, "is not a lane"),
        ({"execution": "workerd"}, "workerd runs exactly the wasm backend"),
    ],
)
def test_generator_refuses_an_invalid_tier1_row(
    tmp_path: Path, overrides: dict[str, str], message: str
) -> None:
    # The first row (sorted by derived id) is the Django native cell.
    path = _authority_with(tmp_path, **overrides)
    with pytest.raises(ValueError, match=message):
        gen.load_release_target_authority(path)


def test_generator_refuses_unsorted_or_duplicate_rows(tmp_path: Path) -> None:
    text = (ROOT / "config" / "release_targets.toml").read_text(encoding="utf-8")
    head, marker, tail = text.partition("[[tier1_cell]]")
    rows = [marker + row for row in tail.split("[[tier1_cell]]")]
    path = tmp_path / "release_targets.toml"
    path.write_text(head + "".join(reversed(rows)), encoding="utf-8")
    with pytest.raises(ValueError, match="sorted by derived id"):
        gen.load_release_target_authority(path)
    path.write_text(head + rows[0] + rows[0] + "".join(rows[1:]), encoding="utf-8")
    with pytest.raises(ValueError, match="sorted by derived id"):
        gen.load_release_target_authority(path)


def test_tier1_cells_map_to_verified_subset_coordinates() -> None:
    mapped = tier1_coordinates()
    assert set(mapped) == {cell.id for cell in TIER1_CELLS}
    for cell in TIER1_CELLS:
        coordinate = mapped[cell.id]
        if cell.python == "3.15":
            # Molt does not admit target 3.15 yet (V1-35): the gate must fail
            # this cell, so it has no coordinate rather than a stand-in.
            assert coordinate is None
            continue
        assert coordinate is not None
        assert (coordinate.platform, coordinate.arch, coordinate.python) == (
            cell.platform,
            cell.arch,
            cell.python,
        )
        assert (coordinate.backend, coordinate.build_profile) == (
            cell.backend,
            cell.guest_profile,
        )
        assert coordinate.concurrency == "gil"
