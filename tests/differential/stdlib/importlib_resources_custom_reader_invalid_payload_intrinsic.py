"""Purpose: validate custom ResourceReader invalid payload handling via intrinsics."""

import importlib.resources
import types


class _BadReader:
    def contents(self):
        return ["ok.txt", 123]

    def is_resource(self, name: str) -> bool:
        return name == "ok.txt"

    def open_resource(self, name: str):
        if name != "ok.txt":
            raise FileNotFoundError(name)
        return "not-bytes"


class _Loader:
    def __init__(self, reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "badreaderpkg":
            return self._reader
        return None


module = types.ModuleType("badreaderpkg")
module.__name__ = "badreaderpkg"
module.__package__ = "badreaderpkg"
module.__file__ = "<badreaderpkg>"
module.__spec__ = types.SimpleNamespace(
    name="badreaderpkg",
    loader=_Loader(_BadReader()),
    submodule_search_locations=[],
)

iterdir_exc = "none"
try:
    list(importlib.resources.files(module).iterdir())
except BaseException as exc:
    iterdir_exc = exc.__class__.__name__

read_exc = "none"
try:
    importlib.resources.read_binary(module, "ok.txt")
except BaseException as exc:
    read_exc = exc.__class__.__name__

print(iterdir_exc in {"RuntimeError", "TypeError"})
print(read_exc in {"RuntimeError", "TypeError"})
