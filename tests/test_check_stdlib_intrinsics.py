from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest
from tests.process_guard_common import run_guarded_test_process
from tests.reconstructed_function_test_support import (
    ACTIVATION_TARGET_ROUTES,
    _activation_context_sources,
    _activation_publication_sources,
    activation_target_sources,
)

from tools import check_stdlib_intrinsics

REPO_ROOT = Path(__file__).resolve().parents[1]


@pytest.fixture(autouse=True)
def _private_build_state(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # The gate caches per-module facts under the build state; keep each test's
    # cache in its own directory instead of the checkout's target tree.
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "build-state"))


@pytest.mark.parametrize(
    "source",
    [
        "import molt",
        "import molt.net as networking",
        "import os, molt.intrinsics as loader",
        "import molt.stdlib.io, molt.net as networking",
        "from molt import intrinsics as loader",
        "from molt import (\n    net as networking,  # runtime edge\n    intrinsics,\n)",
        "from molt import stdlib",
        "from molt import *",
        "from molt.net import Stream",
        "from molt.stdlib_intrinsic_policy import intrinsic_names_from_source",
        "from molt.stdlib_extra import helper",
        "from typing import TYPE_CHECKING\n"
        "if TYPE_CHECKING:\n"
        "    from molt._intrinsics import molt_spawn",
        "def lazy():\n    from molt import net",
        "if False:\n    import molt.frontend",
    ],
)
def test_runtime_boundary_rejects_compiler_imports(tmp_path: Path, source: str) -> None:
    module = check_stdlib_intrinsics
    path = tmp_path / "runtime.py"
    path.write_text(source + "\n", encoding="utf-8")

    errors, _, _ = module._scan_file(path)

    assert len(errors) == 1
    assert "Importing the host molt compiler package" in errors[0]
    assert errors[0].startswith("Line ")


@pytest.mark.parametrize(
    "source",
    [
        "import moltlib.net, molten, molt_extra",
        "from moltlib import net as networking",
        "from _intrinsics import require_intrinsic as require",
        "import molt.stdlib as stdlib",
        "import os, molt.stdlib.io as io",
        "from molt.stdlib import io, os",
        "from molt.stdlib._intrinsics import require_intrinsic as require",
        "from . import molt",
        "from .molt import helper",
        "# from molt import intrinsics\ntext = 'import molt.net'",
        '"""\nimport molt\nfrom molt import intrinsics\n"""',
    ],
)
def test_runtime_boundary_allows_runtime_and_normalized_imports(
    tmp_path: Path, source: str
) -> None:
    module = check_stdlib_intrinsics
    path = tmp_path / "runtime.py"
    path.write_text(source + "\n", encoding="utf-8")

    errors, _, _ = module._scan_file(path)

    assert errors == []


@pytest.mark.parametrize("owner", ["stdlib", "moltlib"])
def test_runtime_boundary_is_enforced_for_both_source_roots(
    tmp_path: Path, monkeypatch, capsys, owner: str
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    moltlib_root = tmp_path / "moltlib"
    moltlib_root.mkdir()
    _seed_bootstrap_strict_modules(stdlib_root)
    bad_path = tmp_path / owner / "bad.py"
    bad_path.write_text(
        "from typing import TYPE_CHECKING\n"
        "if TYPE_CHECKING:\n"
        "    from molt._intrinsics import molt_spawn\n",
        encoding="utf-8",
    )
    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "MOLTLIB_ROOT", moltlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    assert module.main() == 1
    output = capsys.readouterr().out
    assert module._display_path(bad_path) in output
    assert "Line 3: Importing the host molt compiler package" in output


def test_moltlib_boundary_scan_does_not_change_stdlib_coverage(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    moltlib_root = tmp_path / "moltlib"
    nested = moltlib_root / "nested"
    nested.mkdir(parents=True)
    _seed_bootstrap_strict_modules(stdlib_root)
    # Pure runtime support is not subject to the CPython intrinsic-coverage gate.
    (nested / "runtime_helper.py").write_text("value = 1\n", encoding="utf-8")
    # Generated typing declarations are not compiled runtime sources.
    (nested / "runtime_helper.pyi").write_text("import molt\n", encoding="utf-8")
    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "MOLTLIB_ROOT", moltlib_root)
    audit_path = tmp_path / "audit.md"
    report_path = tmp_path / "report.json"
    monkeypatch.setattr(module, "AUDIT_DOC", audit_path)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--json-out",
            str(report_path),
        ],
    )

    assert module.main() == 0
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert {entry["module"] for entry in report["modules"]} == {
        "_intrinsics",
        "builtins",
        "sys",
        "types",
        "importlib",
        "importlib.machinery",
        "importlib.util",
    }
    assert sum(report["status_counts"].values()) == 7
    assert "runtime_helper" not in audit_path.read_text(encoding="utf-8")


def test_runtime_seeded_builtins_facade_uses_auditable_canonical_resolver() -> None:
    from molt.stdlib_intrinsic_policy import module_required_intrinsic_names

    module = check_stdlib_intrinsics
    path = REPO_ROOT / "src" / "molt" / "stdlib" / "builtins.py"
    errors, intrinsic_names, _ = module._scan_file(path)
    assert errors == []
    assert frozenset(intrinsic_names) == module_required_intrinsic_names(path)
    classification = module.classify_stdlib_module_statuses(
        {"builtins": path}, target_python=module._DEFAULT_TARGET_PYTHON_VERSION
    )
    assert classification.statuses["builtins"] == "intrinsic-backed"
    assert "molt_compile_builtin" in classification.used_intrinsics["builtins"]


@pytest.mark.parametrize(
    "names, expected",
    [
        (("molt_a", "molt_b", "molt_a"), ("molt_a", "molt_b")),
        (("molt_a", 1), None),
        (("invalid",), None),
        (["molt_a"], None),
    ],
)
def test_required_intrinsic_boundary_validates_and_deduplicates(
    tmp_path, names, expected
):
    module = check_stdlib_intrinsics
    manifest = tmp_path / "required.py"
    manifest.write_text(
        f"STDLIB_REQUIRED_INTRINSICS_BY_MODULE = {{'pkg': {names!r}}}\n",
        encoding="utf-8",
    )
    if expected is None:
        with pytest.raises(
            RuntimeError, match="stdlib full-coverage manifest is invalid"
        ):
            module._load_full_coverage_required_intrinsics(manifest)
    else:
        assert module._load_full_coverage_required_intrinsics(manifest) == {
            "pkg": expected
        }


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
    _seed_intrinsic_loader(stdlib_root)
    intrinsic_line = _INTRINSIC_READ
    partial_gap_marker = "# STDLIB_GAP(stdlib-compat, owner:stdlib, milestone:SL1, priority:P0, status:partial): test fixture partial marker.\n"

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
            text += partial_gap_marker
        path.write_text(text, encoding="utf-8")

    for module_name in module_names:
        write_module(module_name)


# A module that requires one intrinsic and reads it.
_INTRINSIC_READ = (
    "from _intrinsics import require_intrinsic as _require_intrinsic\n"
    '_MOLT_TEST_INTR = _require_intrinsic("molt_capabilities_has")\n'
    "def _has_capability(name):\n"
    "    return _MOLT_TEST_INTR(name)\n"
)


def _stub_source(name: str) -> str:
    from molt.stdlib_intrinsic_policy import STDLIB_STUB_MARKER

    return (
        "def __getattr__(attr):\n"
        f"    raise RuntimeError('stdlib module \"{name}\" {STDLIB_STUB_MARKER}')\n"
    )


def _seed_intrinsic_loader(stdlib_root: Path) -> None:
    # Model the real required-loader boundary: only the injected native lookup
    # can supply a callable; absent or invalid providers fail closed. These are
    # source-policy fixtures, not evidence of native provider execution.
    stdlib_root.mkdir(parents=True, exist_ok=True)
    (stdlib_root / "_intrinsics.py").write_text(
        "def require_intrinsic(name, namespace=None):\n"
        "    lookup = globals().get('_molt_intrinsic_lookup')\n"
        "    value = lookup(name) if callable(lookup) else None\n"
        "    if not callable(value):\n"
        "        raise RuntimeError('missing required intrinsic: ' + name)\n"
        "    return value\n",
        encoding="utf-8",
    )


def _seed_intrinsic_module(stdlib_root: Path, module_name: str, body: str) -> None:
    _seed_intrinsic_loader(stdlib_root)
    (stdlib_root / f"{module_name}.py").write_text(
        _INTRINSIC_READ + body + ("\n" if not body.endswith("\n") else ""),
        encoding="utf-8",
    )


def _seed_intrinsic_package(
    stdlib_root: Path, package_name: str, body: str = ""
) -> None:
    _seed_intrinsic_loader(stdlib_root)
    package_dir = stdlib_root / package_name
    package_dir.mkdir(parents=True, exist_ok=True)
    (package_dir / "__init__.py").write_text(
        _INTRINSIC_READ + body + ("\n" if body and not body.endswith("\n") else ""),
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
        lambda _path: (
            frozenset({"_intrinsics"})
            if (stdlib_root / "_intrinsics.py").is_file()
            else frozenset()
        ),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: (
            {"_intrinsics": ()} if (stdlib_root / "_intrinsics.py").is_file() else {}
        ),
    )


def _disable_full_coverage_contract(module, monkeypatch) -> None:
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


def _configure_report_test(tmp_path: Path, monkeypatch, *options: str):
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    _seed_bootstrap_strict_modules(stdlib_root)
    _configure_required_top_level(module, monkeypatch, stdlib_root)
    audit_path = tmp_path / "audit.md"
    report_path = tmp_path / "audit.json"
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "MOLTLIB_ROOT", tmp_path / "moltlib")
    monkeypatch.setattr(module, "AUDIT_DOC", audit_path)
    monkeypatch.setattr(
        sys,
        "argv",
        ["check_stdlib_intrinsics.py", "--json-out", str(report_path), *options],
    )
    return module, stdlib_root, audit_path, report_path


@pytest.mark.parametrize("existing_doc", [False, True])
def test_failed_audit_reports_every_gate_once_without_publishing_docs(
    tmp_path: Path, monkeypatch, capsys, existing_doc: bool
) -> None:
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text(
        '{"max_intrinsic_partial": 0, "max_stub": 0}\n', encoding="utf-8"
    )
    module, root, doc, report = _configure_report_test(
        tmp_path,
        monkeypatch,
        "--update-doc",
        "--intrinsic-partial-ratchet-file",
        str(ratchet),
    )
    _seed_intrinsic_module(root, "alpha", "import molt.net\nimport orphan\n")
    (root / "orphan.py").write_text(_stub_source("orphan"), encoding="utf-8")
    _configure_required_top_level(module, monkeypatch, root)
    monkeypatch.setattr(
        module, "_load_fully_covered_stdlib_modules", lambda _path: frozenset({"alpha"})
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {"alpha": ("molt_time_time",)},
    )
    if existing_doc:
        doc.write_bytes(b"previous green document\n")
    scans: list[Path] = []
    classifications = 0
    scan_file = module._scan_file
    classify = module.classify_stdlib_module_statuses

    def scan_once(path):
        scans.append(path)
        return scan_file(path)

    def classify_once(*args, **kwargs):
        nonlocal classifications
        classifications += 1
        return classify(*args, **kwargs)

    monkeypatch.setattr(module, "_scan_file", scan_once)
    monkeypatch.setattr(module, "classify_stdlib_module_statuses", classify_once)
    assert module.main() == 1
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert payload["analysis_complete"] is True
    assert payload["ok"] is False
    assert payload["schema"] == "molt.stdlib-intrinsics-audit.v4"
    assert {
        "failures",
        "full-coverage-missing-intrinsic-wiring",
        "stub-budget",
    } <= {item["code"] for item in payload["diagnostics"]}
    output = capsys.readouterr().out
    assert all(
        message in output
        for item in payload["diagnostics"]
        for message in item["messages"]
    )
    assert "stdlib intrinsics lint: ok" not in output
    assert classifications == 1
    assert len(scans) == len(set(scans)) == len(list(root.rglob("*.py")))
    if existing_doc:
        assert doc.read_bytes() == b"previous green document\n"
    else:
        assert not doc.exists()


def test_unknown_and_non_intrinsic_strict_roots_preserve_remaining_diagnostics(
    tmp_path: Path, monkeypatch
) -> None:
    module, root, doc, report = _configure_report_test(
        tmp_path, monkeypatch, "--update-doc", "--allowlist-modules", "absent,orphan"
    )
    (root / "orphan.py").write_text(_stub_source("orphan"), encoding="utf-8")
    _configure_required_top_level(module, monkeypatch, root)
    assert module.main() == 1
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert payload["analysis_complete"] is True
    assert payload["unknown_strict_roots"] == ["absent"]
    assert payload["strict_root_status_violations"] == [
        {"module": "orphan", "status": "stub"}
    ]
    assert {
        "unknown-strict-roots",
        "strict-root-status-violations",
    } <= {item["code"] for item in payload["diagnostics"]}
    assert not doc.exists()


def test_source_audit_admits_python_import_error_selection(tmp_path, monkeypatch):
    module, root, _, report = _configure_report_test(
        tmp_path, monkeypatch, "--update-doc"
    )
    _seed_fallback_module(root)
    _configure_required_top_level(module, monkeypatch, root)
    assert module.main() == 0
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert payload["ok"] is True
    assert not {
        "strict_fallback_violations",
        "intrinsic_backed_fallback_violations",
        "all_fallback_violations",
        "intrinsic_runtime_fallback_violations",
        "intrinsic_pass_fallback_modules",
        "strict_import_violations",
    }.intersection(payload)


@pytest.mark.parametrize("existing_doc", [False, True])
def test_document_gate_failure_still_emits_complete_json(
    tmp_path: Path, monkeypatch, existing_doc: bool
) -> None:
    module, _, doc, report = _configure_report_test(tmp_path, monkeypatch)
    if existing_doc:
        doc.write_bytes(b"stale audit\n")
    assert module.main() == 1
    payload = json.loads(report.read_text(encoding="utf-8"))
    code = "audit-document-stale" if existing_doc else "audit-document-missing"
    assert [item["code"] for item in payload["diagnostics"]] == [code]
    assert payload["analysis_complete"] is True
    assert payload["ok"] is False


def test_green_audit_publishes_document_and_matching_report(
    tmp_path: Path, monkeypatch
) -> None:
    module, _, doc, report = _configure_report_test(
        tmp_path, monkeypatch, "--update-doc"
    )
    assert module.main() == 0
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert payload["analysis_complete"] is True
    assert payload["ok"] is True
    assert payload["diagnostics"] == []
    assert doc.read_text(encoding="utf-8").startswith("# ")


@pytest.mark.parametrize("failure", ["missing-root", "syntax", "manifest"])
def test_incomplete_audit_emits_failure_without_fabricated_empty_coverage(
    tmp_path: Path, monkeypatch, failure: str
) -> None:
    module, root, doc, report = _configure_report_test(
        tmp_path, monkeypatch, "--update-doc"
    )
    if failure == "missing-root":
        monkeypatch.setattr(module, "STDLIB_ROOT", tmp_path / "absent")
    elif failure == "syntax":
        (root / "bad.py").write_text("def broken(\n", encoding="utf-8")
    else:

        def invalid_manifest(_path):
            raise RuntimeError("invalid full-coverage manifest")

        monkeypatch.setattr(
            module, "_load_fully_covered_stdlib_modules", invalid_manifest
        )
    assert module.main() == 1
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert payload["analysis_complete"] is False
    assert payload["ok"] is False
    assert [item["code"] for item in payload["diagnostics"]] == ["incomplete-analysis"]
    assert "status_counts" not in payload
    assert "modules" not in payload
    assert not doc.exists()


@pytest.mark.parametrize(
    "source",
    [
        "STDLIB_FULLY_COVERED_MODULES = (UNDEFINED,)",
        "raise ImportError('manifest dependency missing')",
        "raise TypeError('invalid manifest input')",
        "raise ValueError('invalid manifest value')",
        "raise AttributeError('manifest attribute missing')",
        "raise SystemExit(0)",
    ],
)
def test_manifest_evaluation_failure_replaces_stale_report(
    tmp_path: Path, monkeypatch, source: str
) -> None:
    module = check_stdlib_intrinsics
    root = tmp_path / "stdlib"
    root.mkdir()
    manifest = tmp_path / "broken_manifest.py"
    manifest.write_text(source + "\n", encoding="utf-8")
    report = tmp_path / "audit.json"
    report.write_text('{"ok": true, "modules": ["stale"]}', encoding="utf-8")
    doc = tmp_path / "audit.md"
    doc.write_text("previous green document\n", encoding="utf-8")
    monkeypatch.setattr(module, "STDLIB_ROOT", root)
    monkeypatch.setattr(module, "AUDIT_DOC", doc)
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--full-coverage-manifest",
            str(manifest),
            "--json-out",
            str(report),
            "--update-doc",
        ],
    )
    assert module.main() == 1
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert payload["analysis_complete"] is False
    assert payload["ok"] is False
    assert "modules" not in payload
    assert "status_counts" not in payload
    assert [item["code"] for item in payload["diagnostics"]] == ["incomplete-analysis"]
    assert "failed to evaluate" in payload["diagnostics"][0]["messages"][0]
    assert doc.read_text(encoding="utf-8") == "previous green document\n"


@pytest.mark.parametrize(
    "loader",
    [
        "_load_required_top_level_stdlib",
        "_load_required_stdlib_submodules",
        "_load_fully_covered_stdlib_modules",
        "_load_full_coverage_required_intrinsics",
    ],
)
def test_all_manifest_loaders_normalize_evaluation_errors(
    tmp_path: Path, monkeypatch, loader: str
) -> None:
    module = check_stdlib_intrinsics
    manifest = tmp_path / "broken_manifest.py"
    manifest.write_text("BROKEN = UNDEFINED\n", encoding="utf-8")
    monkeypatch.setattr(module, "STDLIB_UNION_BASELINE", manifest)
    args = () if loader.startswith("_load_required_") else (manifest,)
    with pytest.raises(RuntimeError, match="failed to evaluate.*NameError"):
        getattr(module, loader)(*args)


def test_report_publication_failure_is_not_a_success(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module, _, _, report = _configure_report_test(tmp_path, monkeypatch, "--update-doc")
    report.mkdir()
    assert module.main() == 1
    output = capsys.readouterr().out
    assert "audit report publication failed:" in output
    assert "stdlib intrinsics lint: ok" not in output


def test_critical_allowlist_includes_re() -> None:
    module = check_stdlib_intrinsics
    assert "re" in module.CRITICAL_STRICT_IMPORT_ROOTS


def test_selected_source_roots_do_not_impose_candidate_import_success(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "critical_root", "")
    _seed_intrinsic_module(
        stdlib_root,
        "noncritical_with_fallback",
        """
try:
    import alpha
except ImportError:
    import beta
""".strip(),
    )
    audit_doc = tmp_path / "audit.md"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", audit_doc)
    monkeypatch.setattr(module, "CRITICAL_STRICT_IMPORT_ROOTS", ("critical_root",))
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--critical-allowlist",
        ],
    )

    assert module.main() == 0
    assert audit_doc.exists()


def test_compiled_python_module_is_an_admitted_implementation(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    (stdlib_root / "plain_mod.py").write_text("VALUE = 1\n", encoding="utf-8")
    audit_doc = tmp_path / "audit.md"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", audit_doc)
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    assert module.main() == 0
    text = audit_doc.read_text(encoding="utf-8")
    assert "- `python-compiled`: `1`" in text
    assert "### Compiled pure-Python modules\n- `plain_mod`" in text


@pytest.mark.parametrize(
    "source",
    [
        # A discarded requirement: the anchor shape a probe or stub used.
        '_require_intrinsic("molt_capabilities_has")\n',
        # A private binding nothing loads.
        '_MOLT_READY = _require_intrinsic("molt_capabilities_has")\n',
    ],
)
def test_unread_intrinsic_requirement_is_not_backing(
    tmp_path: Path, monkeypatch, source: str
) -> None:
    module, root, _, report = _configure_report_test(
        tmp_path, monkeypatch, "--update-doc"
    )
    (root / "anchored_mod.py").write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n" + source,
        encoding="utf-8",
    )
    _configure_required_top_level(module, monkeypatch, root)
    assert module.main() == 1
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert [item["code"] for item in payload["diagnostics"]] == ["unread-intrinsics"]
    statuses = {entry["module"]: entry for entry in payload["modules"]}
    assert statuses["anchored_mod"]["status"] == "python-compiled"
    assert statuses["anchored_mod"]["intrinsics"] == []
    unread = [
        (entry["module"], entry["intrinsic"])
        for entry in (
            payload["unread_intrinsic_bindings"]
            + payload["discarded_intrinsic_requirements"]
        )
    ]
    assert unread == [("anchored_mod", "molt_capabilities_has")]


def test_same_package_import_preserves_python_implementation_in_audit(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    package = stdlib_root / "pkg"
    package.mkdir(parents=True)
    (package / "__init__.py").write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_WIDGET_BIND = _require_intrinsic("molt_tk_widget_bind_callback_register")\n',
        encoding="utf-8",
    )
    (package / "widgets.py").write_text(
        "from . import _WIDGET_BIND\nclass Widget:\n    pass\n",
        encoding="utf-8",
    )
    audit_doc = tmp_path / "audit.md"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", audit_doc)
    classify = module.classify_stdlib_module_statuses
    target_versions: list[str] = []

    def classify_for_target(paths, *, target_python, **options):
        target_versions.append(target_python.short)
        return classify(paths, target_python=target_python, **options)

    monkeypatch.setattr(module, "classify_stdlib_module_statuses", classify_for_target)
    selected_target = f"3.{min(sys.version_info.minor, 14)}"
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--target-python",
            selected_target,
        ],
    )

    assert module.main() == 0
    assert target_versions == [selected_target]
    audit_text = audit_doc.read_text(encoding="utf-8")
    assert (
        "### Intrinsic-backed modules (partial lowering pending)\n- `pkg`" in audit_text
    )
    assert "- `pkg.widgets`" in audit_text
    # The package owns the requirement; importing it does not replace the
    # sibling's own compiled Python implementation.
    assert "### Compiled pure-Python modules\n- `pkg.widgets`" in audit_text


def test_private_support_fragment_loaded_by_intrinsic_owner_is_not_python_only_in_audit(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    (stdlib_root / "_pyio.py").write_text(
        _INTRINSIC_READ + "def _load_text_io_classes():\n"
        "    import _pyio_text as text_module\n"
        "    return text_module\n",
        encoding="utf-8",
    )
    (stdlib_root / "_pyio_text.py").write_text(
        "class TextIOBase:\n    pass\n",
        encoding="utf-8",
    )
    audit_doc = tmp_path / "audit.md"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", audit_doc)
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    assert module.main() == 0
    audit_text = audit_doc.read_text(encoding="utf-8")
    assert (
        "### Intrinsic-owned support fragments and forwarding facades\n- `_pyio_text`"
        in audit_text
    )
    assert "- `_pyio_text`" in audit_text
    assert "### Compiled pure-Python modules\n- `_pyio_text`" not in audit_text


def test_fail_closed_import_policy_gate_is_allowed(tmp_path: Path, monkeypatch) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    (stdlib_root / "reserved_mod.py").write_text(
        '"""reserved namespace"""\n'
        "raise ImportError('reserved; use the explicit adapter')\n",
        encoding="utf-8",
    )
    audit_doc = tmp_path / "audit.md"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", audit_doc)
    monkeypatch.setattr(
        module, "ALLOWED_POLICY_GATE_MODULES", frozenset({"reserved_mod"})
    )
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    assert module.main() == 0
    text = audit_doc.read_text(encoding="utf-8")
    assert "- `policy-gate`: `1`" in text
    assert "- `reserved_mod`" in text


def test_fail_closed_import_policy_gate_requires_explicit_allowlist(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    (stdlib_root / "reserved_mod.py").write_text(
        '"""reserved namespace"""\n'
        "raise ImportError('reserved; use the explicit adapter')\n",
        encoding="utf-8",
    )

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 1
    assert "policy-gate modules require explicit allowlist" in out
    assert "reserved_mod" in out


def test_policy_gate_rejects_executable_python_body(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    (stdlib_root / "not_reserved_mod.py").write_text(
        '"""not a pure gate"""\nVALUE = 1\nraise ImportError(\'reserved\')\n',
        encoding="utf-8",
    )
    audit_doc = tmp_path / "audit.md"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", audit_doc)
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    # Executable Python makes it a compiled module, not an allowlisted gate.
    assert module.main() == 0
    text = audit_doc.read_text(encoding="utf-8")
    assert "- `policy-gate`: `0`" in text
    assert "### Compiled pure-Python modules\n- `not_reserved_mod`" in text


def test_real_stdlib_source_inventory_keeps_named_customization_queries():
    module = check_stdlib_intrinsics
    classification, audits = module._stdlib_module_audits(
        module._DEFAULT_TARGET_PYTHON_VERSION
    )
    assert {row.module for row in audits} >= {"_pyio", "_pylong", "getopt", "site"}
    names = {"sitecustomize", "usercustomize"}
    sites = [
        site
        for site in classification.import_evidence["site"].sites
        if site.request is not None and site.request.name in names
    ]
    assert {site.request.name for site in sites} == names
    assert all(
        site.owner_plan.modules == (site.request.name,) and not site.unresolved
        for site in sites
    )
    assert not names.intersection(classification.statuses)


def test_bootstrap_strict_closure_allows_intrinsic_partial_root(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    _seed_bootstrap_strict_modules(stdlib_root, partial_modules=("builtins",))

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    assert module.main() == 0


def test_bootstrap_source_inventory_preserves_candidate_stub_dependency(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    _seed_bootstrap_strict_modules(stdlib_root)
    (stdlib_root / "sys.py").write_text(
        _INTRINSIC_READ + "import os\n", encoding="utf-8"
    )
    (stdlib_root / "os.py").write_text(_stub_source("os"), encoding="utf-8")

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 0
    assert "stdlib intrinsics lint: ok" in out


def test_bootstrap_source_inventory_preserves_candidate_policy_gate_dependency(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    _seed_bootstrap_strict_modules(stdlib_root)
    (stdlib_root / "sys.py").write_text(
        _INTRINSIC_READ + "import reserved_mod\n", encoding="utf-8"
    )
    (stdlib_root / "reserved_mod.py").write_text(
        '"""reserved namespace"""\n'
        "raise ImportError('reserved; use the explicit adapter')\n",
        encoding="utf-8",
    )

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module, "ALLOWED_POLICY_GATE_MODULES", frozenset({"reserved_mod"})
    )
    monkeypatch.setattr(sys, "argv", ["check_stdlib_intrinsics.py", "--update-doc"])

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 0
    assert "stdlib intrinsics lint: ok" in out


def test_source_audit_does_not_infer_json_error_contract_from_catch_shape(
    tmp_path: Path, monkeypatch, capsys
) -> None:
    module = check_stdlib_intrinsics
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
        ],
    )

    exit_code = module.main()
    out = capsys.readouterr().out

    assert exit_code == 0
    assert "stdlib intrinsics lint: ok" in out


def test_source_audit_allows_python_exception_mapping(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
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
        ],
    )

    assert module.main() == 0


def test_stdlib_parity_gap_marker_is_classified_intrinsic_partial(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "parity_mod",
        "# STDLIB_GAP(stdlib-parity, owner:stdlib, milestone:SL2, priority:P1, status:planned): parity backlog.\n",
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


def test_stdlib_generic_gap_marker_is_classified_intrinsic_partial(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "generic_mod",
        "# STDLIB_GAP(stdlib-compat, owner:runtime, milestone:TL3, priority:P2, status:planned): runtime backlog.\n",
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
    module = check_stdlib_intrinsics
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
    _disable_full_coverage_contract(module, monkeypatch)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
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
    module = check_stdlib_intrinsics
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
    _disable_full_coverage_contract(module, monkeypatch)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
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
    module = check_stdlib_intrinsics
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
    _disable_full_coverage_contract(module, monkeypatch)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
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
    module = check_stdlib_intrinsics
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
    _disable_full_coverage_contract(module, monkeypatch)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
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
    module = check_stdlib_intrinsics
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
    _disable_full_coverage_contract(module, monkeypatch)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
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
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "partial_mod",
        "# STDLIB_GAP(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): fixture partial marker.\n",
    )
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text(
        '{"max_intrinsic_partial": 0, "max_stub": 0}\n', encoding="utf-8"
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
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "partial_mod",
        "# STDLIB_GAP(stdlib-compat, owner:stdlib, milestone:SL2, priority:P1, status:partial): fixture partial marker.\n",
    )
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text(
        '{"max_intrinsic_partial": 1, "max_stub": 0}\n', encoding="utf-8"
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
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
        ],
    )

    assert module.main() == 0


def test_full_coverage_attestation_defaults_to_intrinsic_partial(
    tmp_path: Path, monkeypatch
) -> None:
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1\n")
    manifest = tmp_path / "full_coverage.py"
    manifest.write_text(
        'STDLIB_FULLY_COVERED_MODULES = ("_intrinsics",)\n', encoding="utf-8"
    )
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text(
        '{"max_intrinsic_partial": 1, "max_stub": 0}\n', encoding="utf-8"
    )
    report = tmp_path / "report.json"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset({"_intrinsics"}),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {"_intrinsics": ()},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
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
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1\n")
    manifest = tmp_path / "full_coverage.py"
    manifest.write_text(
        'STDLIB_FULLY_COVERED_MODULES = ("alpha", "_intrinsics")\n', encoding="utf-8"
    )
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text(
        '{"max_intrinsic_partial": 0, "max_stub": 0}\n', encoding="utf-8"
    )
    report = tmp_path / "report.json"

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset({"alpha", "_intrinsics"}),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {"alpha": tuple(), "_intrinsics": ()},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
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
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1\n")
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text(
        '{"max_intrinsic_partial": 0, "max_stub": 0}\n', encoding="utf-8"
    )

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset({"alpha", "_intrinsics"}),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {"_intrinsics": ()},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
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
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(stdlib_root, "alpha", "VALUE = 1\n")
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text(
        '{"max_intrinsic_partial": 0, "max_stub": 0}\n', encoding="utf-8"
    )

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset({"alpha", "_intrinsics"}),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {"alpha": ("molt_time_time",), "_intrinsics": ()},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
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
    module = check_stdlib_intrinsics
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    _seed_intrinsic_module(
        stdlib_root,
        "alpha",
        '_MOLT_TIME = _require_intrinsic("molt_time_time")\n'
        "def now():\n"
        "    return _MOLT_TIME()\n",
    )
    ratchet = tmp_path / "ratchet.json"
    ratchet.write_text(
        '{"max_intrinsic_partial": 0, "max_stub": 0}\n', encoding="utf-8"
    )

    _configure_required_top_level(module, monkeypatch, stdlib_root)
    monkeypatch.setattr(module, "STDLIB_ROOT", stdlib_root)
    monkeypatch.setattr(module, "AUDIT_DOC", tmp_path / "audit.md")
    monkeypatch.setattr(
        module,
        "_load_fully_covered_stdlib_modules",
        lambda _path: frozenset({"alpha", "_intrinsics"}),
    )
    monkeypatch.setattr(
        module,
        "_load_full_coverage_required_intrinsics",
        lambda _path: {"alpha": ("molt_time_time",), "_intrinsics": ()},
    )
    monkeypatch.setattr(
        sys,
        "argv",
        [
            "check_stdlib_intrinsics.py",
            "--update-doc",
            "--intrinsic-partial-ratchet-file",
            str(ratchet),
        ],
    )

    assert module.main() == 0


def _source_inventory_fixture(tmp_path, sources):
    paths = {}
    for name, source in sources.items():
        path = tmp_path / (name.replace(".", "/") + ".py")
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(source, encoding="utf-8")
        paths[name] = path
    classification = check_stdlib_intrinsics.classify_stdlib_module_statuses(
        paths, target_python=check_stdlib_intrinsics._DEFAULT_TARGET_PYTHON_VERSION
    )
    return classification


def _inventory_owners(classification):
    return {
        owner
        for evidence in classification.import_evidence.values()
        for site in evidence.sites
        for owner in site.owner_plan.modules
    }


@pytest.mark.parametrize(
    "provider", ["missing", "stub", "policy", "missing-child", "opaque", "compiled"]
)
def test_source_inventory_distinguishes_absent_unknown_and_implemented_providers(
    tmp_path, provider
):
    sources = {"consumer": "try:\n    import provider\nexcept ImportError:\n    pass\n"}
    if provider == "stub":
        sources["provider"] = _stub_source("provider")
    elif provider == "policy":
        sources["provider"] = "raise ImportError('not admitted')\n"
    elif provider == "missing-child":
        sources["provider"] = "import pkg.missing\n"
        sources["pkg"] = "VALUE = 1\n"
    elif provider == "opaque":
        sources["provider"] = "__import__(requested_name)\n"
    elif provider == "compiled":
        sources["provider"] = "VALUE = 1\n"
    classification = _source_inventory_fixture(tmp_path, sources)
    assert "provider" in _inventory_owners(classification)
    assert (
        classification.statuses["consumer"]
        == check_stdlib_intrinsics.STATUS_PYTHON_COMPILED
    )
    if provider == "opaque":
        assert any(
            site.unresolved for site in classification.import_evidence["provider"].sites
        )
    elif provider == "missing-child":
        assert "pkg.missing" in _inventory_owners(classification)
    elif provider == "stub":
        assert (
            classification.statuses["provider"] == check_stdlib_intrinsics.STATUS_STUB
        )
    elif provider == "policy":
        assert (
            classification.statuses["provider"]
            == check_stdlib_intrinsics.STATUS_POLICY_GATE
        )
    elif provider == "missing":
        assert "provider" not in classification.statuses


@pytest.mark.parametrize("child", [False, True])
def test_source_inventory_distinguishes_fromlist_attributes_from_existing_children(
    tmp_path, child
):
    sources = {
        "consumer": "try:\n    from pkg import Value\nexcept ImportError:\n    pass\n",
        "pkg": "Value = 1\n",
    }
    if child:
        sources["pkg.Value"] = _stub_source("pkg.Value")
    classification = _source_inventory_fixture(tmp_path, sources)
    evidence = classification.import_evidence["consumer"]
    assert evidence.proven_modules == {"pkg", "pkg.Value"}
    assert _inventory_owners(classification) == {"pkg"}
    if child:
        assert (
            classification.statuses["pkg.Value"] == check_stdlib_intrinsics.STATUS_STUB
        )


def test_source_inventory_cycle_keeps_named_absence_without_failure_propagation(
    tmp_path,
):
    sources = {
        "consumer": "try:\n    import a\n    import b\nexcept ImportError:\n    pass\n",
        "a": "import b\n",
        "b": "import a\nimport missing\n",
    }
    classification = _source_inventory_fixture(tmp_path, sources)
    assert classification.import_evidence["a"].proven_modules == {"b"}
    assert classification.import_evidence["b"].proven_modules == {"a", "missing"}
    assert "missing" not in classification.statuses
    sources["b"] = "import a\n"
    classification = _source_inventory_fixture(tmp_path, sources)
    assert classification.import_evidence["b"].proven_modules == {"a"}


def test_source_inventory_does_not_certify_candidate_import_success_after_edit(
    tmp_path, monkeypatch
):
    module, root, _, report = _configure_report_test(
        tmp_path, monkeypatch, "--update-doc"
    )
    (root / "consumer.py").write_text(
        "try:\n    import provider\nexcept ImportError:\n    pass\n", encoding="utf-8"
    )
    provider = root / "provider.py"
    provider.write_text("VALUE = 1\n", encoding="utf-8")
    _configure_required_top_level(module, monkeypatch, root)
    assert module.main() == 0
    provider.write_text("import absent.child\n", encoding="utf-8")
    assert module.main() == 0
    payload = json.loads(report.read_text(encoding="utf-8"))
    assert payload["ok"] is True
    assert (
        next(row for row in payload["modules"] if row["module"] == "provider")["status"]
        == "python-compiled"
    )


@pytest.mark.parametrize("root_kind", ["bootstrap", "explicit"])
def test_selected_source_root_keeps_named_absence_without_inventing_runtime_success(
    tmp_path, monkeypatch, root_kind
):
    options = () if root_kind == "bootstrap" else ("--allowlist-modules", "consumer")
    module, root, _doc, report = _configure_report_test(
        tmp_path, monkeypatch, "--update-doc", *options
    )
    name = "sys" if root_kind == "bootstrap" else "consumer"
    (root / f"{name}.py").write_text("import absent.child\n", encoding="utf-8")
    _configure_required_top_level(module, monkeypatch, root)
    assert module.main() == 0
    classification = module._classify_stdlib_modules(
        {name: root / f"{name}.py"}, module._DEFAULT_TARGET_PYTHON_VERSION
    )
    assert classification.import_evidence[name].proven_modules == {"absent.child"}


@pytest.mark.parametrize("dynamic", [False, True])
@pytest.mark.parametrize("stub_child", [False, True])
@pytest.mark.parametrize("export_kind", ["literal", "getattr"])
def test_package_star_source_inventory_preserves_actual_import_names_and_unknowns(
    tmp_path, monkeypatch, dynamic, stub_child, export_kind
):
    module = check_stdlib_intrinsics
    package_name = "_molt_policy_star_oracle"
    package = tmp_path / package_name
    package.mkdir()
    exports = (
        "__all__ = ['child']\n"
        if export_kind == "literal"
        else "def __getattr__(name):\n"
        "    if name == '__all__': return ['child']\n"
        "    raise AttributeError(name)\n"
    )
    (package / "__init__.py").write_text(exports, encoding="utf-8")
    child = package / "child.py"
    child.write_text(
        _stub_source(package_name + ".child") if stub_child else "VALUE = 1\n",
        encoding="utf-8",
    )
    statement = (
        f"__import__({package_name!r}, fromlist=('*',))"
        if dynamic
        else f"from {package_name} import *"
    )
    source = f"try:\n    {statement}\nexcept ImportError:\n    pass\n"
    consumer = tmp_path / "consumer.py"
    consumer.write_text(source, encoding="utf-8")
    monkeypatch.syspath_prepend(str(tmp_path))
    imported_names = (package_name, package_name + ".child")
    absent = object()
    prior = {name: sys.modules.get(name, absent) for name in imported_names}
    try:
        namespace = {}
        exec(
            compile(source, "<package-star-oracle>", "exec", dont_inherit=True),
            namespace,
        )
        # CPython actually imports the declared child for both spellings.
        assert imported_names[1] in sys.modules
        if not dynamic:
            assert namespace["child"] is sys.modules[imported_names[1]]
    finally:
        for name, value in prior.items():
            if value is absent:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = value
    paths = {
        "consumer": consumer,
        package_name: package / "__init__.py",
        imported_names[1]: child,
    }
    classification = module.classify_stdlib_module_statuses(
        paths, target_python=module._DEFAULT_TARGET_PYTHON_VERSION
    )
    evidence = classification.import_evidence["consumer"]
    assert {owner for site in evidence.sites for owner in site.owner_plan.modules} == {
        package_name
    }
    if imported_names[1] not in evidence.proven_modules:
        assert any(site.unresolved for site in evidence.sites)
    else:
        assert evidence.proven_modules == set(imported_names)
    assert classification.statuses[imported_names[1]] == (
        module.STATUS_STUB if stub_child else module.STATUS_PYTHON_COMPILED
    )


@pytest.mark.parametrize(
    "source,swallowed",
    [
        ("try:\n    thunk = lambda: _MOLT_OP()\nexcept Exception:\n    pass\n", False),
        ("try:\n    (lambda: _MOLT_OP())()\nexcept Exception:\n    pass\n", True),
        (
            "def invoke():\n    _MOLT_OP()\n"
            "try:\n    invoke()\nexcept Exception:\n    pass\n",
            True,
        ),
        (
            "def invoke():\n    _MOLT_OP()\nalias = invoke\n"
            "try:\n    alias()\nexcept Exception:\n    pass\n",
            True,
        ),
        ("def invoke():\n    _MOLT_OP()\ninvoke()\n", False),
        (
            "try:\n    list(_MOLT_OP() for item in (1,))\n"
            "except Exception:\n    pass\n",
            True,
        ),
        pytest.param(
            "class C:\n    def method(self): _MOLT_OP()\n"
            "try:\n    C()\nexcept Exception:\n    pass\n",
            False,
            id="class-construction-keeps-method-deferred",
        ),
        pytest.param(
            "class C:\n    def method(self): _MOLT_OP()\n"
            "try:\n    C().method()\nexcept Exception:\n    pass\n",
            True,
            id="instance-method-call-executes-body",
        ),
        pytest.param(
            "class C:\n    def method(self): _MOLT_OP()\n"
            "try:\n    method = C().method\nexcept Exception:\n    pass\n",
            False,
            id="bound-method-retrieval-keeps-body-deferred",
        ),
        pytest.param(
            "class C:\n    def __init__(self): _MOLT_OP()\n"
            "try:\n    C()\nexcept Exception:\n    pass\n",
            True,
            id="class-construction-executes-initializer",
        ),
        pytest.param(
            "def make():\n    return lambda: _MOLT_OP()\n"
            "try:\n    make()\nexcept Exception:\n    pass\n",
            False,
            id="returned-function-stays-deferred",
        ),
        pytest.param(
            "def make():\n    return lambda: _MOLT_OP()\n"
            "try:\n    make()()\nexcept Exception:\n    pass\n",
            True,
            id="returned-function-call-executes-body",
        ),
        pytest.param(
            "def make():\n    return lambda: _MOLT_OP()\n"
            "function = make()\ntry:\n    function()\nexcept Exception:\n    pass\n",
            True,
            id="returned-function-alias-call-executes-body",
        ),
        pytest.param(
            "def make():\n    return lambda: _MOLT_OP()\n"
            "function = make()\ntry:\n    retained = function\nexcept Exception:\n    pass\n",
            False,
            id="returned-function-alias-value-stays-deferred",
        ),
        pytest.param(
            "def target(): _MOLT_OP()\ndef make(): return target\n"
            "try:\n    make()()\nexcept Exception:\n    pass\n",
            True,
            id="returned-named-function-executes-body",
        ),
        pytest.param(
            "def target(): _MOLT_OP()\ndef inner(): return target\n"
            "def outer(): return inner()\n"
            "try:\n    outer()()\nexcept Exception:\n    pass\n",
            True,
            id="nested-returned-call-executes-body",
        ),
        pytest.param(
            "def use():\n"
            "    def make():\n        _MOLT_OP()\n        return lambda: None\n"
            "    function = make()\n"
            "    try:\n        function()\n    except Exception:\n        pass\n"
            "use()\n",
            False,
            id="stable-local-returned-call-does-not-reexecute-factory",
        ),
        pytest.param(
            "def generator():\n    yield _MOLT_OP()\n"
            "def make(): return generator\n"
            "try:\n    held = make()()\nexcept Exception:\n    pass\n",
            False,
            id="returned-generator-factory-stays-suspended",
        ),
        pytest.param(
            "def generator():\n    yield _MOLT_OP()\n"
            "def make(): return generator\n"
            "try:\n    list(make()())\nexcept Exception:\n    pass\n",
            True,
            id="returned-generator-result-executes-on-consumption",
        ),
        pytest.param(
            "class Base:\n    def method(self): _MOLT_OP()\n"
            "class Derived(Base): pass\n"
            "try:\n    Derived().method()\nexcept Exception:\n    pass\n",
            True,
            id="inherited-method-call-executes-body",
        ),
        pytest.param(
            "class Callable:\n    def __call__(self): _MOLT_OP()\n"
            "try:\n    Callable()()\nexcept Exception:\n    pass\n",
            True,
            id="returned-instance-invokes-call-protocol",
        ),
        pytest.param(
            "def make(): return [lambda: _MOLT_OP()]\n"
            "try:\n    make()[0]()\nexcept Exception:\n    pass\n",
            True,
            id="returned-container-element-call",
        ),
        pytest.param(
            "def make(): return [lambda: _MOLT_OP()]\n"
            "try:\n    held = make()[0]\nexcept Exception:\n    pass\n",
            False,
            id="returned-container-element-retrieval",
        ),
        pytest.param(
            "def make(): return [lambda: _MOLT_OP()]\n"
            "try:\n    make()()\nexcept Exception:\n    pass\n",
            False,
            id="returned-container-is-not-its-callable-content",
        ),
        pytest.param(
            "def make(): return (lambda: _MOLT_OP(),)\n"
            "try:\n    function, = make()\n    function()\nexcept Exception:\n    pass\n",
            True,
            id="returned-unpacked-callable",
        ),
        pytest.param(
            "def make(): return (lambda: _MOLT_OP(),)\n"
            "try:\n    list(make())\nexcept Exception:\n    pass\n",
            False,
            id="returned-container-iteration-does-not-call-elements",
        ),
        pytest.param(
            "def generate():\n    yield lambda: _MOLT_OP()\n"
            "try:\n    for function in generate(): function()\nexcept Exception:\n    pass\n",
            True,
            id="yielded-callable-invocation",
        ),
        pytest.param(
            "def generate():\n    yield from (lambda: _MOLT_OP(),)\n"
            "try:\n    for function in generate(): function()\nexcept Exception:\n    pass\n",
            True,
            id="delegated-yield-callable-invocation",
        ),
        pytest.param(
            "def make(): return (lambda: _MOLT_OP() for item in (1,))\n"
            "try:\n    for function in make(): function()\nexcept Exception:\n    pass\n",
            True,
            id="returned-generator-expression-callable",
        ),
    ],
)
def test_python_exception_observations_do_not_change_source_implementation_status(
    tmp_path, source, swallowed
):
    calls = []
    failure = RuntimeError("provider refused")

    def operation():
        calls.append("called")
        raise failure

    propagated = False
    try:
        exec(
            compile(source, "<guarded-invocation-oracle>", "exec", dont_inherit=True),
            {"_MOLT_OP": operation},
        )
    except RuntimeError as error:
        assert error is failure
        propagated = True
    assert (bool(calls) and not propagated) is swallowed
    classification = _source_inventory_fixture(tmp_path, {"consumer": source})
    assert (
        classification.statuses["consumer"]
        == check_stdlib_intrinsics.STATUS_PYTHON_COMPILED
    )


@pytest.mark.parametrize(
    "source,swallowed",
    [
        ("def f():\n    try: _MOLT_OP()\n    except Exception: pass\nf()\n", True),
        ("def f():\n    _MOLT_OP()\ntry:\n    f()\nfinally:\n    pass\n", False),
        (
            "def f():\n    _MOLT_OP()\ndef g():\n    f()\ntry:\n    g()\nexcept Exception:\n    pass\n",
            True,
        ),
        (
            "def f():\n    yield _MOLT_OP()\ntry:\n    value = f()\nexcept Exception:\n    pass\n",
            False,
        ),
        (
            "def f():\n    yield _MOLT_OP()\ntry:\n    list(f())\nexcept Exception:\n    pass\n",
            True,
        ),
        (
            "def f():\n    _MOLT_OP()\ntry:\n    pass\nexcept Exception:\n    f()\n",
            False,
        ),
    ],
)
def test_python_resume_error_observations_do_not_change_source_implementation_status(
    tmp_path, source, swallowed
):
    calls = []
    failure = RuntimeError("provider refused")

    def operation():
        calls.append("called")
        raise failure

    propagated = False
    try:
        exec(
            compile(source, "<source-demand-oracle>", "exec", dont_inherit=True),
            {"_MOLT_OP": operation},
        )
    except RuntimeError as error:
        assert error is failure
        propagated = True
    assert (bool(calls) and not propagated) is swallowed
    classification = _source_inventory_fixture(tmp_path, {"consumer": source})
    assert (
        classification.statuses["consumer"]
        == check_stdlib_intrinsics.STATUS_PYTHON_COMPILED
    )


def test_source_inventory_keeps_imported_alias_without_handler_inference(tmp_path):
    module = check_stdlib_intrinsics
    helper = tmp_path / "helper.py"
    helper.write_text("def invoke():\n    _MOLT_OP()\n", encoding="utf-8")
    consumer = tmp_path / "consumer.py"
    consumer.write_text(
        "from helper import invoke as alias\ntry:\n    alias()\nexcept Exception:\n    pass\n",
        encoding="utf-8",
    )
    classification = module.classify_stdlib_module_statuses(
        {"helper": helper, "consumer": consumer},
        target_python=module._DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert _inventory_owners(classification) == {"helper"}
    assert classification.statuses["helper"] == module.STATUS_PYTHON_COMPILED


@pytest.mark.parametrize(
    "declaration",
    [
        "__all__ = make_names()\n",
        "if flag: __all__ = ['child']\n",
        "__all__ = ['first']\n__all__[:] = ['child']\n",
        "__all__ = ['first']\nalias = __all__\nalias.append('child')\n",
        "globals()['__all__'] = ['child']\n",
    ],
)
def test_unknown_package_exports_are_not_absent_exports(tmp_path, declaration):
    module = check_stdlib_intrinsics
    package = tmp_path / "pkg"
    package.mkdir()
    init = package / "__init__.py"
    init.write_text(declaration, encoding="utf-8")
    consumer = tmp_path / "consumer.py"
    consumer.write_text(
        "try:\n    from pkg import *\nexcept ImportError:\n    pass\n", encoding="utf-8"
    )
    classification = module.classify_stdlib_module_statuses(
        {"pkg": init, "consumer": consumer},
        target_python=module._DEFAULT_TARGET_PYTHON_VERSION,
    )
    [site] = classification.import_evidence["consumer"].sites
    assert site.unresolved and site.plan.requires_runtime


@pytest.mark.parametrize("invoke", [False, True])
def test_cyclic_imported_alias_effect_uses_actual_execution(
    tmp_path, monkeypatch, invoke
):
    # Real import cycles and aliases, with one actual callback exception oracle.
    left_name, right_name = "_molt_effect_cycle_left", "_molt_effect_cycle_right"
    left = tmp_path / (left_name + ".py")
    right = tmp_path / (right_name + ".py")
    left.write_text(
        f"import {right_name} as right\ndef invoke():\n    _MOLT_OP()\n",
        encoding="utf-8",
    )
    right.write_text(
        f"import {left_name} as left\ndef forward():\n    left.invoke()\n",
        encoding="utf-8",
    )
    source = (
        f"from {right_name} import forward as alias\ntry:\n    "
        + ("alias()" if invoke else "saved = alias")
        + "\nexcept Exception:\n    pass\n"
    )
    consumer = tmp_path / "consumer.py"
    consumer.write_text(source, encoding="utf-8")
    calls = []
    failure = RuntimeError("native operation refused")

    def operation():
        calls.append("called")
        raise failure

    absent = object()
    prior = {name: sys.modules.get(name, absent) for name in (left_name, right_name)}
    monkeypatch.syspath_prepend(str(tmp_path))
    try:
        left_module = __import__(left_name)
        left_module._MOLT_OP = operation
        exec(compile(source, "<imported-effect-oracle>", "exec", dont_inherit=True), {})
        assert bool(calls) is invoke
    finally:
        for name, value in prior.items():
            if value is absent:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = value
    classification = check_stdlib_intrinsics.classify_stdlib_module_statuses(
        {"consumer": consumer, left_name: left, right_name: right},
        target_python=check_stdlib_intrinsics._DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert classification.import_evidence[left_name].proven_modules == {right_name}
    assert classification.import_evidence[right_name].proven_modules == {left_name}


@pytest.mark.parametrize("local_guard", [False, True])
def test_retirement_does_not_inherit_caller_exception_handler(tmp_path, local_guard):
    body = (
        "        try: _MOLT_OP()\n        except Exception: pass\n"
        if local_guard
        else "        _MOLT_OP()\n"
    )
    source = (
        "class C:\n    def __del__(self):\n"
        + body
        + "try:\n    C()\nexcept Exception:\n    raise AssertionError('not unraisable')\n"
    )
    # CPython's finalizer hook belongs to the interpreter, so observe it in an
    # isolated child rather than replacing pytest's process-wide hook.
    observed = run_guarded_test_process(
        [
            sys.executable,
            "-I",
            "-c",
            "import sys\n"
            "failures = []\n"
            "calls = []\n"
            "failure = RuntimeError('finalizer provider refused')\n"
            "def operation():\n"
            "    calls.append('called')\n"
            "    raise failure\n"
            "sys.unraisablehook = lambda event: failures.append(event.exc_value)\n"
            "exec(compile(sys.argv[1], '<retirement-oracle>', 'exec', "
            "dont_inherit=True), {'_MOLT_OP': operation})\n"
            "assert calls == ['called'], calls\n"
            "assert failures == ([] if sys.argv[2] == '1' else [failure]), failures\n",
            source,
            str(int(local_guard)),
        ],
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert observed.returncode == 0, observed.stderr
    classification = _source_inventory_fixture(tmp_path, {"consumer": source})
    assert (
        classification.statuses["consumer"]
        == check_stdlib_intrinsics.STATUS_PYTHON_COMPILED
    )


@pytest.mark.parametrize("placement", ["direct", "callee"])
@pytest.mark.parametrize(
    "handler,propagates",
    [
        ("cleanup.append('done')\nraise", True),
        ("raise RuntimeError('replacement')", True),
        ("if flag:\n    raise\nelse:\n    raise RuntimeError('replacement')", True),
        ("if flag:\n    raise", False),
        ("return None", False),
        ("raise\npass", True),
        ("try:\n    raise\nfinally:\n    cleanup.append('done')", True),
        ("try:\n    raise\nfinally:\n    return None", False),
        ("try:\n    raise\nexcept ImportError:\n    pass", False),
        ("break", False),
        ("continue", False),
    ],
    ids=[
        "cleanup-reraise",
        "replacement",
        "both-branches-raise",
        "conditional-suppress",
        "return",
        "unreachable-pass",
        "finally-propagates",
        "finally-suppresses",
        "nested-suppresses",
        "break",
        "continue",
    ],
)
def test_python_import_error_completion_is_separate_from_source_inventory(
    tmp_path, placement, handler, propagates
):
    import builtins
    import textwrap

    definitions = "def load():\n    import provider\n" if placement == "callee" else ""
    operation = "load()" if placement == "callee" else "import provider"
    body = textwrap.indent(handler, "                ")
    source = (
        definitions + "def entry(flag):\n    for _ in (0,):\n"
        "        try:\n            " + operation + "\n"
        "        except Exception:\n" + body + "\n"
    )
    marker = ImportError("independent missing provider")

    def missing_provider(name, *args, **kwargs):
        assert name == "provider"
        raise marker

    code = compile(source, "<handler-completion-oracle>", "exec")
    observed = []
    for flag in (False, True):
        namespace = {
            "__builtins__": {**vars(builtins), "__import__": missing_provider},
            "cleanup": [],
        }
        exec(code, namespace)
        try:
            namespace["entry"](flag)
        except Exception as error:
            assert error is marker or (
                type(error) is RuntimeError and str(error) == "replacement"
            )
            observed.append(True)
        else:
            observed.append(False)
        assert namespace["cleanup"] == (["done"] if "cleanup.append" in handler else [])
    assert all(observed) is propagates
    classification = _source_inventory_fixture(tmp_path, {"consumer": source})
    assert "provider" in _inventory_owners(classification)
    assert "provider" not in classification.statuses


@pytest.mark.parametrize(
    "finalizer,may_suppress",
    [
        ("return None", True),
        ("break", True),
        ("continue", True),
        ("if flag:\n    return None", True),
        ("cleanup.append('done')", False),
        ("raise RuntimeError('replacement')", False),
    ],
)
def test_finally_completion_can_override_a_reraising_handler(
    tmp_path, finalizer, may_suppress
):
    import builtins
    import textwrap

    source = (
        "def entry(flag):\n    for _ in (0,):\n"
        "        try:\n            import provider\n"
        "        except Exception:\n            raise\n"
        "        finally:\n" + textwrap.indent(finalizer, "            ") + "\n"
    )
    marker = ImportError("independent missing provider")

    def missing_provider(name, *args, **kwargs):
        assert name == "provider"
        raise marker

    code = compile(source, "<finally-completion-oracle>", "exec")
    suppressed = []
    for flag in (False, True):
        namespace = {
            "__builtins__": {**vars(builtins), "__import__": missing_provider},
            "cleanup": [],
        }
        exec(code, namespace)
        try:
            namespace["entry"](flag)
        except Exception as error:
            assert error is marker or (
                type(error) is RuntimeError and str(error) == "replacement"
            )
            suppressed.append(False)
        else:
            suppressed.append(True)
    assert any(suppressed) is may_suppress
    classification = _source_inventory_fixture(tmp_path, {"consumer": source})
    assert "provider" in _inventory_owners(classification)
    assert "provider" not in classification.statuses


@pytest.mark.parametrize("suspension", ["handler", "finally"])
def test_resumable_handler_never_proves_transparent_propagation(tmp_path, suspension):
    import builtins

    handler = "yield 'fallback'\n        raise" if suspension == "handler" else "raise"
    finalizer = (
        "    finally:\n        yield 'fallback'\n" if suspension == "finally" else ""
    )
    source = (
        "def entry():\n    try:\n        import provider\n"
        "    except Exception:\n        " + handler + "\n" + finalizer
    )
    marker = ImportError("independent missing provider")

    def missing_provider(name, *args, **kwargs):
        assert name == "provider"
        raise marker

    namespace = {"__builtins__": {**vars(builtins), "__import__": missing_provider}}
    exec(compile(source, "<suspended-propagation-oracle>", "exec"), namespace)
    generator = namespace["entry"]()
    assert next(generator) == "fallback"
    generator.close()
    classification = _source_inventory_fixture(tmp_path, {"consumer": source})
    assert "provider" in _inventory_owners(classification)
    assert "provider" not in classification.statuses


def _context_phase_source(failure, asynchronous, placement, wrapped):
    import textwrap

    enter = "__aenter__" if asynchronous else "__enter__"
    exit = "__aexit__" if asynchronous else "__exit__"
    prefix = "async " if asynchronous else ""
    operation = "import provider" if placement == "direct" else "load()"
    helper = "def load():\n    import provider\n"
    sources = {"helper": helper} if placement == "imported" else {}
    definitions = (
        "from helper import load\n"
        if placement == "imported"
        else helper
        if placement == "local"
        else ""
    )
    definitions += (
        f"class Outer:\n    {prefix}def {enter}(self): return self\n"
        f"    {prefix}def {exit}(self, *args): return True\n"
        "class Pair:\n    def __iter__(self):\n"
        + textwrap.indent(operation, "        ")
        + "\n        return iter((1, 2))\n"
        "class Guard:\n"
    )
    receiver = "*args" if failure == "target-varargs" else "self"
    entered = (
        operation
        if failure in {"first-enter", "second-enter"}
        else "return Pair()"
        if failure == "target-pair"
        else "return args[0]"
        if failure == "target-varargs"
        else "return self"
    )
    definitions += (
        f"    {prefix}def {enter}({receiver}):\n"
        + textwrap.indent(entered, "        ")
        + "\n"
    )
    definitions += (
        f"    {prefix}def {exit}(self, *args):\n"
        + textwrap.indent(
            operation
            if failure in {"first-exit", "inner-exit", "first-exit-as"}
            else "return True",
            "        ",
        )
        + "\n"
    )
    definitions += (
        "    def __iter__(self):\n"
        + textwrap.indent(operation, "        ")
        + "\n        return iter((1, 2))\n"
    )
    definitions += "    def unused(self):\n        import unused_provider\n"
    definitions += (
        "def make():\n" + textwrap.indent(operation, "    ") + "\n    return Guard()\n"
    )
    managers = (
        "make()"
        if failure == "first-expression"
        else "Guard(), make()"
        if failure == "second-expression"
        else "Outer(), Guard()"
        if failure in {"second-enter", "inner-exit"}
        else "Guard() as (a, b)"
        if failure.startswith("target-")
        else "Guard() as value"
        if failure == "first-exit-as"
        else "Guard()"
    )
    body = f"{prefix}with {managers}:\n" + textwrap.indent(
        operation if failure == "body" else "pass", "    "
    )
    if wrapped:
        body = (
            "try:\n"
            + textwrap.indent(body, "    ")
            + "\nexcept ImportError:\n    raise"
        )
    sources["consumer"] = (
        definitions + f"{prefix}def entry():\n" + textwrap.indent(body, "    ") + "\n"
    )
    return sources


@pytest.mark.parametrize("wrapped", [False, True], ids=["bare", "reraising-caller"])
@pytest.mark.parametrize("placement", ["direct", "local", "imported"])
@pytest.mark.parametrize("asynchronous", [False, True], ids=["with", "async-with"])
@pytest.mark.parametrize(
    "failure",
    [
        "first-expression",
        "second-expression",
        "first-enter",
        "second-enter",
        "first-exit",
        "inner-exit",
        "first-exit-as",
        "body",
        "target-pair",
        "target-self",
        "target-varargs",
    ],
)
def test_python_context_error_observations_preserve_named_source_imports(
    tmp_path, failure, asynchronous, placement, wrapped
):
    import builtins
    import types

    sources = _context_phase_source(failure, asynchronous, placement, wrapped)
    marker = ImportError("independent missing provider")
    attempted = []
    helper = types.ModuleType("helper")

    def source_import(name, *args, **kwargs):
        if name == "helper":
            return helper
        assert name == "provider"
        attempted.append(name)
        raise marker

    environment = {**vars(builtins), "__import__": source_import}
    if placement == "imported":
        helper.__dict__["__builtins__"] = environment
        exec(
            compile(sources["helper"], "<phase-helper-oracle>", "exec"), helper.__dict__
        )
    namespace = {"__builtins__": environment}
    exec(compile(sources["consumer"], "<context-phase-oracle>", "exec"), namespace)
    try:
        if asynchronous:
            coroutine = namespace["entry"]()
            try:
                coroutine.send(None)
            except StopIteration:
                pass
            else:
                raise AssertionError("oracle unexpectedly suspended")
        else:
            namespace["entry"]()
    except ImportError as error:
        assert error is marker
        suppressed = False
    else:
        suppressed = True
    assert attempted == ["provider"]
    assert suppressed == (
        failure
        not in {"first-expression", "first-enter", "first-exit", "first-exit-as"}
    )
    classification = _source_inventory_fixture(tmp_path, sources)
    assert "provider" in _inventory_owners(classification)
    assert "provider" not in classification.statuses


@pytest.mark.parametrize("writes_flag", [False, True])
def test_inventory_namespace_premise_matches_actual_foreign_write(
    tmp_path, writes_flag
):
    import builtins
    import types

    from molt.stdlib_intrinsic_policy import classify_stdlib_module_statuses
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

    sources = {
        "consumer": "flag = False\nimport writer\nif flag:\n    import provider\n",
        "writer": "import consumer\nconsumer.flag = True\n"
        if writes_flag
        else "pass\n",
    }
    modules = {name: types.ModuleType(name) for name in sources}
    attempts = []

    def source_import(name, *args, **kwargs):
        if name == "provider":
            attempts.append(name)
            raise ImportError("independent missing provider")
        module = modules[name]
        if name == "writer":
            exec(compile(sources[name], name, "exec"), module.__dict__)
        return module

    for module in modules.values():
        module.__dict__["__builtins__"] = {
            **vars(builtins),
            "__import__": source_import,
        }
    try:
        exec(
            compile(sources["consumer"], "consumer", "exec"),
            modules["consumer"].__dict__,
        )
    except ImportError:
        assert attempts == ["provider"]
    graph = {}
    for name, source in sources.items():
        path = tmp_path / f"{name}.py"
        path.write_text(source, encoding="utf-8")
        graph[name] = path
    classification = classify_stdlib_module_statuses(
        graph, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    actual = classification.import_evidence["consumer"].proven_modules
    assert ("provider" in actual) == bool(attempts)


def test_inventory_rejects_source_namespace_change_during_classification(tmp_path):
    from molt.compiler_analysis.python_imports import UnresolvedStaticImportError
    from molt.stdlib_intrinsic_policy import (
        classify_stdlib_module_statuses,
        stdlib_namespace_inventory,
    )
    from molt.target_python import _DEFAULT_TARGET_PYTHON_VERSION

    consumer = tmp_path / "consumer.py"
    writer = tmp_path / "writer.py"
    consumer.write_text(
        "flag = False\nimport writer\nif flag:\n    import provider\n",
        encoding="utf-8",
    )
    writer.write_text("pass\n", encoding="utf-8")
    graph = {"consumer": consumer, "writer": writer}
    inventory = stdlib_namespace_inventory(
        graph, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    writer.write_text("import consumer\nconsumer.flag = True\n", encoding="utf-8")
    with pytest.raises(UnresolvedStaticImportError, match="namespace changed.*writer"):
        classify_stdlib_module_statuses(
            graph,
            target_python=_DEFAULT_TARGET_PYTHON_VERSION,
            namespace_inventory=inventory,
        )


@pytest.mark.parametrize("protected", [False, True], ids=["propagates", "suppressed"])
@pytest.mark.parametrize(
    "route",
    [
        "own",
        "foreign",
        "alias-own",
        "alias-foreign",
        "function-dict",
        "function-missing",
        "function-helper-missing",
    ],
)
def test_activation_publication_reference_observations_preserve_source_custody(
    tmp_path, route, protected
):
    import builtins
    import types

    sources, expected_provider = _activation_publication_sources(route, protected)
    modules = {name: types.ModuleType(name) for name in sources}
    loading = set()
    loaded = set()
    attempts = []
    marker = ImportError("independent activation provider failure")

    def source_import(name, *args, **kwargs):
        if name in {"original_provider", "replacement_provider"}:
            attempts.append(name)
            raise marker
        if name not in modules:
            return builtins.__import__(name, *args, **kwargs)
        module = modules[name]
        if name not in loaded and name not in loading:
            loading.add(name)
            try:
                exec(compile(sources[name], name, "exec"), module.__dict__)
            finally:
                loading.remove(name)
            loaded.add(name)
        return module

    environment = {**vars(builtins), "__import__": source_import}
    for module in modules.values():
        module.__dict__["__builtins__"] = environment
    source_import("consumer")
    try:
        modules["consumer"].entry()
    except ImportError as error:
        assert error is marker
        suppressed = False
    else:
        suppressed = True
    assert attempts == [expected_provider]
    assert suppressed is protected

    classification = _source_inventory_fixture(tmp_path, sources)
    assert expected_provider in _inventory_owners(classification)
    assert expected_provider not in classification.statuses


@pytest.mark.parametrize(
    "route",
    [
        "shared-return",
        "nested-def",
        "nested-lambda",
        "replace-globals",
        "replace-builtins",
        "code-read",
    ],
)
def test_activation_context_reference_observations_preserve_source_custody(
    tmp_path, route
):
    import builtins
    import types

    sources = _activation_context_sources(route)
    modules = {name: types.ModuleType(name) for name in sources}
    loaded = set()
    attempts = []
    markers = {
        name: ImportError(f"independent {name} failure")
        for name in ("left_provider", "right_provider")
    }

    def source_import(name, *args, **kwargs):
        if name in markers:
            attempts.append(name)
            raise markers[name]
        if name not in modules:
            return builtins.__import__(name, *args, **kwargs)
        if name not in loaded:
            loaded.add(name)
            exec(compile(sources[name], name, "exec"), modules[name].__dict__)
        return modules[name]

    environment = {**vars(builtins), "__import__": source_import}
    for module in modules.values():
        module.__dict__["__builtins__"] = environment
    consumer = source_import("consumer")
    if route == "code-read":
        assert consumer.entry() is consumer.dormant.__code__
        assert attempts == []
        assert consumer.seen == []
    else:
        assert consumer.a.__globals__ is consumer.left_namespace
        assert consumer.b.__globals__ is consumer.right_namespace
        if route.startswith("replace-"):
            assert len(consumer.replacement_code.co_freevars) == 1
            assert consumer.a.__closure__ is consumer.retained_closure
            assert consumer.b.__closure__ is consumer.retained_closure
            assert consumer.a.__code__ is consumer.replacement_code
            assert consumer.b.__code__ is consumer.replacement_code
        if route == "replace-builtins":
            assert consumer.a.__builtins__ is consumer.left_builtins
            assert consumer.b.__builtins__ is consumer.right_builtins
            assert consumer.a.__globals__["__builtins__"] is consumer.right_builtins
            assert consumer.b.__globals__["__builtins__"] is consumer.left_builtins
        with pytest.raises(ImportError) as raised:
            consumer.entry()
        assert raised.value is markers["right_provider"]
        assert attempts == ["left_provider", "right_provider"]
        assert len(consumer.seen) == 1
        assert consumer.seen[0] is markers["left_provider"]

    classification = _source_inventory_fixture(tmp_path, sources)
    assert (
        {"left_provider"} if route == "code-read" else set(markers)
    ) <= _inventory_owners(classification)
    assert not set(markers).intersection(classification.statuses)


@pytest.mark.parametrize("route", ACTIVATION_TARGET_ROUTES)
def test_activation_target_sources_preserve_physical_reference_observations(
    tmp_path: Path, route: str
) -> None:
    # These expectations are independent of the source projection. Observe the
    # real import machinery in a child, including failed-module execution and
    # retained exception identities, rather than replacing its import hook.
    expected_stdout = {
        "function-dict": ("function-dict\n['replacement_provider']\nTrue\n"),
        "function-missing": ("function-missing\n['replacement_provider']\nTrue\n"),
        "function-helper-missing": (
            "function-helper-missing\n['replacement_provider']\nTrue\n"
        ),
        "shared-return": (
            "shared-return\n['left_provider', 'right_provider']\n"
            "True\n[True]\n[True, True]\n"
        ),
        "nested-def": (
            "nested-def\n['left_provider', 'right_provider']\n"
            "True\n[True]\n[True, True]\n"
        ),
        "nested-lambda": (
            "nested-lambda\n['left_provider', 'right_provider']\n"
            "True\n[True]\n[True, True]\n"
        ),
        "replace-globals": (
            "replace-globals\n['left_provider', 'right_provider']\n"
            "True\n[True]\n[True, True, True, True, True, True, True]\n"
        ),
        "replace-builtins": (
            "replace-builtins\n['left_provider', 'right_provider']\n"
            "True\n[True]\n"
            "[True, True, True, True, True, True, True, True, True, True, True]\n"
        ),
    }[route]
    for module, source in activation_target_sources(route).items():
        (tmp_path / f"{module}.py").write_text(source, encoding="utf-8")
    observed = run_guarded_test_process(
        [sys.executable, "-B", str(tmp_path / "main.py")],
        cwd=tmp_path,
        capture_output=True,
        text=True,
        timeout=10,
    )
    assert observed.returncode == 0, observed.stdout + observed.stderr
    assert observed.stderr == ""
    assert observed.stdout == expected_stdout


def test_reconstructed_function_target_fixtures_are_current() -> None:
    from tools.gen_reconstructed_function_fixtures import generated_outputs

    for path, expected in generated_outputs().items():
        assert path.read_bytes() == expected.encode("utf-8"), path


def test_reconstructed_function_generator_rejects_obsolete_consumers(
    tmp_path: Path, monkeypatch
) -> None:
    from tools import gen_reconstructed_function_fixtures as generator

    # Removing a route or module must not silently leave a runnable old lane.
    monkeypatch.setattr(generator, "FIXTURES", tmp_path)
    monkeypatch.setattr(generator, "ROOT", tmp_path)
    (tmp_path / "obsolete.py").write_text("raise AssertionError\n", encoding="utf-8")
    with pytest.raises(ValueError, match="obsolete.py"):
        generator.generated_outputs()


@pytest.mark.parametrize(
    "source",
    [
        "import _py_helper\n",
        "from importlib import import_module as resolve\nresolve('_py_helper')\n",
        "__import__(name='_py_helper')\n",
    ],
)
def test_compiled_helper_spelling_is_not_host_python_fallback(tmp_path, source):
    classification = _source_inventory_fixture(
        tmp_path,
        {
            "consumer": source,
            "_py_helper": "VALUE = 1\n",
        },
    )
    assert (
        classification.statuses["consumer"]
        == check_stdlib_intrinsics.STATUS_PYTHON_COMPILED
    )
    assert (
        classification.statuses["_py_helper"]
        == check_stdlib_intrinsics.STATUS_PYTHON_COMPILED
    )
    assert check_stdlib_intrinsics._scan_file(tmp_path / "consumer.py")[0] == []


@pytest.mark.parametrize(
    "source",
    [
        "def require_intrinsic(value): return value\nresult = require_intrinsic('ordinary')\n",
        "def load_intrinsic(value): return value\nresult = load_intrinsic('ordinary')\n",
        "_molt_intrinsics = {'ordinary': 1}\nresult = _molt_intrinsics['ordinary']\n",
    ],
)
def test_local_loader_like_spelling_does_not_establish_intrinsic_identity(
    tmp_path, source
):
    classification = _source_inventory_fixture(tmp_path, {"consumer": source})
    assert classification.used_intrinsics["consumer"] == frozenset()
    assert (
        classification.statuses["consumer"]
        == check_stdlib_intrinsics.STATUS_PYTHON_COMPILED
    )
    assert check_stdlib_intrinsics._scan_file(tmp_path / "consumer.py")[0] == []
