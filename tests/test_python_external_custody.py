"""Editable import bytes, role ownership and finder boundaries share custody."""

from __future__ import annotations

from copy import deepcopy
import ast
import hashlib
import importlib.machinery as machinery
from pathlib import Path, PurePosixPath
import sys
from types import ModuleType
import unicodedata
import zipimport

import pytest

from molt import python_external_custody as external
from molt import python_file_node_custody as files
from molt.exact_json import canonical_json_sha256
from molt.python_environment_custody import _canonical_external_roots
from molt.python_identity_common import PythonEnvironmentIdentityError


@pytest.fixture
def standard_finders(monkeypatch):
    # The isolated capture child does not run pytest's assertion-rewrite finder.
    monkeypatch.setattr(
        sys,
        "meta_path",
        [machinery.BuiltinImporter, machinery.FrozenImporter, machinery.PathFinder],
    )
    monkeypatch.setattr(
        sys,
        "path_hooks",
        [
            zipimport.zipimporter,
            machinery.FileFinder.path_hook(
                (machinery.ExtensionFileLoader, machinery.EXTENSION_SUFFIXES),
                (machinery.SourceFileLoader, machinery.SOURCE_SUFFIXES),
                (machinery.SourcelessFileLoader, machinery.BYTECODE_SUFFIXES),
            ),
        ],
    )
    monkeypatch.setattr(sys, "path_importer_cache", {})


class _ExternalFixture:
    def __init__(self, root: Path):
        self.environment = root / "environment"
        self.site = self.environment / "site"
        self.site.mkdir(parents=True)
        self.source = root / "source"
        self.imports = self.source / "src"
        self.imports.mkdir(parents=True)
        (self.imports / "module.py").write_bytes(b"VALUE = 1\n")
        (self.imports / ".gitignore").write_bytes(b"ignored-data.bin\n")
        (self.imports / "ignored-data.bin").write_bytes(b"first")
        self.pth = self.site / "demo.pth"
        self.pth.write_text(str(self.imports) + "\n", encoding="utf-8", newline="\n")
        self.active = [
            {
                "role": "external-import-0",
                "owner": "external",
                "kind": "directory",
                "root": "external-root-0",
                "path": "src",
            }
        ]
        self.bootstrap_paths = []

    def capture(self):
        self.context = files.PythonFileCaptureContext()
        pool = files._FileNodePool(capture_context=self.context)
        tree, _paths, _metadata = files._stable_tree_inventory(
            self.environment, root_id="environment-root", label="fixture", pool=pool
        )
        self.entries = {row["path"]: row for row in tree["entries"]}
        self.nodes = {row["id"]: row for row in pool.nodes}
        self.distributions = [
            {
                "name": "demo",
                "external_source": {"root": "external-root-0", "path": "."},
                "installed_files": [
                    {
                        "path": "site/demo.pth",
                        "node": self.entries["site/demo.pth"]["node"],
                        "declared": None,
                    }
                ],
            }
        ]
        return external.capture_external_import_custody(
            environment_root=self.environment,
            external_roots=[("external-root-0", self.source)],
            active_import_roots=self.active,
            distributions=self.distributions,
            tree_entries=self.entries,
            tree_nodes=self.nodes,
            tree_pool=pool,
            site_roots=["site"],
            bootstrap_paths=self.bootstrap_paths,
            capture_context=self.context,
        )

    def validate(self, payload):
        external.validate_external_import_custody(
            payload,
            external_roots={"external-root-0": str(self.source)},
            active_import_roots=self.active,
            distributions=self.distributions,
            tree_entries=self.entries,
            tree_nodes=self.nodes,
            site_roots=["site"],
            bootstrap_paths=self.bootstrap_paths,
        )


@pytest.fixture
def editable(tmp_path, standard_finders):
    return _ExternalFixture(tmp_path)


def test_ignored_source_data_and_membership_change_editable_identity(editable):
    first = editable.capture()
    entries = first["trees"][0]["entries"]
    assert {row["path"] for row in entries} == {
        ".gitignore",
        "ignored-data.bin",
        "module.py",
    }
    assert editable.distributions[0]["external_source"]["import_roles"] == [
        "external-import-0"
    ]
    (editable.imports / "ignored-data.bin").write_bytes(b"other")
    second = editable.capture()
    assert canonical_json_sha256(first) != canonical_json_sha256(second)
    (editable.imports / "new-importable.py").write_bytes(b"NEW = True\n")
    third = editable.capture()
    assert canonical_json_sha256(second) != canonical_json_sha256(third)


def test_overlapping_import_regions_share_one_complete_tree(editable):
    package = editable.imports / "package"
    package.mkdir()
    (package / "__init__.py").write_bytes(b"PACKAGE = True\n")
    editable.active.append(
        {
            "role": "external-import-1",
            "owner": "external",
            "kind": "directory",
            "root": "external-root-0",
            "path": "src/package",
        }
    )
    with editable.pth.open("a", encoding="utf-8", newline="\n") as stream:
        stream.write(str(package) + "\n")
    payload = editable.capture()
    assert len(payload["trees"]) == 1
    tree = payload["trees"][0]
    assert tree["roles"] == [
        {"role": "external-import-0", "path": "."},
        {"role": "external-import-1", "path": "package"},
    ]
    assert editable.context.inventory_profile()["hashed_files"] == 1 + len(
        tree["file_nodes"]
    )
    editable.validate(payload)


def test_admission_and_runtime_forests_use_shared_role_ordered_ownership(tmp_path):
    parent = tmp_path / "source"
    child = parent / "src"
    child.mkdir(parents=True)
    environment = tmp_path / "environment"
    environment.mkdir()
    assert _canonical_external_roots([child, parent, child], environment) == (
        ("external-root-0", parent),
    )
    roots, roles = files._root_forest(
        [("b", PurePosixPath("owner/child")), ("a", PurePosixPath("owner"))],
        root_prefix="fixture-root",
    )
    assert roots == [("fixture-root-0", PurePosixPath("owner"))]
    assert roles == [
        {"role": "a", "root": "fixture-root-0", "path": "."},
        {"role": "b", "root": "fixture-root-0", "path": "child"},
    ]


@pytest.mark.parametrize(
    "fault",
    [
        "missing-tree",
        "duplicate-tree",
        "missing-role",
        "orphan-node",
        "missing-node",
        "wrong-manifest",
        "boolean-count",
        "non-directory-role",
        "missing-declaration",
        "wrong-declaration-content",
        "wrong-declaration-role",
        "wrong-policy",
    ],
)
def test_resealed_external_custody_rejects_incomplete_or_aliased_authority(
    editable, fault
):
    payload = deepcopy(editable.capture())
    tree = payload["trees"][0]
    if fault == "missing-tree":
        payload["trees"] = []
    elif fault == "duplicate-tree":
        payload["trees"].append(deepcopy(tree))
    elif fault == "missing-role":
        tree["roles"] = []
    elif fault == "orphan-node":
        tree["file_nodes"].append(
            {
                "id": f"file-node-{len(tree['file_nodes'])}",
                "size": 1,
                "sha256": "a" * 64,
            }
        )
    elif fault == "missing-node":
        tree["file_nodes"].pop()
    elif fault == "wrong-manifest":
        tree["manifest_sha256"] = "0" * 64
    elif fault == "boolean-count":
        tree["file_count"] = True
    elif fault == "non-directory-role":
        tree["roles"][0]["path"] = "module.py"
    elif fault == "missing-declaration":
        payload["path_declarations"] = []
    elif fault == "wrong-declaration-content":
        payload["path_declarations"][0]["content"] += "# modified\n"
    elif fault == "wrong-declaration-role":
        payload["path_declarations"][0]["roles"] = []
    else:
        payload["finder_policy"] = "allow-custom-hooks"
    with pytest.raises(PythonEnvironmentIdentityError):
        editable.validate(payload)


@pytest.mark.parametrize(
    "fault",
    ["missing-binding", "source-escape", "noneditable-owner", "unowned-declaration"],
)
def test_editable_distribution_must_own_its_declared_import_region(editable, fault):
    payload = editable.capture()
    distribution = editable.distributions[0]
    if fault == "missing-binding":
        distribution["external_source"]["import_roles"] = []
    elif fault == "source-escape":
        distribution["external_source"]["path"] = "other-source"
    elif fault == "noneditable-owner":
        distribution["external_source"] = None
    else:
        distribution["installed_files"] = []
    with pytest.raises(PythonEnvironmentIdentityError):
        editable.validate(payload)


@pytest.mark.parametrize(
    "directive",
    [
        "import editable_finder; editable_finder.install()",
        "import\teditor",
        "relative/source",
        "import _virtualenv; arbitrary()",
    ],
)
def test_executable_or_relative_pth_fails_before_external_scanning(
    editable, monkeypatch, directive
):
    editable.pth.write_text(directive + "\n", encoding="utf-8")
    original = external._stable_tree_inventory

    def reject_external_scan(root, **kwargs):
        pytest.fail(f"unsupported declaration reached external source scan: {root}")

    with monkeypatch.context() as scoped:
        scoped.setattr(external, "_stable_tree_inventory", reject_external_scan)
        with pytest.raises(PythonEnvironmentIdentityError):
            editable.capture()
    assert external._stable_tree_inventory is original


def test_unknown_uv_template_reports_observed_identity_without_fallback(editable):
    module = editable.site / "_virtualenv.py"
    content = b"# arbitrary code is not a reviewed virtualenv bootstrap\n"
    module.write_bytes(content)
    (editable.site / "_virtualenv.pth").write_bytes(b"import _virtualenv\n")
    editable.bootstrap_paths = ["site/_virtualenv.pth", "site/_virtualenv.py"]
    with pytest.raises(
        PythonEnvironmentIdentityError, match=hashlib.sha256(content).hexdigest()
    ):
        editable.capture()


@pytest.mark.parametrize(
    "fault", ["meta", "spoofed-virtualenv", "path-hook", "cached-finder", "cached-path"]
)
def test_custom_import_mechanisms_cannot_alias_the_standard_finder_policy(
    standard_finders, tmp_path, fault
):
    if fault == "meta":
        sys.meta_path.insert(0, object())
    elif fault == "spoofed-virtualenv":
        finder = type("_Finder", (), {"__module__": "_virtualenv"})()
        sys.meta_path.insert(0, finder)
    elif fault == "path-hook":
        sys.path_hooks.append(lambda path: None)
    elif fault == "cached-finder":
        sys.path_importer_cache[str(tmp_path)] = object()
    else:
        sys.path_importer_cache[str(tmp_path)] = machinery.FileFinder(
            str(tmp_path / "elsewhere")
        )
    with pytest.raises(PythonEnvironmentIdentityError, match="finder|hook"):
        external.validate_active_import_finders()


@pytest.mark.parametrize(
    "fault", [None, "module-digest", "provenance", "executable-line", "missing-pair"]
)
def test_reviewed_uv_policy_is_bound_to_exact_declarative_file_custody(fault):
    policy = external._UV_BOOTSTRAP_PROVENANCE
    data = b"import _virtualenv\n"
    nodes = {
        "file-node-0": {
            "id": "file-node-0",
            "size": len(data),
            "sha256": hashlib.sha256(data).hexdigest(),
        },
        "file-node-1": {
            "id": "file-node-1",
            "size": policy["size"],
            "sha256": policy["sha256"],
        },
    }
    entries = {
        "site/_virtualenv.pth": {
            "path": "site/_virtualenv.pth",
            "kind": "file",
            "node": "file-node-0",
        },
        "site/_virtualenv.py": {
            "path": "site/_virtualenv.py",
            "kind": "file",
            "node": "file-node-1",
        },
    }
    payload = external.empty_external_import_custody()
    payload["reviewed_startup"] = [
        {
            "capability": "uv-virtualenv.v1",
            "declaration": "site/_virtualenv.pth",
            "declaration_provenance": None,
            "module": {
                "path": "site/_virtualenv.py",
                "node": "file-node-1",
                "provenance": dict(policy),
            },
            "gate": {},
        }
    ]
    payload["finder_order"] = ["uv-virtualenv.v1"]
    payload["path_declarations"] = [
        {
            "path": "site/_virtualenv.pth",
            "node": "file-node-0",
            "content": data.decode(),
            "roles": [],
        }
    ]
    bootstrap_paths = list(entries)
    if fault == "module-digest":
        nodes["file-node-1"]["sha256"] = "0" * 64
    elif fault == "provenance":
        payload["reviewed_startup"][0]["module"]["provenance"]["git_blob_sha1"] = (
            "0" * 40
        )
    elif fault == "executable-line":
        payload["path_declarations"][0]["content"] = "import custom_hook\n"
    elif fault == "missing-pair":
        bootstrap_paths.pop()

    def validate():
        external.validate_external_import_custody(
            payload,
            external_roots={},
            active_import_roots=[],
            distributions=[],
            tree_entries=entries,
            tree_nodes=nodes,
            site_roots=["site"],
            bootstrap_paths=bootstrap_paths,
        )

    if fault is None:
        validate()
    else:
        with pytest.raises(PythonEnvironmentIdentityError):
            validate()


@pytest.mark.parametrize("name", ["COVERAGE_PROCESS_START", "COVERAGE_PROCESS_CONFIG"])
@pytest.mark.parametrize("value", [None, "", "configured"])
def test_coverage_startup_gate_is_precise_and_never_runs_active_configuration(
    monkeypatch, name, value
):
    monkeypatch.delenv("COVERAGE_PROCESS_START", raising=False)
    monkeypatch.delenv("COVERAGE_PROCESS_CONFIG", raising=False)
    if value is not None:
        monkeypatch.setenv(name, value)
    if value:
        with pytest.raises(PythonEnvironmentIdentityError, match=name):
            external._observe_startup_gate("coverage-inactive.v1")
    else:
        assert external._observe_startup_gate("coverage-inactive.v1") == {
            "COVERAGE_PROCESS_START": False,
            "COVERAGE_PROCESS_CONFIG": False,
        }


@pytest.mark.parametrize(
    "value,sentinel",
    [
        (None, False),
        ("local", False),
        ("stdlib", False),
        ("unexpected", False),
        ("local", True),
    ],
)
def test_setuptools_finder_gate_binds_env_and_cpython_build_branch(
    tmp_path, monkeypatch, value, sentinel
):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("SETUPTOOLS_USE_DISTUTILS", raising=False)
    if value is not None:
        monkeypatch.setenv("SETUPTOOLS_USE_DISTUTILS", value)
    if sentinel:
        (tmp_path / "pybuilddir.txt").write_bytes(b"build/source")
    if sentinel or value not in {None, "local"}:
        with pytest.raises(PythonEnvironmentIdentityError, match="setuptools"):
            external._observe_startup_gate("setuptools-local-distutils.v1")
    else:
        assert external._observe_startup_gate("setuptools-local-distutils.v1") == {
            "SETUPTOOLS_USE_DISTUTILS": "local",
            "cwd_pybuilddir_file": False,
        }


@pytest.mark.parametrize(
    "fault", [None, "code", "defaults", "import-global", "class-member"]
)
def test_shared_reviewed_code_comparison_rejects_live_drift(fault):
    # This is synthetic test code, not a copied upstream bootstrap or finder.
    source = b"import sys\n\nclass Sample:\n def find_spec(self, name, path=None):\n  return None\n"
    module = ModuleType("synthetic_startup")
    compiled = compile(source, "synthetic_startup.py", "exec", dont_inherit=True)
    exec(compiled, vars(module))
    if fault == "code":
        module.Sample.find_spec = lambda self, name, path=None: True
    elif fault == "defaults":
        module.Sample.find_spec.__defaults__ = ("changed",)
    elif fault == "import-global":
        module.sys = object()
    elif fault == "class-member":
        module.Sample.other_dispatch = lambda self: True

    def verify():
        external._verify_code_owner(module, ast.parse(source).body, compiled, module)

    if fault is None:
        verify()
    else:
        with pytest.raises(PythonEnvironmentIdentityError):
            verify()


@pytest.mark.parametrize(
    "fault", [None, "ownership", "version", "bytes", "active-coverage"]
)
def test_bounded_startup_consumer_needs_owned_reviewed_inactive_bytes(
    tmp_path, monkeypatch, standard_finders, fault
):
    site = tmp_path / "site"
    site.mkdir()
    path = site / "a1_coverage.pth"
    # A synthetic reviewed artifact isolates policy dispatch without vendoring
    # executable upstream startup code into this repository.
    source = b"import synthetic_coverage_startup\n"
    path.write_bytes(source if fault != "bytes" else source + b"# changed\n")
    policies = deepcopy(external._STARTUP_CAPABILITIES)
    artifact = {
        "project": "synthetic",
        "version": "7.14.3",
        "path": "a1_coverage.pth",
        "sha256": hashlib.sha256(source).hexdigest(),
        "size": len(source),
    }
    policies["coverage-inactive.v1"]["declaration_artifacts"] = [artifact]
    monkeypatch.setattr(external, "_STARTUP_CAPABILITIES", policies)
    monkeypatch.delenv("COVERAGE_PROCESS_START", raising=False)
    monkeypatch.delenv("COVERAGE_PROCESS_CONFIG", raising=False)
    if fault == "active-coverage":
        monkeypatch.setenv("COVERAGE_PROCESS_START", "test-configuration")
    context = files.PythonFileCaptureContext()
    pool = files._FileNodePool(capture_context=context)
    node = pool.bind(path, path.stat(), label="synthetic startup")
    nodes = {row["id"]: row for row in pool.nodes}
    entries = {
        "site/a1_coverage.pth": {
            "path": "site/a1_coverage.pth",
            "kind": "file",
            "node": node,
        }
    }
    owners = [
        {
            "name": "other" if fault == "ownership" else "coverage",
            "version": "0" if fault == "version" else "7.14.3",
            "installed_files": [{"path": "site/a1_coverage.pth", "node": node}],
        }
    ]

    def reject_scan(*args, **kwargs):
        pytest.fail("bounded startup consumer must not scan any tree")

    monkeypatch.setattr(external, "_stable_tree_inventory", reject_scan)

    def capture():
        return external.capture_reviewed_startup(
            environment_root=tmp_path,
            external_roots=[],
            active_import_roots=[],
            distributions=owners,
            tree_entries=entries,
            tree_nodes=nodes,
            tree_pool=pool,
            site_roots=["site"],
            bootstrap_paths=[],
        )

    if fault is None:
        payload = capture()
        assert payload["reviewed_startup"][0]["capability"] == "coverage-inactive.v1"
        assert payload["finder_order"] == []
        assert payload["path_declarations"][0]["roles"] == []
    else:
        with pytest.raises(PythonEnvironmentIdentityError):
            capture()


@pytest.mark.parametrize("reverse", [False, True])
def test_reviewed_finder_order_is_semantic_and_not_hardcoded(
    monkeypatch, tmp_path, standard_finders, reverse
):
    monkeypatch.chdir(tmp_path)
    monkeypatch.delenv("SETUPTOOLS_USE_DISTUTILS", raising=False)
    uv = type("_Finder", (), {"__module__": "_virtualenv"})()
    setuptools = type("DistutilsMetaFinder", (), {"__module__": "_distutils_hack"})()
    prefix = [setuptools, uv] if reverse else [uv, setuptools]
    sys.meta_path[:0] = prefix
    observed = []
    monkeypatch.setattr(
        external,
        "_verify_reviewed_module",
        lambda capability, path, source, finder: observed.append(capability),
    )
    startup = [
        {
            "capability": capability,
            "gate": external._STARTUP_CAPABILITIES[capability]["gate"],
        }
        for capability in ["uv-virtualenv.v1", "setuptools-local-distutils.v1"]
    ]
    sources = {
        row["capability"]: (tmp_path / "synthetic.py", b"synthetic") for row in startup
    }
    order = external.validate_active_import_finders(
        reviewed_startup=startup, module_sources=sources
    )
    assert order == observed
    assert order == (
        ["setuptools-local-distutils.v1", "uv-virtualenv.v1"]
        if reverse
        else ["uv-virtualenv.v1", "setuptools-local-distutils.v1"]
    )


@pytest.mark.parametrize("location", ["absolute-root", "relative-region"])
def test_editable_nfd_host_spelling_is_preserved_while_receipt_names_are_nfc(
    tmp_path, standard_finders, location
):
    native_name = "cafe\u0301"
    root = tmp_path / native_name if location == "absolute-root" else tmp_path
    fixture = _ExternalFixture(root)
    if location == "relative-region":
        native = fixture.source / native_name
        fixture.imports.rename(native)
        fixture.imports = native
        fixture.active[0]["path"] = unicodedata.normalize("NFC", native_name)
        fixture.pth.write_text(str(native) + "\n", encoding="utf-8", newline="\n")
    payload = fixture.capture()
    assert payload["path_declarations"][0]["content"] == fixture.pth.read_text(
        encoding="utf-8"
    )
    assert payload["trees"][0]["source_path"] == fixture.active[0]["path"]
    fixture.validate(payload)


def test_normalized_external_region_cannot_choose_between_distinct_native_aliases(
    tmp_path, standard_finders
):
    fixture = _ExternalFixture(tmp_path)
    nfd = fixture.source / "cafe\u0301"
    fixture.imports.rename(nfd)
    fixture.imports = nfd
    fixture.active[0]["path"] = "caf\u00e9"
    fixture.pth.write_text(str(nfd) + "\n", encoding="utf-8", newline="\n")
    try:
        (fixture.source / "caf\u00e9").mkdir()
    except FileExistsError:
        pytest.skip("filesystem does not permit distinct normalization aliases")
    with pytest.raises(PythonEnvironmentIdentityError, match="path collision"):
        fixture.capture()
