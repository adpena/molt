# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: validate module-anchor legacy APIs stay intrinsic-backed for readers and namespace roots."""

import importlib
import importlib.resources
import io
import os
import pathlib
import sys
import tempfile
import types
import warnings


def _open_binary(anchor, resource: str) -> bytes:
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        with importlib.resources.open_binary(anchor, resource) as handle:
            return handle.read()


def _read_text(anchor, resource: str) -> str:
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        return importlib.resources.read_text(anchor, resource, encoding="utf-8")


def _is_resource(anchor, resource: str) -> bool:
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        return importlib.resources.is_resource(anchor, resource)


def _contents(anchor) -> list[str]:
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        return sorted(importlib.resources.contents(anchor))


def _path_value(anchor, resource: str) -> str:
    with warnings.catch_warnings():
        warnings.simplefilter("ignore", DeprecationWarning)
        with importlib.resources.path(anchor, resource) as resolved:
            return os.fspath(resolved)


class _Reader:
    def __init__(self, resource_path: str) -> None:
        self._resource_path = resource_path
        self._resources = {"data.txt": b"reader-data\n"}

    def contents(self):
        return list(self._resources)

    def is_resource(self, name: str) -> bool:
        return name in self._resources

    def open_resource(self, name: str):
        if name not in self._resources:
            raise FileNotFoundError(name)
        return io.BytesIO(self._resources[name])

    def resource_path(self, name: str) -> str:
        if name == "data.txt":
            return self._resource_path
        raise FileNotFoundError(name)


class _Loader:
    def __init__(self, reader: _Reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "anchor_reader_pkg":
            return self._reader
        return None


with tempfile.TemporaryDirectory(prefix="molt_anchor_reader_") as tmp:
    root = pathlib.Path(tmp)
    data_path = root / "data.txt"
    data_path.write_text("reader-data\n", encoding="utf-8")

    module = types.ModuleType("anchor_reader_pkg")
    module.__name__ = "anchor_reader_pkg"
    module.__package__ = "anchor_reader_pkg"
    module.__file__ = "<anchor_reader_pkg>"
    module.__spec__ = types.SimpleNamespace(
        name="anchor_reader_pkg",
        loader=_Loader(_Reader(str(data_path))),
        submodule_search_locations=[],
    )

    custom_open = _open_binary(module, "data.txt")
    custom_text = _read_text(module, "data.txt").strip()
    custom_is_resource = _is_resource(module, "data.txt")
    custom_contents = _contents(module)
    custom_path_value = _path_value(module, "data.txt")

print("custom_open_binary", custom_open == b"reader-data\n")
print("custom_read_text", custom_text == "reader-data")
print("custom_is_resource", custom_is_resource)
print("custom_contents", custom_contents == ["data.txt"])
print("custom_path_value", custom_path_value.endswith("data.txt"))

with tempfile.TemporaryDirectory(prefix="molt_anchor_ns_") as tmp:
    root = pathlib.Path(tmp)
    left_root = root / "left"
    right_root = root / "right"
    left_pkg = left_root / "nsanchor_mod" / "pkg"
    right_pkg = right_root / "nsanchor_mod" / "pkg"
    left_pkg.mkdir(parents=True)
    right_pkg.mkdir(parents=True)

    (left_pkg / "left.txt").write_text("left-data\n", encoding="utf-8")
    (right_pkg / "right.txt").write_text("right-data\n", encoding="utf-8")

    original_path = list(sys.path)
    original_modules = {
        name: sys.modules.get(name) for name in ("nsanchor_mod", "nsanchor_mod.pkg")
    }
    try:
        sys.path[:] = [str(left_root), str(right_root)]
        anchor = importlib.import_module("nsanchor_mod.pkg")

        ns_open = _open_binary(anchor, "right.txt")
        ns_text = _read_text(anchor, "right.txt").strip()
        ns_is_resource = _is_resource(anchor, "right.txt")
        ns_contents = _contents(anchor)
        ns_path_value = _path_value(anchor, "right.txt")
    finally:
        sys.path[:] = original_path
        for name, previous in original_modules.items():
            if previous is None:
                sys.modules.pop(name, None)
            else:
                sys.modules[name] = previous

print("namespace_open_binary", ns_open == b"right-data\n")
print("namespace_read_text", ns_text == "right-data")
print("namespace_is_resource", ns_is_resource)
print("namespace_contents", "left.txt" in ns_contents and "right.txt" in ns_contents)
print("namespace_path_value", ns_path_value.endswith("right.txt"))
