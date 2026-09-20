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


def test_active_buffer_owns_only_native_storage(monkeypatch):
    monkeypatch.setattr(_intrinsics, "runtime_active", lambda: True)
    storage = {}

    def new(rows, cols, value):
        handle = object()
        storage[handle] = (rows, cols, [value] * (rows * cols))
        return handle

    def get(handle, row, col):
        rows, cols, values = storage[handle]
        return values[(row % rows) * cols + (col % cols)]

    def set_value(handle, row, col, value):
        rows, cols, values = storage[handle]
        values[(row % rows) * cols + (col % cols)] = value

    exports = {
        "molt_buffer2d_new": new,
        "molt_buffer2d_rows": lambda handle: storage[handle][0],
        "molt_buffer2d_cols": lambda handle: storage[handle][1],
        "molt_buffer2d_get": get,
        "molt_buffer2d_set": set_value,
    }
    monkeypatch.setattr(
        _intrinsics, "require_intrinsic", lambda name, namespace: exports[name]
    )
    source = Path(__file__).resolve().parents[2] / "src/molt_buffer/__init__.py"
    namespace = runpy.run_path(str(source))
    buffer = namespace["Buffer2D"](2, 3, 7)
    assert (buffer.rows, buffer.cols, buffer.get(0, 1)) == (2, 3, 7)
    buffer.set(1, 2, 9)
    assert buffer.get(1, 2) == 9
    assert vars(buffer) == {"_native": buffer._native}


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
