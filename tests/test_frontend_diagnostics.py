"""Generated frontend diagnostic authority and consumer-structure proofs."""

from __future__ import annotations

import ast
import importlib.util
import json
from pathlib import Path
import sys
import tomllib

import pytest

from molt.compat import CompatibilityError, CompatibilityReporter
from molt.frontend import SimpleTIRGenerator
from molt.frontend.diagnostics import (
    FrontendDiagnostic,
    FrontendRejection,
    raise_compatibility_error,
)
from molt.frontend.frontend_diagnostics_generated import (
    FRONTEND_DIAGNOSTIC_METADATA,
)


ROOT = Path(__file__).resolve().parents[1]
GENERATOR = ROOT / "tools/gen_frontend_diagnostics.py"


def _load_generator():
    spec = importlib.util.spec_from_file_location(
        "molt_test_gen_frontend_diagnostics", GENERATOR
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


GEN = _load_generator()


def test_generated_frontend_diagnostics_are_in_sync() -> None:
    assert GEN.main(["--check"]) == 0


def test_frontend_diagnostic_generator_is_registered_and_ci_gated() -> None:
    manifest = tomllib.loads(
        (ROOT / "tools/generator_manifest.toml").read_text(encoding="utf-8")
    )
    rows = {row["tool"]: row for row in manifest["generator"] if "tool" in row}
    row = rows["tools/gen_frontend_diagnostics.py"]
    assert row["outputs"] == ["src/molt/frontend/frontend_diagnostics_generated.py"]
    assert row["check_command"] == "tools/gen_frontend_diagnostics.py --check"
    assert row["sync_test"] == "tests/test_frontend_diagnostics.py"
    proof_plan = tomllib.loads(
        (ROOT / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    command = next(
        command
        for command in proof_plan["command"]
        if command["id"] == "repository.frontend-diagnostics.generated"
    )
    assert command["argv"][-2:] == ["tools/gen_frontend_diagnostics.py", "--check"]


def test_frontend_diagnostic_codes_are_unique_contiguous_and_complete() -> None:
    diagnostics = tuple(FrontendDiagnostic)
    assert [item.value for item in diagnostics] == [
        f"MOLT-FE{index:03d}" for index in range(1, len(diagnostics) + 1)
    ]
    assert set(FRONTEND_DIAGNOSTIC_METADATA) == set(diagnostics)
    assert all(metadata.title for metadata in FRONTEND_DIAGNOSTIC_METADATA.values())


def test_every_frontend_rejection_uses_the_generated_authority() -> None:
    total, counts = GEN.validate_consumers(GEN.load_diagnostics())
    assert total == sum(counts.values())
    assert total > 0
    assert all(count > 0 for count in counts.values())


def test_consumer_gate_rejects_direct_notimplemented_lane(tmp_path: Path) -> None:
    frontend = tmp_path / "src/molt/frontend"
    (frontend / "lowering").mkdir(parents=True)
    (frontend / "lowering/emission_core.py").write_text(
        "try:\n    pass\nexcept FrontendRejection:\n    pass\n",
        encoding="utf-8",
    )
    (frontend / "visitor.py").write_text(
        "def lower():\n    raise NotImplementedError('stub')\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="direct NotImplementedError"):
        GEN.validate_consumers(GEN.load_diagnostics(), tmp_path)


def test_consumer_gate_rejects_stringly_rejection(tmp_path: Path) -> None:
    frontend = tmp_path / "src/molt/frontend"
    (frontend / "lowering").mkdir(parents=True)
    (frontend / "lowering/emission_core.py").write_text(
        "try:\n    pass\nexcept FrontendRejection:\n    pass\n",
        encoding="utf-8",
    )
    (frontend / "visitor.py").write_text(
        "def lower():\n    raise FrontendRejection('stringly', 'detail')\n",
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="generated FrontendDiagnostic member"):
        GEN.validate_consumers(GEN.load_diagnostics(), tmp_path)


def test_rejection_conversion_has_stable_code_and_location() -> None:
    node = ast.parse("value = len()\n").body[0]
    reporter = CompatibilityReporter("error", "probe.py")
    rejection = FrontendRejection(
        FrontendDiagnostic.CALL_SIGNATURE,
        "len() takes exactly one argument (0 given)",
    )
    with pytest.raises(CompatibilityError) as raised:
        raise_compatibility_error(reporter, node, rejection)
    message = str(raised.value)
    assert "MOLT-FE002: call signature is outside the lowered contract" in message
    assert "location: probe.py:1:0" in message


def test_real_call_dispatch_rejection_is_deterministic() -> None:
    source = "value = len()\n"
    messages: list[str] = []
    for _ in range(2):
        with pytest.raises(CompatibilityError) as raised:
            SimpleTIRGenerator(source_path="deterministic.py").visit(ast.parse(source))
        messages.append(str(raised.value))
    assert messages[0] == messages[1]
    assert "MOLT-FE002" in messages[0]
    assert "feature: len() takes exactly one argument (0 given)" in messages[0]


def test_native_and_wasm_cli_share_the_frontend_diagnostic(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    import molt.cli as cli

    source = tmp_path / "unsupported_call.py"
    source.write_text("value = len()\n", encoding="utf-8")
    monkeypatch.setenv("MOLT_COMPAT_WARNINGS", "0")
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    errors: list[list[str]] = []
    for target in ("native", "wasm"):
        monkeypatch.setattr(
            sys,
            "argv",
            ["molt", "build", str(source), "--target", target, "--json"],
        )
        assert cli.main() == 2
        captured = capsys.readouterr()
        assert captured.err == ""
        payload = json.loads(captured.out)
        assert payload["status"] == "error"
        errors.append(payload["errors"])
    assert errors[0] == errors[1]
    assert "MOLT-FE002" in errors[0][0]
