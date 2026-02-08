# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: keep loader-reader resource_path filesystem-only for archive members."""

import importlib.resources
import os
import tempfile
import types
import zipfile


root = tempfile.mkdtemp(prefix="molt_reader_roots_archive_path_")
archive = os.path.join(root, "resources.zip")
with zipfile.ZipFile(archive, "w") as zf:
    zf.writestr("pkg/data.txt", "zip-root\n")


class _Reader:
    def __init__(self, package_root: str) -> None:
        self._package_root = package_root

    def molt_roots(self):
        return (self._package_root,)


class _Loader:
    def __init__(self, reader: _Reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "ziprootpkg":
            return self._reader
        return None


module = types.ModuleType("ziprootpkg")
module.__name__ = "ziprootpkg"
module.__package__ = "ziprootpkg"
module.__file__ = "<ziprootpkg>"
module.__spec__ = types.SimpleNamespace(
    name="ziprootpkg",
    loader=_Loader(_Reader(f"{archive}/pkg")),
    submodule_search_locations=[],
)

item = importlib.resources.files(module).joinpath("data.txt")
fallback_path = item.__fspath__()
with item.open("rb") as handle:
    via_traversable = handle.read()
with importlib.resources.open_binary(module, "data.txt") as handle:
    via_open_binary = handle.read()

print(fallback_path.startswith("<loader-resource:ziprootpkg/data.txt>"))
print(via_traversable == b"zip-root\n")
print(via_open_binary == b"zip-root\n")
print(importlib.resources.is_resource(module, "data.txt"))
