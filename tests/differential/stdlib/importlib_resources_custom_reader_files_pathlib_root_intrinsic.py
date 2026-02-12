"""Purpose: validate importlib.resources custom-reader files() pathlib-root parity."""

import importlib.resources
import os
import pathlib
import tempfile
import types


root = tempfile.mkdtemp(prefix="molt_resources_files_pathlib_root_")
with open(os.path.join(root, "alpha.txt"), "w", encoding="utf-8") as handle:
    handle.write("alpha\n")
os.makedirs(os.path.join(root, "nested"), exist_ok=True)
with open(os.path.join(root, "nested", "beta.txt"), "w", encoding="utf-8") as handle:
    handle.write("beta\n")


class _Reader:
    def files(self):
        return pathlib.Path(root)


class _Loader:
    def __init__(self, reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "readerpkg_pathlib_root":
            return self._reader
        return None


module = types.ModuleType("readerpkg_pathlib_root")
module.__name__ = "readerpkg_pathlib_root"
module.__package__ = "readerpkg_pathlib_root"
module.__file__ = "<readerpkg_pathlib_root>"
module.__spec__ = types.SimpleNamespace(
    name="readerpkg_pathlib_root",
    loader=_Loader(_Reader()),
    submodule_search_locations=[],
)

traversable = importlib.resources.files(module)
entries = sorted(entry.name for entry in traversable.iterdir())
alpha_text = traversable.joinpath("alpha.txt").read_text().strip()
nested_entries = sorted(entry.name for entry in traversable.joinpath("nested").iterdir())

print(entries == ["alpha.txt", "nested"])
print(alpha_text == "alpha")
print(nested_entries == ["beta.txt"])
print(importlib.resources.is_resource(module, "alpha.txt"))
print(not importlib.resources.is_resource(module, "nested"))
