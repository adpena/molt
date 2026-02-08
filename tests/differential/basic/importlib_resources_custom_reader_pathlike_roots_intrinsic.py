"""Purpose: validate importlib.resources custom reader path-like root parity."""

import importlib.resources
import io
import os
import tempfile
import types


root = tempfile.mkdtemp(prefix="molt_resources_pathlike_root_")
resource_path = os.path.join(root, "data.txt")
with open(resource_path, "w", encoding="utf-8") as handle:
    handle.write("pathlike-root\n")


class _PathLike:
    def __init__(self, path: str) -> None:
        self._path = path

    def __fspath__(self) -> str:
        return self._path


class _Reader:
    def molt_roots(self):
        return [_PathLike(root)]

    def contents(self):
        return ["data.txt"]

    def is_resource(self, name: str) -> bool:
        return name == "data.txt"

    def open_resource(self, name: str):
        if name != "data.txt":
            raise FileNotFoundError(name)
        return io.BytesIO(b"pathlike-root\n")


class _Loader:
    def __init__(self, reader: _Reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "readerpkg_pathlike":
            return self._reader
        return None


module = types.ModuleType("readerpkg_pathlike")
module.__name__ = "readerpkg_pathlike"
module.__package__ = "readerpkg_pathlike"
module.__file__ = "<readerpkg_pathlike>"
module.__spec__ = types.SimpleNamespace(
    name="readerpkg_pathlike",
    loader=_Loader(_Reader()),
    submodule_search_locations=[],
)

traversable = importlib.resources.files(module)
print(traversable.joinpath("data.txt").read_text().strip() == "pathlike-root")
print(importlib.resources.contents(module) == ["data.txt"])
