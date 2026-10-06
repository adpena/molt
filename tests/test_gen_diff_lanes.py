"""Shared lane inventory additions and sibling freshness regressions."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

from tools.generator_io import stale_outputs

ROOT = Path(__file__).resolve().parents[1]


def _generator(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    monkeypatch.syspath_prepend(str(ROOT / "tools"))
    spec = importlib.util.spec_from_file_location(
        "diff_lane_generator_test", ROOT / "tools/gen_diff_lanes.py"
    )
    assert spec is not None and spec.loader is not None
    gen = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(gen)
    for lane in ("basic", "stdlib", "pyperformance"):
        (tmp_path / "tests/differential" / lane).mkdir(parents=True)
    monkeypatch.setattr(gen, "ROOT", tmp_path)
    for name, path in {
        "COVERAGE_INDEX": "tests/differential/COVERAGE_INDEX.yaml",
        "CORE_MANIFEST": "tests/differential/basic/CORE_TESTS.txt",
        "STDLIB_MANIFEST": "tests/differential/stdlib/TESTS.txt",
        "PYPERFORMANCE_MANIFEST": "tests/differential/pyperformance/TESTS.txt",
    }.items():
        monkeypatch.setattr(gen, name, tmp_path / path)
    monkeypatch.setattr(gen, "CORE_EXTRA", set())
    gen.COVERAGE_INDEX.write_text(
        "core:\n  - tests/differential/basic/language.py\nstdlib:\n  - tests/differential/basic/language.py\npyperformance:\n",
        encoding="utf-8",
    )
    for name in (
        "basic/language.py",
        "stdlib/new_program.py",
        "pyperformance/new_benchmark.py",
    ):
        (tmp_path / "tests/differential" / name).write_text(
            "print(1)\n", encoding="utf-8"
        )
    return gen


def test_physical_additions_are_preserved_with_unique_lane_owners(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gen = _generator(tmp_path, monkeypatch)
    rendered = gen.generated_outputs()
    assert "tests/differential/stdlib/new_program.py" in rendered[gen.STDLIB_MANIFEST]
    assert (
        "tests/differential/pyperformance/new_benchmark.py"
        in rendered[gen.PYPERFORMANCE_MANIFEST]
    )
    assert "tests/differential/basic/language.py" in rendered[gen.CORE_MANIFEST]
    assert "tests/differential/basic/language.py" not in rendered[gen.STDLIB_MANIFEST]
    assert all("\\" not in value for value in rendered.values())
    assert rendered == gen.generated_outputs()


@pytest.mark.parametrize(
    "lane", ["CORE_MANIFEST", "STDLIB_MANIFEST", "PYPERFORMANCE_MANIFEST"]
)
def test_freshness_rejects_each_stale_sibling(
    lane: str, tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    gen = _generator(tmp_path, monkeypatch)
    rendered = gen.generated_outputs()
    for path, value in rendered.items():
        path.write_text(value, encoding="utf-8")
    assert stale_outputs(rendered) == []
    getattr(gen, lane).write_text("# stale\n", encoding="utf-8")
    assert stale_outputs(rendered) == [getattr(gen, lane)]
