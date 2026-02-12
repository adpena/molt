"""Purpose: validate loader-reader/module-name resolution lowered through intrinsics."""

import importlib.resources
import io
import types


class _Reader:
    def __init__(self) -> None:
        self._resources = {
            "alpha.txt": b"alpha\n",
        }

    def contents(self):
        return list(self._resources)

    def is_resource(self, name: str) -> bool:
        return name in self._resources

    def open_resource(self, name: str):
        if name not in self._resources:
            raise FileNotFoundError(name)
        return io.BytesIO(self._resources[name])


class _Loader:
    def __init__(self, reader: _Reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "specpkg":
            return self._reader
        return None


module = types.SimpleNamespace()
module.__package__ = "specpkg"
module.__spec__ = types.SimpleNamespace(
    name="specpkg",
    loader=_Loader(_Reader()),
    submodule_search_locations=[],
)

traversable = importlib.resources.files(module)
entries = sorted(entry.name for entry in traversable.iterdir())
text = traversable.joinpath("alpha.txt").read_text().strip()

print(entries == ["alpha.txt"])
print(text == "alpha")
print(importlib.resources.is_resource(module, "alpha.txt"))
print(not importlib.resources.is_resource(module, "missing.txt"))
