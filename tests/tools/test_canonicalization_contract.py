from __future__ import annotations

import importlib.util
import json
from pathlib import Path
import sys

import pytest

ROOT = Path(__file__).resolve().parents[2]
TOOL = ROOT / "tools" / "canonicalization_contract.py"


def _load_contract_module():
    spec = importlib.util.spec_from_file_location("canonicalization_contract", TOOL)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_runtime_support_satellite_is_not_stdlib_duplicate_domain() -> None:
    contract = _load_contract_module()

    assert contract.layer_of("molt-stdlib-text") == "stdlib"
    assert contract._stdlib_domain("molt-stdlib-text") == "text"
    assert contract.layer_of("molt-runtime-stringprep") == "stdlib"
    assert contract._stdlib_domain("molt-runtime-stringprep") == "stringprep"
    assert contract.layer_of("molt-runtime-platform") == "core"
    assert contract._stdlib_domain("molt-runtime-platform") is None


def _write_graph(root: Path, entries: list[dict]) -> None:
    graph = root / "runtime" / "crate_graph.toml"
    graph.parent.mkdir(parents=True, exist_ok=True)
    graph.write_text(
        "schema_version = 1\n"
        + "\n".join(
            "[[crate]]\n"
            + "\n".join(f"{key} = {json.dumps(value)}" for key, value in entry.items())
            for entry in entries
        ),
        encoding="utf-8",
    )


def _write_crate(root: Path, directory: str, package: str, source: str = "") -> Path:
    path = root / "runtime" / directory
    path.mkdir(parents=True, exist_ok=True)
    (path / "Cargo.toml").write_text(
        f'[package]\nname = "{package}"\n' + source, encoding="utf-8"
    )
    return path


def _semantic_fixture(root: Path) -> list[dict]:
    (root / "Cargo.toml").write_text(
        '[workspace]\nmembers=["runtime/molt-stdlib-spoof", "runtime/renamed-codec", "runtime/engine"]\n',
        encoding="utf-8",
    )
    _write_crate(root, "molt-stdlib-spoof", "foundation-package")
    _write_crate(
        root,
        "renamed-codec",
        "codec-package",
        '[dependencies]\nrenamed = { package="foundation-package", path="../molt-stdlib-spoof" }\n',
    )
    _write_crate(root, "engine", "runtime-package")
    entries = [
        {
            "name": "foundation-package",
            "layer": 0,
            "runtime_layer": "core",
            "role": "stdlib satellite",
        },
        {
            "name": "codec-package",
            "layer": 1,
            "runtime_layer": "stdlib",
            "stdlib_domain": "serial",
            "role": "runtime policy satellite",
        },
        {
            "name": "runtime-package",
            "layer": 2,
            "runtime_layer": "runtime",
            "role": "arbitrary prose",
        },
    ]
    _write_graph(root, entries)
    return entries


def test_semantics_use_package_identity_not_directory_prefix_or_role(
    tmp_path: Path,
) -> None:
    contract = _load_contract_module()
    entries = _semantic_fixture(tmp_path)
    semantics = contract.load_runtime_semantics(tmp_path)
    assert contract.layer_of("molt-stdlib-spoof", semantics) == "core"
    assert contract._stdlib_domain("molt-stdlib-spoof", semantics) is None
    assert contract.layer_of("renamed-codec", semantics) == "stdlib"
    assert contract._stdlib_domain("renamed-codec", semantics) == "serial"
    assert (
        contract.check_dependency_direction(
            contract.discover_crates(tmp_path), semantics, root=tmp_path
        )
        == []
    )

    # Prose and package spelling may change without changing the typed semantic
    # assignment. The graph-to-directory projection comes from Cargo manifests.
    entries[0]["role"] = "platform and OS ABI primitive"
    entries[1]["role"] = "a description with no classification keywords"
    entries[1]["name"] = "new-package-spelling"
    _write_crate(tmp_path, "renamed-codec", "new-package-spelling")
    _write_graph(tmp_path, entries)
    assert contract.load_runtime_semantics(tmp_path) == semantics


@pytest.mark.parametrize(
    "dependency_table",
    [
        "dependencies",
        "build-dependencies",
        "dev-dependencies",
        "target.'cfg(windows)'.build-dependencies",
    ],
)
def test_package_directory_aliases_preserve_dependency_and_domain_checks(
    tmp_path: Path, dependency_table: str
) -> None:
    contract = _load_contract_module()
    _semantic_fixture(tmp_path)
    _write_crate(
        tmp_path,
        "renamed-codec",
        "codec-package",
        f'[{dependency_table}]\narbitrary_alias = {{ package="runtime-package", path="./../engine" }}\n',
    )
    builtins = tmp_path / "runtime" / "molt-runtime" / "src" / "builtins"
    builtins.mkdir(parents=True)
    (builtins / "serial.rs").write_text("// implementation\n" * 401, encoding="utf-8")
    crates = contract.discover_crates(tmp_path)
    semantics = contract.load_runtime_semantics(tmp_path, crates)
    [edge] = contract.check_dependency_direction(crates, semantics, root=tmp_path)
    assert edge.crate == "renamed-codec"
    assert edge.severity == "high"
    assert "engine" in edge.detail
    [duplicate] = contract.check_duplicate_authority(tmp_path, crates, semantics)
    assert duplicate.crate == "renamed-codec"
    assert "serial" in duplicate.detail


def test_dependency_audit_consumes_shared_local_facts_not_its_own_manifest_scanner(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cargo_workspace import LocalCargoDependency, LocalCargoManifestFacts

    contract = _load_contract_module()
    _semantic_fixture(tmp_path)
    crates = contract.discover_crates(tmp_path)
    semantics = contract.load_runtime_semantics(tmp_path, crates)
    source = crates["renamed-codec"] / "Cargo.toml"
    dependency = crates["engine"] / "Cargo.toml"
    calls: list[Path] = []

    def shared_facts(root: Path) -> LocalCargoManifestFacts:
        calls.append(root)
        return LocalCargoManifestFacts(
            (source, dependency),
            (
                LocalCargoDependency(
                    source, dependency, "renamed", "build", "cfg(windows)"
                ),
            ),
        )

    monkeypatch.setattr(contract, "workspace_manifest_facts", shared_facts)
    [edge] = contract.check_dependency_direction(crates, semantics, root=tmp_path)
    assert calls == [tmp_path]
    assert edge.crate == "renamed-codec" and "engine" in edge.detail


def test_inherited_local_edge_uses_root_workspace_path_once(tmp_path: Path) -> None:
    contract = _load_contract_module()
    _semantic_fixture(tmp_path)
    root_manifest = tmp_path / "Cargo.toml"
    root_manifest.write_text(
        root_manifest.read_text(encoding="utf-8")
        + '[workspace.dependencies]\nrenamed={package="runtime-package",path="runtime/engine"}\n',
        encoding="utf-8",
    )
    _write_crate(
        tmp_path,
        "renamed-codec",
        "codec-package",
        "[dependencies]\nrenamed.workspace=true\n[build-dependencies]\nrenamed.workspace=true\n",
    )
    crates = contract.discover_crates(tmp_path)
    [edge] = contract.check_dependency_direction(crates, root=tmp_path)
    assert edge.crate == "renamed-codec" and "engine" in edge.detail


@pytest.mark.parametrize(
    "change,diagnostic",
    [
        ({"runtime_layer": None}, "invalid runtime_layer"),
        ({"runtime_layer": 3}, "invalid runtime_layer"),
        ({"runtime_layer": "stdlib-ish"}, "invalid runtime_layer"),
        ({"runtime_layer": "stdlib"}, "stdlib requires a valid stdlib_domain"),
        (
            {"runtime_layer": "stdlib", "stdlib_domain": "../serial"},
            "valid stdlib_domain",
        ),
        ({"stdlib_domain": "serial"}, "stdlib_domain requires"),
    ],
)
def test_bad_typed_semantics_are_errors_not_unclassified_crates(
    tmp_path: Path, change: dict, diagnostic: str
) -> None:
    contract = _load_contract_module()
    entries = _semantic_fixture(tmp_path)
    for key, value in change.items():
        if value is None:
            entries[0].pop(key, None)
        else:
            entries[0][key] = value
    _write_graph(tmp_path, entries)
    with pytest.raises(ValueError, match=diagnostic):
        contract.load_runtime_semantics(tmp_path)


def test_unregistered_workspace_package_and_stale_graph_identity_fail_clearly(
    tmp_path: Path,
) -> None:
    contract = _load_contract_module()
    entries = _semantic_fixture(tmp_path)
    _write_graph(tmp_path, entries[:-1])
    with pytest.raises(ValueError, match="missing runtime semantics.*runtime-package"):
        contract.load_runtime_semantics(tmp_path)
    _write_graph(
        tmp_path,
        entries + [{"name": "missing-package", "layer": 0, "runtime_layer": "outside"}],
    )
    with pytest.raises(ValueError, match="no manifest authority.*missing-package"):
        contract.load_runtime_semantics(tmp_path)
    _write_graph(tmp_path, entries + [entries[0]])
    with pytest.raises(ValueError, match="duplicate crate package"):
        contract.load_runtime_semantics(tmp_path)


def test_numeric_build_graph_layer_and_human_role_remain_independent(
    tmp_path: Path,
) -> None:
    from tools.build_graph_audit import load_declared_graph

    contract = _load_contract_module()
    entries = _semantic_fixture(tmp_path)
    graph = load_declared_graph(tmp_path)
    assert graph.layer_of("codec-package") == 1
    assert graph.crates["foundation-package"].role == "stdlib satellite"
    assert contract.layer_of("molt-stdlib-spoof", root=tmp_path) == "core"
    entries[0]["layer"] = 7
    _write_graph(tmp_path, entries)
    assert load_declared_graph(tmp_path).layer_of("foundation-package") == 7
    assert contract.layer_of("molt-stdlib-spoof", root=tmp_path) == "core"


def test_real_package_aliases_and_all_workspace_crates_have_typed_metadata() -> None:
    contract = _load_contract_module()
    semantics = contract.load_runtime_semantics(ROOT)
    assert len(semantics) == len(contract.workspace_members(ROOT))
    assert contract.layer_of("molt-obj-model", semantics) == "core"
    assert contract.layer_of("molt-cpython-abi", semantics) == "third_party"
    assert semantics["molt-cext-discovery"].layer == "outside"
    assert semantics["molt-runtime-platform"].layer == "core"


@pytest.mark.parametrize("check", [False, True])
def test_json_preserves_check_failure_but_informational_output_stays_successful(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys, check: bool
) -> None:
    contract = _load_contract_module()
    baseline = tmp_path / contract.BASELINE_REL
    baseline.parent.mkdir(parents=True)
    baseline.write_text("{}\n", encoding="utf-8")
    monkeypatch.setattr(
        contract.release_receipt, "prepare_receipt_destination", lambda **_kwargs: None
    )
    monkeypatch.setattr(
        contract,
        "run_all",
        lambda _root: [
            contract.Violation(
                "layer_dependency", "high", "codec", "forbidden dependency", 1
            )
        ],
    )
    args = ["--root", str(tmp_path), "--json"] + (["--check"] if check else [])
    assert contract.main(args) == (1 if check else 0)
    output = json.loads(capsys.readouterr().out)
    assert output["metrics"]["layer_dependency_violations"] == 1
