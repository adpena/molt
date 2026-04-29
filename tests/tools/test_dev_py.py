from __future__ import annotations

import importlib.util
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
DEV_PY = REPO_ROOT / "tools" / "dev.py"


def _load_dev_py():
    spec = importlib.util.spec_from_file_location("molt_tools_dev_py", DEV_PY)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_dev_py_update_dispatches_to_cli(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[list[str], str | None, bool]] = []

    def fake_run_uv(args, python=None, env=None, tty=False):
        calls.append((list(args), python, tty))

    monkeypatch.setattr(module, "run_uv", fake_run_uv, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "update", "--check", "--all"],
        raising=True,
    )
    module.main()

    assert calls == [
        (
            ["python3", "-m", "molt.cli", "update", "--check", "--all"],
            module.TEST_PYTHONS[0],
            False,
        )
    ]


def test_dev_py_lint_uses_documented_stdlib_intrinsic_gates(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[list[str], str | None, bool]] = []

    def fake_run_uv(args, python=None, env=None, tty=False):
        calls.append((list(args), python, tty))

    monkeypatch.setattr(module, "run_uv", fake_run_uv, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "lint"],
        raising=True,
    )
    module.main()

    stdlib_calls = [
        args
        for args, _python, _tty in calls
        if args[:2] == ["python3", "tools/check_stdlib_intrinsics.py"]
    ]

    assert [
        "python3",
        "tools/check_stdlib_intrinsics.py",
        "--fallback-intrinsic-backed-only",
    ] in stdlib_calls
    assert [
        "python3",
        "tools/check_stdlib_intrinsics.py",
        "--critical-allowlist",
    ] in stdlib_calls


def test_dev_py_test_forwards_random_order_flags(monkeypatch) -> None:
    module = _load_dev_py()
    calls: list[tuple[list[str], str | None, bool]] = []

    def fake_run_uv(args, python=None, env=None, tty=False):
        calls.append((list(args), python, tty))

    monkeypatch.setattr(module, "run_uv", fake_run_uv, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        ["tools/dev.py", "test", "--random-order", "--random-seed", "17"],
        raising=True,
    )
    module.main()

    assert calls == [
        (
            [
                "python3",
                "tools/dev_test_runner.py",
                "--verified-subset",
                "--random-order",
                "--random-seed",
                "17",
            ],
            module.TEST_PYTHONS[0],
            False,
        ),
        (
            [
                "python3",
                "tools/dev_test_runner.py",
                "--random-order",
                "--random-seed",
                "17",
            ],
            module.TEST_PYTHONS[1],
            False,
        ),
        (
            [
                "python3",
                "tools/dev_test_runner.py",
                "--random-order",
                "--random-seed",
                "17",
            ],
            module.TEST_PYTHONS[2],
            False,
        ),
    ]
