"""Public runtime packages have ordinary source/import custody."""

from pathlib import Path
import runpy

import pytest
import _intrinsics

from molt.cli import module_graph_discovery, module_resolution, module_stdlib_policy


@pytest.mark.parametrize("package", ["molt_json", "molt_msgpack", "molt_buffer"])
def test_active_runtime_package_never_hides_missing_intrinsics(monkeypatch, package):
    monkeypatch.setattr(_intrinsics, "runtime_active", lambda: True)
    failure = RuntimeError("missing required runtime intrinsic")

    def missing_intrinsic(name, namespace):
        raise failure

    monkeypatch.setattr(_intrinsics, "require_intrinsic", missing_intrinsic)
    source = Path(__file__).resolve().parents[2] / "src" / package / "__init__.py"
    with pytest.raises(RuntimeError) as raised:
        runpy.run_path(str(source))
    assert raised.value is failure


@pytest.mark.parametrize("package", ["molt_json", "molt_msgpack", "molt_buffer"])
def test_cpython_package_does_not_probe_runtime_intrinsics(monkeypatch, package):
    monkeypatch.setattr(_intrinsics, "runtime_active", lambda: False)

    def unexpected_probe(name, namespace):
        raise AssertionError(f"CPython attempted runtime intrinsic {name}")

    monkeypatch.setattr(_intrinsics, "require_intrinsic", unexpected_probe)
    source = Path(__file__).resolve().parents[2] / "src" / package / "__init__.py"
    runpy.run_path(str(source))


@pytest.mark.parametrize(
    ("source", "packages"),
    [
        (
            "import molt_json as json_api\n"
            "from molt_msgpack import parse as parse_msgpack\n"
            "import molt_cbor\nimport molt_buffer as buffers\n",
            {"molt_json", "molt_msgpack", "molt_cbor", "molt_buffer"},
        ),
        ("from molt_cbor import parse\n", {"molt_cbor", "molt_msgpack"}),
    ],
)
def test_runtime_package_sources_and_transitive_imports_are_discovered(
    tmp_path: Path, source: str, packages: set[str]
) -> None:
    repo = Path(__file__).resolve().parents[2]
    entry = tmp_path / "main.py"
    entry.write_text(source, encoding="utf-8")
    stdlib_root = module_resolution._stdlib_root_path()
    module_roots = [tmp_path, repo / "src"]
    result = module_graph_discovery._discover_module_graph(
        entry,
        [*module_roots, stdlib_root],
        module_roots,
        stdlib_root,
        tmp_path,
        module_stdlib_policy._stdlib_allowlist(),
    )
    for package in packages:
        assert (
            result.graph[package].resolve()
            == (repo / "src" / package / "__init__.py").resolve()
        )
