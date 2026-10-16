from __future__ import annotations

from pathlib import Path

import pytest

from tools import check_core_lane_lowering as gate


def _providers(root, names):
    paths = {}
    for name in names:
        path = root.joinpath(*name.split(".")) / "__init__.py"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("VALUE = 1\n", encoding="utf-8")
        paths[name] = path
    return paths


def test_core_manifest_excludes_newer_syntax_before_parsing(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    future = tmp_path / "future.py"
    future.write_text(
        "# MOLT_META: min_py=3.13\ntype Alias[T = int] = T\n", encoding="utf-8"
    )
    common = tmp_path / "common.py"
    common.write_text("import math\n", encoding="utf-8")
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(f"{future}\n{common}\n", encoding="utf-8")
    monkeypatch.setattr(gate, "sys", type("Host", (), {"version_info": (3, 12)}))
    original_facts = gate.stdlib_module_intrinsic_facts

    def facts(module_name, path, **kwargs):
        assert path != future, "excluded source reached the parser"
        return original_facts(module_name, path, **kwargs)

    monkeypatch.setattr(gate, "stdlib_module_intrinsic_facts", facts)
    assert gate._collect_seed_modules(manifest, _providers(tmp_path, {"math"})) == {
        "math"
    }
    output = capsys.readouterr().out
    assert f"[SKIP] {future} (min_py 3.13)" in output
    assert "admitted=1 excluded=1" in output


def test_core_manifest_keeps_applicable_syntax_errors_visible(tmp_path: Path) -> None:
    source = tmp_path / "broken.py"
    source.write_text("# MOLT_META: min_py=3.12\ndef broken(\n", encoding="utf-8")
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(f"{source}\n", encoding="utf-8")
    with pytest.raises(gate.UnresolvedStaticImportError) as error:
        gate._collect_seed_modules(manifest, {})
    assert isinstance(error.value.__cause__, SyntaxError)


@pytest.mark.parametrize(
    "source,known,expected",
    [
        ("from pkg import Value\n", {"pkg"}, {"pkg"}),
        ("from pkg import child\n", {"pkg", "pkg.child"}, {"pkg", "pkg.child"}),
        ("import project_helpers\nimport math\n", {"math"}, {"math"}),
        ("from project_helpers import *\nimport math\n", {"math"}, {"math"}),
        (
            "__import__('project_helpers', fromlist=('*',))\nimport math\n",
            {"math"},
            {"math"},
        ),
        (
            "import molt.stdlib.pkg.child\n",
            {"molt.stdlib", "pkg", "pkg.child"},
            {"molt.stdlib", "pkg", "pkg.child"},
        ),
        ("def later():\n    import math\n", {"math"}, {"math"}),
        ("__import__('math')\n", {"math"}, {"math"}),
    ],
)
def test_core_seed_projection_preserves_source_candidate_owners(
    tmp_path, source, known, expected
):
    path = tmp_path / "case.py"
    path.write_text(source, encoding="utf-8")
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(str(path) + "\n", encoding="utf-8")
    assert gate._collect_seed_modules(manifest, _providers(tmp_path, known)) == expected


@pytest.mark.parametrize(
    "source", ["import pkg.missing\n", "import molt.stdlib.pkg.missing\n"]
)
def test_core_inventory_preserves_named_absent_descendant_without_success_claim(
    tmp_path, source
):
    path = tmp_path / "case.py"
    path.write_text(source, encoding="utf-8")
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(str(path) + "\n", encoding="utf-8")
    assert gate._collect_seed_modules(manifest, _providers(tmp_path, {"pkg"})) == {
        "pkg",
        "pkg.missing",
    }


def test_core_dynamic_seed_is_explicit_custody_obligation_not_missing_stdlib(tmp_path):
    path = tmp_path / "case.py"
    path.write_text("__import__(project_selected_name)\n", encoding="utf-8")
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(str(path) + "\n", encoding="utf-8")
    with pytest.raises(
        gate.UnresolvedStaticImportError, match="requires runtime import custody"
    ):
        gate._collect_seed_modules(manifest, _providers(tmp_path, {"math"}))


def test_core_inventory_reports_candidate_status_and_exact_absence(
    tmp_path, monkeypatch, capsys
):
    path = tmp_path / "case.py"
    path.write_text(
        "try:\n    import pkg.missing\nexcept ImportError:\n    pass\n",
        encoding="utf-8",
    )
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(str(path) + "\n", encoding="utf-8")
    paths = _providers(tmp_path, {"pkg"})
    monkeypatch.setattr(
        gate.stdlib_audit,
        "stdlib_module_statuses",
        lambda: {"pkg": ("stub", paths["pkg"])},
    )
    monkeypatch.setattr(
        gate.sys, "argv", ["check_core_lane_lowering.py", "--manifest", str(manifest)]
    )
    assert gate.main() == 0
    output = capsys.readouterr().out
    assert "pkg: stub" in output
    assert "pkg.missing: absent" in output
    assert "runtime qualification is separate" in output


def test_core_inventory_refuses_opaque_custody_with_diagnostic(
    tmp_path, monkeypatch, capsys
):
    path = tmp_path / "case.py"
    path.write_text("__import__(project_selected_name)\n", encoding="utf-8")
    manifest = tmp_path / "manifest.txt"
    manifest.write_text(str(path) + "\n", encoding="utf-8")
    monkeypatch.setattr(gate.stdlib_audit, "stdlib_module_statuses", lambda: {})
    monkeypatch.setattr(
        gate.sys, "argv", ["check_core_lane_lowering.py", "--manifest", str(manifest)]
    )
    assert gate.main() == 1
    output = capsys.readouterr().out
    assert "requires runtime import custody" in output
    assert "inventory: ok" not in output
