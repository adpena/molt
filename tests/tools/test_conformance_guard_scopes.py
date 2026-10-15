from __future__ import annotations

from contextlib import contextmanager
import sys
from pathlib import Path

from tools import check_translation_validation, parity_gate


def _fake_scope(calls: list[dict[str, object]]):
    @contextmanager
    def fake_guarded_harness_scope(**kwargs):
        calls.append(kwargs)

        class Scope:
            limits = kwargs["limits"]
            memory_guard = {"enabled": True}

        yield Scope()

    return fake_guarded_harness_scope


def test_check_translation_validation_uses_conformance_guard_scope(
    monkeypatch, tmp_path: Path
) -> None:
    module = check_translation_validation
    src = tmp_path / "case.py"
    src.write_text("print('ok')\n", encoding="utf-8")
    calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_harness_scope",
        _fake_scope(calls),
    )
    monkeypatch.setattr(
        module,
        "validate_file",
        lambda source, profile, timeout, verbose=False: module.ValidationResult(
            source,
            module.ValidationResult.PASS,
            "",
            0.01,
        ),
    )
    monkeypatch.setattr(
        sys,
        "argv",
        ["tools/check_translation_validation.py", str(src)],
    )

    rc = module.main()

    assert rc == 0
    assert calls[0]["prefix"] == "MOLT_CONFORMANCE"
    assert calls[0]["repo_root"] == module.REPO_ROOT
    assert calls[0]["label"] == "check_translation_validation"


def test_parity_gate_uses_conformance_guard_scope(monkeypatch, tmp_path: Path) -> None:
    module = parity_gate
    test_dir = tmp_path / "diff"
    test_dir.mkdir()
    case = test_dir / "case.py"
    case.write_text("print('ok')\n", encoding="utf-8")
    calls: list[dict[str, object]] = []
    monkeypatch.setattr(
        module.harness_memory_guard,
        "guarded_harness_scope",
        _fake_scope(calls),
    )
    monkeypatch.setattr(module, "_resolve_molt_cmd", lambda: (["molt"], {}))
    monkeypatch.setattr(
        module,
        "run_one",
        lambda path, molt_cmd, molt_env=None, *, timeout: module.TestResult(
            file=path,
            tier=module.TIER_STRICT,
            status="pass",
            cpython_stdout="ok\n",
            cpython_stderr="",
            molt_stdout="ok\n",
            molt_stderr="",
        ),
    )
    monkeypatch.setattr(sys, "argv", ["tools/parity_gate.py", str(test_dir)])

    rc = module.main()

    assert rc == 0
    assert calls[0]["prefix"] == "MOLT_CONFORMANCE"
    assert calls[0]["repo_root"] == module.REPO_ROOT
    assert calls[0]["label"] == "parity_gate"
