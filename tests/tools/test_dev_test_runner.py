from __future__ import annotations

import importlib.util
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
DEV_TEST_RUNNER = REPO_ROOT / "tools" / "dev_test_runner.py"


def _load_dev_test_runner():
    spec = importlib.util.spec_from_file_location(
        "molt_tools_dev_test_runner", DEV_TEST_RUNNER
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_dev_test_runner_main_forwards_random_order_flags(monkeypatch) -> None:
    module = _load_dev_test_runner()
    calls: list[list[str]] = []

    def fake_run(cmd: list[str]) -> None:
        calls.append(list(cmd))

    monkeypatch.setattr(module, "_run", fake_run, raising=True)
    monkeypatch.setattr(
        module.sys,
        "argv",
        [
            "tools/dev_test_runner.py",
            "--verified-subset",
            "--random-order",
            "--random-seed",
            "23",
        ],
        raising=True,
    )

    module.main()

    assert calls == [
        [
            "pytest",
            "-q",
            "-p",
            "tools.pytest_random_order_plugin",
            "--molt-random-order",
            "--molt-random-seed",
            "23",
        ],
        ["python3", "tools/verified_subset.py", "run"],
    ]


def test_dev_test_runner_generates_seed_when_random_order_enabled(monkeypatch) -> None:
    module = _load_dev_test_runner()
    monkeypatch.setattr(module.secrets, "randbelow", lambda upper: 1234, raising=True)

    cmd, seed = module._build_pytest_command(random_order=True, random_seed=None)

    assert seed == "1234"
    assert cmd == [
        "pytest",
        "-q",
        "-p",
        "tools.pytest_random_order_plugin",
        "--molt-random-order",
        "--molt-random-seed",
        "1234",
    ]
