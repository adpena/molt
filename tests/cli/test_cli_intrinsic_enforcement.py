from pathlib import Path

import molt.cli as cli


def _write_module(tmp_path: Path, name: str, source: str) -> Path:
    path = tmp_path / name
    path.write_text(source, encoding="utf-8")
    return path


def test_marker_literal_does_not_count_as_intrinsic_usage(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "marker_only.py",
        '_MOLT_INTRINSIC_MARKER = "molt_capabilities_has"\n',
    )
    assert cli._stdlib_module_intrinsic_status(module) == "python-only"


def test_require_intrinsic_call_is_intrinsic_backed(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "intrinsic_backed.py",
        (
            "from _intrinsics import require_intrinsic as _require_intrinsic\n"
            '_require_intrinsic("molt_capabilities_has", globals())\n'
        ),
    )
    assert cli._stdlib_module_intrinsic_status(module) == "intrinsic-backed"


def test_probe_only_module_status(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "probe_only.py",
        (
            "from _intrinsics import require_intrinsic as _require_intrinsic\n"
            '_require_intrinsic("molt_stdlib_probe", globals())\n'
        ),
    )
    assert cli._stdlib_module_intrinsic_status(module) == "probe-only"


def test_syntax_error_with_intrinsic_marker_is_python_only(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "invalid.py",
        (
            '_MOLT_INTRINSIC_MARKER = "molt_capabilities_has"\n'
            "def broken(:\n"
            "    return 1\n"
        ),
    )
    assert cli._stdlib_module_intrinsic_status(module) == "python-only"
