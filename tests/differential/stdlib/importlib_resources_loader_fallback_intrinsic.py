"""Purpose: validate importlib.resources reader lookup falls back to module.__loader__."""

import importlib.resources
import io
import os
import tempfile
import types


root = tempfile.mkdtemp(prefix="molt_resources_loader_fallback_")
resource_path = os.path.join(root, "data.txt")
with open(resource_path, "w", encoding="utf-8") as handle:
    handle.write("loader-fallback\n")


class _Reader:
    def contents(self):
        return ["data.txt"]

    def is_resource(self, name: str) -> bool:
        return name == "data.txt"

    def open_resource(self, name: str):
        if name != "data.txt":
            raise FileNotFoundError(name)
        return io.BytesIO(b"loader-fallback\n")

    def resource_path(self, name: str) -> str:
        if name != "data.txt":
            raise FileNotFoundError(name)
        return resource_path


class _Loader:
    def __init__(self, reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "loaderfallbackpkg":
            return self._reader
        return None


module = types.ModuleType("loaderfallbackpkg")
module.__name__ = "loaderfallbackpkg"
module.__package__ = "loaderfallbackpkg"
module.__file__ = "<loaderfallbackpkg>"
module.__loader__ = _Loader(_Reader())
module.__spec__ = None

entries = sorted(entry.name for entry in importlib.resources.files(module).iterdir())
text = importlib.resources.read_text(module, "data.txt").strip()

print(entries == ["data.txt"])
print(text == "loader-fallback")
print(importlib.resources.is_resource(module, "data.txt"))
