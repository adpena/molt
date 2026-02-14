from __future__ import annotations

import importlib.util
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
SCRIPT_PATH = REPO_ROOT / "tools" / "check_stdlib_intrinsics.py"


def _load_gate_module():
    spec = importlib.util.spec_from_file_location(
        "check_stdlib_intrinsics_gate", SCRIPT_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _seed_fallback_module(stdlib_root: Path) -> None:
    (stdlib_root / "fallback_mod.py").write_text(
        """
try:
    import alpha
except ImportError:
    import beta
""".strip()
        + "\n",
        encoding="utf-8",
    )


def _seed_bootstrap_strict_modules(
    stdlib_root: Path, *, partial_modules: tuple[str, ...] = ()
) -> None:
    stdlib_root.mkdir(parents=True, exist_ok=True)
    intrinsic_line = (
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_require_intrinsic("molt_capabilities_has", globals())\n'
    )
    partial_todo = (
        "# TODO(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, "
        "status:partial): test fixture partial marker.\n"
    )

    module_names = (
        "builtins",
        "sys",
        "types",
        "importlib",
        "importlib.machinery",
        "importlib.util",
    )
    package_roots = {name.split(".", 1)[0] for name in module_names if "." in name}

    def write_module(module_name: str) -> None:
        parts = module_name.split(".")
        if len(parts) == 1:
            if module_name in package_roots:
                package_dir = stdlib_root / module_name
                package_dir.mkdir(parents=True, exist_ok=True)
                path = package_dir / "__init__.py"
            else:
                path = stdlib_root / f"{parts[0]}.py"
        else:
            package_dir = stdlib_root / "/".join(parts[:-1])
            package_dir.mkdir(parents=True, exist_ok=True)
            init_path = package_dir / "__init__.py"
            if not init_path.exists():
                init_path.write_text(intrinsic_line, encoding="utf-8")
            path = package_dir / f"{parts[-1]}.py"
        text = intrinsic_line
        if module_name in partial_modules:
            text += partial_todo
        path.write_text(text, encoding="utf-8")

    for module_name in module_names:
        write_module(module_name)


def _seed_intrinsic_module(stdlib_root: Path, module_name: str, body: str) -> None:
    (stdlib_root / f"{module_name}.py").write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_MOLT_TEST_INTR = _require_intrinsic("molt_capabilities_has", globals())\n'
        + body
        + ("\n" if not body.endswith("\n") else ""),
        encoding="utf-8",
    )


def _seed_intrinsic_package(
    stdlib_root: Path, package_name: str, body: str = ""
) -> None:
    package_dir = stdlib_root / package_name
    package_dir.mkdir(parents=True, exist_ok=True)
    (package_dir / "__init__.py").write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_MOLT_TEST_INTR = _require_intrinsic("molt_capabilities_has", globals())\n'
        + body
        + ("\n" if body and not body.endswith("\n") else ""),
        encoding="utf-8",
    )


def _configure_required_top_level(module, monkeypatch, stdlib_root: Path) -> None:
    required_modules: set[str] = set()
    required_packages: set[str] = set()
    required_submodules: set[str] = set()
    required_subpackages: set[str] = set()
    present_modules: set[str] = set()
    for path in stdlib_root.rglob("*.py"):
        rel = path.relative_to(stdlib_root)
        if path.name == "__init__.py":
            if len(rel.parts) == 2:
                required_modules.add(rel.parts[0])
                required_packages.add(rel.parts[0])
            if len(rel.parts) > 1:
                present_modules.add(".".join(rel.parts[:-1]))
            if len(rel.parts) > 1:
                sub_name = ".".join(rel.parts[:-1])
                if "." in sub_name:
                    required_submodules.add(sub_name)
                    required_subpackages.add(sub_name)
            continue
        if len(rel.parts) == 1:
            required_modules.add(path.stem)
            present_modules.add(path.stem)
        else:
            sub_name = ".".join((*rel.parts[:-1], path.stem))
            present_modules.add(sub_name)
            if "." in sub_name:
                required_submodules.add(sub_name)
    monkeypatch.setattr(
        module,
        "_load_required_top_level_stdlib",
        lambda: (frozenset(required_modules), frozenset(required_packages)),
    )
    monkeypatch.setattr(
        module,
        "_load_required_stdlib_submodules",
        lambda: (frozenset(required_submodules), frozenset(required_subpackages)),
    )
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset(),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {},
    )


def test_critical_allowlist_includes_re() -> None:
    module = _load_gate_module()
    assert "re" in module.CRITICAL_STRICT_IMPORT_ROOTS


def test_all_stdlib_fallback_gate_is_default(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_fallback_module(stdlib_root)

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        ["check_stdlib_intrinsics.py", "--update-doc"],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "all-stdlib fallback gate violated" in out
    assert "fallback_mod" in out


def test_fallback_intrinsic_backed_only_opt_down_flag_is_accepted(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    (stdlib_root / "intrinsic_mod.py").write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_require_intrinsic("molt_capabilities_has", globals())\n',
        encoding="utf-8",
    )
    audit_doc = tmp_path / "audit.md"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", audit_doc)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
        ],
    )

    assert module.main() == 0
    assert audit_doc.exists()


def test_zero_non_intrinsic_gate_rejects_python_only_module(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    (stdlib_root / "plain_mod.py").write_text("VALUE = 1\n", encoding="utf-8")

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "zero non-intrinsic gate violated" in out
    assert "python-only modules" in out
    assert "plain_mod" in out


def test_zero_non_intrinsic_gate_rejects_probe_only_module(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    (stdlib_root / "probe_mod.py").write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_require_intrinsic("molt_stdlib_probe", globals())\n',
        encoding="utf-8",
    )

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "zero non-intrinsic gate violated" in out
    assert "probe-only modules" in out
    assert "probe_mod" in out


def test_bootstrap_strict_closure_allows_intrinsic_partial_root(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    _seed_bootstrap_strict_modules(stdlib_root, partial_modules=("builtins",))

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    assert module.main() == 0


def test_bootstrap_strict_closure_rejects_transitive_python_only_dependency(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    _seed_bootstrap_strict_modules(stdlib_root)
    (stdlib_root / "sys.py").write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_require_intrinsic("molt_capabilities_has", globals())\n'
        "import os\n",
        encoding="utf-8",
    )
    (stdlib_root / "os.py").write_text("VALUE = 1\n", encoding="utf-8")

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "bootstrap strict closure must be intrinsic-implemented" in out
    assert "os: python-only" in out


def test_intrinsic_runtime_fallback_gate_rejects_swallowed_errors(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "json",
        """
def parse_value(text: str):
    try:
        return _MOLT_TEST_INTR(text, "")
    except ValueError:
        pass
    return None
""".strip(),
    )

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "intrinsic runtime fallback gate violated" in out
    assert "json" in out


def test_intrinsic_runtime_fallback_gate_allows_raise_mapping(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "json",
        """
def parse_value(text: str):
    try:
        return _MOLT_TEST_INTR(text, "")
    except ValueError as exc:
        raise RuntimeError("intrinsic failed") from exc
""".strip(),
    )

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
        ],
    )

    assert module.main() == 0


def test_stdlib_parity_todo_is_classified_intrinsic_partial(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "parity_mod",
        "# TODO(stdlib-parity, owner:stdlib, milestone:SL2, priority:P1, "
        "status:planned): parity backlog.\n",
    )
    audit_doc = tmp_path / "audit.md"
    report = tmp_path / "report.json"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", audit_doc)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--json-out",
            str(report),
        ],
    )

    assert module.main() == 0
    import json

    payload = json.loads(report.read_text(encoding="utf-8"))
    module_status = {
        entry["module"]: entry["status"] for entry in payload.get("modules", [])
    }
    assert module_status.get("parity_mod") == "intrinsic-partial"


def test_stdlib_generic_todo_is_classified_intrinsic_partial(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "generic_mod",
        "# TODO(stdlib, owner:runtime, milestone:TL3, priority:P2, "
        "status:planned): runtime backlog.\n",
    )
    audit_doc = tmp_path / "audit.md"
    report = tmp_path / "report.json"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", audit_doc)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--json-out",
            str(report),
        ],
    )

    assert module.main() == 0
    import json

    payload = json.loads(report.read_text(encoding="utf-8"))
    module_status = {
        entry["module"]: entry["status"] for entry in payload.get("modules", [])
    }
    assert module_status.get("generic_mod") == "intrinsic-partial"


def test_top_level_union_gate_rejects_missing_entries(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1")

    monkeypatch.setattr(
        module,
        "_load_required_top_level_stdlib",
        lambda: (frozenset({"alpha", "beta"}), frozenset()),
    )
    monkeypatch.setattr(
        module,
        "_load_required_stdlib_submodules",
        lambda: (frozenset(), frozenset()),
    )
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "stdlib top-level coverage gate violated" in out
    assert "beta" in out


def test_top_level_union_gate_rejects_module_package_collision(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1")
    _seed_intrinsic_package(stdlib_root, "alpha")

    monkeypatch.setattr(
        module,
        "_load_required_top_level_stdlib",
        lambda: (frozenset({"alpha"}), frozenset({"alpha"})),
    )
    monkeypatch.setattr(
        module,
        "_load_required_stdlib_submodules",
        lambda: (frozenset(), frozenset()),
    )
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "top-level module/package duplicate mapping" in out
    assert "alpha" in out


def test_top_level_union_gate_rejects_package_kind_mismatch(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "xml", "VALUE = 1")

    monkeypatch.setattr(
        module,
        "_load_required_top_level_stdlib",
        lambda: (frozenset({"xml"}), frozenset({"xml"})),
    )
    monkeypatch.setattr(
        module,
        "_load_required_stdlib_submodules",
        lambda: (frozenset(), frozenset()),
    )
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "stdlib package kind gate violated" in out
    assert "xml" in out


def test_submodule_union_gate_rejects_missing_entries(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    _seed_intrinsic_package(stdlib_root, "alpha")
    _seed_intrinsic_module(stdlib_root, "alpha/beta", "VALUE = 1")

    monkeypatch.setattr(
        module,
        "_load_required_top_level_stdlib",
        lambda: (frozenset({"alpha"}), frozenset({"alpha"})),
    )
    monkeypatch.setattr(
        module,
        "_load_required_stdlib_submodules",
        lambda: (frozenset({"alpha.beta", "alpha.gamma"}), frozenset({"alpha"})),
    )
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "stdlib submodule coverage gate violated" in out
    assert "alpha.gamma" in out


def test_submodule_union_gate_rejects_subpackage_kind_mismatch(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    _seed_intrinsic_package(stdlib_root, "alpha")
    _seed_intrinsic_module(stdlib_root, "alpha/beta", "VALUE = 1")

    monkeypatch.setattr(
        module,
        "_load_required_top_level_stdlib",
        lambda: (frozenset({"alpha"}), frozenset({"alpha"})),
    )
    monkeypatch.setattr(
        module,
        "_load_required_stdlib_submodules",
        lambda: (frozenset({"alpha.beta"}), frozenset({"alpha.beta"})),
    )
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "stdlib subpackage kind gate violated" in out
    assert "alpha.beta" in out


def test_intrinsic_partial_ratchet_gate_rejects_regression(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "partial_mod",
        "# TODO(stdlib, owner:stdlib, milestone:SL2, priority:P1, "
        "status:partial): fixture partial marker.\n",
    )
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text('{"max_intrinsic_partial": 0}\n', encoding="utf-8")

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "intrinsic-partial ratchet gate violated" in out
    assert "budget: 0" in out


def test_intrinsic_partial_ratchet_gate_allows_within_budget(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "partial_mod",
        "# TODO(stdlib, owner:stdlib, milestone:SL2, priority:P1, "
        "status:partial): fixture partial marker.\n",
    )
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text('{"max_intrinsic_partial": 1}\n', encoding="utf-8")

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
        ],
    )

    assert module.main() == 0


def test_host_fallback_import_pattern_rejected(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "fallback_mod",
        "import _py_decimal\n",
    )
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text('{"max_intrinsic_partial": 0}\n', encoding="utf-8")

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "Host fallback imports (`_py_*`) are forbidden" in out


def test_full_coverage_attestation_defaults_to_intrinsic_partial(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1\n")
    manifest = tmp_path / "full_coverage.py"
    manifest.write_text("STDLIB_FULLY_COVERED_MODULES = ()\n", encoding="utf-8")
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text('{"max_intrinsic_partial": 1}\n', encoding="utf-8")
    report = tmp_path / "report.json"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset(),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
            "--full-coverage-manifest",
            str(manifest),
            "--json-out",
            str(report),
        ],
    )

    assert module.main() == 0

    import json

    payload = json.loads(report.read_text(encoding="utf-8"))
    module_status = {
        entry["module"]: entry["status"] for entry in payload.get("modules", [])
    }
    assert module_status.get("alpha") == "intrinsic-partial"


def test_full_coverage_attestation_marks_intrinsic_backed(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1\n")
    manifest = tmp_path / "full_coverage.py"
    manifest.write_text('STDLIB_FULLY_COVERED_MODULES = ("alpha",)\n', encoding="utf-8")
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text('{"max_intrinsic_partial": 0}\n', encoding="utf-8")
    report = tmp_path / "report.json"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset({"alpha"}),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {"alpha": tuple()},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
            "--full-coverage-manifest",
            str(manifest),
            "--json-out",
            str(report),
        ],
    )

    assert module.main() == 0

    import json

    payload = json.loads(report.read_text(encoding="utf-8"))
    module_status = {
        entry["module"]: entry["status"] for entry in payload.get("modules", [])
    }
    assert module_status.get("alpha") == "intrinsic-backed"


def test_full_coverage_intrinsic_contract_requires_module_entry(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1\n")
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text('{"max_intrinsic_partial": 0}\n', encoding="utf-8")

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset({"alpha"}),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "full-coverage intrinsic contract missing modules" in out
    assert "alpha" in out


def test_full_coverage_intrinsic_contract_requires_intrinsic_wiring(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1\n")
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text('{"max_intrinsic_partial": 0}\n', encoding="utf-8")

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset({"alpha"}),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {"alpha": ("molt_time_time",)},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "full-coverage intrinsic contract violated" in out
    assert "alpha" in out


def test_full_coverage_intrinsic_contract_accepts_required_intrinsics(
    tmp_path: Path, monkeypatch
) -> None:
    module = _load_gate_module()
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "alpha",
        '_MOLT_TIME = _require_intrinsic("molt_time_time", globals())\n',
    )
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text('{"max_intrinsic_partial": 0}\n', encoding="utf-8")

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset({"alpha"}),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {"alpha": ("molt_time_time",)},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--fallback-intrinsic-backed-only",
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
        ],
    )

    assert module.main() == 0
