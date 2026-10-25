from __future__ import annotations

import sys
from pathlib import Path

from tests.surface_process_guard import run_surface_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
_PROBE = f"""
from pathlib import Path
from tools.profile_typing_lazy_once import typing_source_modules

with typing_source_modules(Path({str(REPO_ROOT)!r})) as (_typing, _private):
    rows = [
        (name, type(getattr(_private, name)).__name__, bool(callable(getattr(_private, name))))
        for name in sorted(dir(_private))
        if not name.startswith("_") and name != "annotations"
    ]
    for name, type_name, is_callable in rows:
        print(f"ROW|{{name}}|{{type_name}}|{{is_callable}}")

    checks = {{
        "behavior": (
            hasattr(_private, "Iterable")
            and hasattr(_private, "Mapping")
            and _private.__name__ == "collections.abc"
        ),
    }}
    for key in sorted(checks):
        print(f"CHECK|{{key}}|{{checks[key]}}")
"""


def _run_probe() -> tuple[list[tuple[str, str, str]], dict[str, str]]:
    proc = run_surface_test_process(
        [sys.executable, "-c", _PROBE],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=True,
    )
    rows: list[tuple[str, str, str]] = []
    checks: dict[str, str] = {}
    for line in proc.stdout.splitlines():
        prefix, *rest = line.split("|")
        if prefix == "ROW":
            rows.append((rest[0], rest[1], rest[2]))
        elif prefix == "CHECK":
            checks[rest[0]] = rest[1]
    return rows, checks


def test__collections_abc_public_surface_matches_expected_shape() -> None:
    rows, checks = _run_probe()
    names = [name for name, _, _ in rows]
    assert "molt_abc_bootstrap" not in names
    assert "molt_collections_abc_runtime_types" not in names
    assert "Iterable" in names
    assert "Mapping" in names
    assert "MutableSequence" in names
    assert checks == {"behavior": "True"}
