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
    RETIRED_FRONTEND_DIAGNOSTIC_CODES,
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
    assert row.get("ci_checkable", True)
    # CI gates every CI-checkable manifest generator through one runner command.
    proof_plan = tomllib.loads(
        (ROOT / "tools/proof_plan.toml").read_text(encoding="utf-8")
    )
    assert any(
        command["argv"][-2:] == ["tools/generators.py", "check"]
        for command in proof_plan["command"]
    )


def test_frontend_diagnostic_codes_are_unique_contiguous_and_complete() -> None:
    diagnostics = tuple(FrontendDiagnostic)
    active = [item.value for item in diagnostics]
    assert active == sorted(active)
    assert not set(active) & RETIRED_FRONTEND_DIAGNOSTIC_CODES
    assert sorted(set(active) | RETIRED_FRONTEND_DIAGNOSTIC_CODES) == [
        f"MOLT-FE{index:03d}"
        for index in range(1, len(active) + len(RETIRED_FRONTEND_DIAGNOSTIC_CODES) + 1)
    ]
    assert set(FRONTEND_DIAGNOSTIC_METADATA) == set(diagnostics)
    assert all(metadata.title for metadata in FRONTEND_DIAGNOSTIC_METADATA.values())


def test_a_retired_code_stays_reserved(tmp_path: Path) -> None:
    authority = tmp_path / "frontend_diagnostics.toml"
    active = (
        'schema_version = 1\n[[diagnostic]]\nname = "first"\ncode = "MOLT-FE001"\n'
        'title = "t"\ntier = "bridge"\nimpact = "high"\n'
    )
    authority.write_text(
        active + '[[retired]]\nname = "old"\ncode = "MOLT-FE001"\nreason = "r"\n',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="duplicate diagnostic code: MOLT-FE001"):
        GEN.load_authority(authority)
    authority.write_text(
        active + '[[retired]]\nname = "old"\ncode = "MOLT-FE003"\nreason = "r"\n',
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="missing MOLT-FE002"):
        GEN.load_authority(authority)


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
    node = ast.parse("value = getattr(obj, name)\n").body[0]
    reporter = CompatibilityReporter("error", "probe.py")
    rejection = FrontendRejection(
        FrontendDiagnostic.OPERAND_VALUE, "getattr expects object and name"
    )
    with pytest.raises(CompatibilityError) as raised:
        raise_compatibility_error(reporter, node, rejection)
    message = str(raised.value)
    assert "MOLT-FE003: operand or value cannot be lowered" in message
    assert "location: probe.py:1:0" in message


def test_real_frontend_rejection_is_deterministic() -> None:
    # CPython also rejects this at compile time.
    source = "nonlocal value\n"
    messages: list[str] = []
    for _ in range(2):
        with pytest.raises(CompatibilityError) as raised:
            SimpleTIRGenerator(source_path="deterministic.py").visit(ast.parse(source))
        messages.append(str(raised.value))
    assert messages[0] == messages[1]
    assert "MOLT-FE005" in messages[0]
    assert "feature: nonlocal declarations at module scope" in messages[0]
    assert "location: deterministic.py:1:0" in messages[0]


CALL_SHAPE_ERRORS = (
    "value = len()\n",
    "value = len([1], [2])\n",
    "value = isinstance(1)\n",
    "value = getattr(object())\n",
    "setattr(object(), 'a')\n",
    "value = ord()\n",
    "value = enumerate([], 0, 1)\n",
    "value = enumerate([], step=1)\n",
    "value = classmethod()\n",
    "value = slice()\n",
    "items = set()\nitems.add()\n",
    "items = [1]\nitems.pop(0, 1)\n",
    "items = [1]\nvalue = items.index()\n",
    "table = {}\nvalue = table.get()\n",
    "value = 'a'.lower(1)\n",
    "value = 'a'.strip(' ', ' ')\n",
    "value = 'a'.startswith()\n",
    "def gen():\n    yield 1\nvalue = gen().send()\n",
)


CLASS_CREATION_ERRORS = (
    "class Twice(int, int):\n    pass\n",
    "class Base:\n    pass\nAlias = Base\nclass Twice(Base, Alias):\n    pass\n",
)


@pytest.mark.parametrize("source", CLASS_CREATION_ERRORS)
def test_class_creation_errors_reach_the_runtime(source: str) -> None:
    # CPython raises TypeError when the class statement runs.
    with pytest.raises(TypeError, match="duplicate base class"):
        exec(compile(source, "probe.py", "exec"), {})
    SimpleTIRGenerator(source_path="probe.py").visit(ast.parse(source))


@pytest.mark.parametrize("source", CALL_SHAPE_ERRORS)
def test_call_shape_errors_reach_the_runtime_binder(source: str) -> None:
    # CPython compiles each call and raises TypeError only when it runs, so a
    # program can catch it. The frontend must lower it, not reject it.
    with pytest.raises(TypeError):
        exec(compile(source, "probe.py", "exec"), {})
    SimpleTIRGenerator(source_path="probe.py").visit(ast.parse(source))


def test_native_and_wasm_cli_share_the_frontend_diagnostic(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    import molt.cli as cli

    source = tmp_path / "unsupported_nonlocal.py"
    source.write_text("nonlocal value\n", encoding="utf-8")
    # The build's project is the working directory, which gets dist/; keep it
    # out of the checkout.
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("MOLT_COMPAT_WARNINGS", "0")
    monkeypatch.setenv("PYTHONHASHSEED", "0")
    # Cache activation has separate stderr diagnostics; this fixture stops at
    # the structured frontend error before backend compilation.
    monkeypatch.setenv("MOLT_USE_SCCACHE", "0")
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
    assert "MOLT-FE005" in errors[0][0]
    assert "feature: nonlocal declarations at module scope" in errors[0][0]
    assert f"location: {source}:1:0" in errors[0][0]
