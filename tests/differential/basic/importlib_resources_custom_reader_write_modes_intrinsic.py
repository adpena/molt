"""Purpose: validate importlib.resources custom-reader write/read mode parity."""

import importlib.resources
import io
import os
import tempfile
import types


root = tempfile.mkdtemp(prefix="molt_resources_custom_reader_write_")
disk_path = os.path.join(root, "disk.txt")
with open(disk_path, "w", encoding="utf-8") as handle:
    handle.write("disk-old\n")


class _Reader:
    def __init__(self, path: str) -> None:
        self._path = path
        self._virtual = {"virtual.txt": b"virtual-data\n"}

    def contents(self):
        return ["disk.txt", "virtual.txt"]

    def is_resource(self, name: str) -> bool:
        return name in {"disk.txt", "virtual.txt"}

    def open_resource(self, name: str):
        if name == "disk.txt":
            with open(self._path, "rb") as handle:
                return io.BytesIO(handle.read())
        if name == "virtual.txt":
            return io.BytesIO(self._virtual[name])
        raise FileNotFoundError(name)

    def resource_path(self, name: str) -> str:
        if name == "disk.txt":
            return self._path
        raise FileNotFoundError(name)


class _Loader:
    def __init__(self, reader: _Reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "readerpkg_write":
            return self._reader
        return None


module = types.ModuleType("readerpkg_write")
module.__name__ = "readerpkg_write"
module.__package__ = "readerpkg_write"
module.__file__ = "<readerpkg_write>"
module.__spec__ = types.SimpleNamespace(
    name="readerpkg_write",
    loader=_Loader(_Reader(disk_path)),
    submodule_search_locations=[],
)

root_view = importlib.resources.files(module)
disk_entry = root_view.joinpath("disk.txt")
virtual_entry = root_view.joinpath("virtual.txt")

with disk_entry.open("w", encoding="utf-8") as handle:
    handle.write("disk-new\n")

disk_after = disk_entry.read_text().strip()
disk_binary_after = disk_entry.read_bytes()

virtual_write_error = "none"
try:
    with virtual_entry.open("w", encoding="utf-8") as handle:
        handle.write("x")
except BaseException as exc:
    virtual_write_error = exc.__class__.__name__

print(disk_after == "disk-new")
print(disk_binary_after == b"disk-new\n")
print(virtual_write_error in {"UnsupportedOperation", "FileNotFoundError"})
