"""Purpose: validate importlib.resources custom reader files()-only parity."""

import importlib.resources
import io
import types


class _Node:
    def __init__(self, name: str, *, data: bytes | None = None, children=None) -> None:
        self.name = name
        self._data = data
        self._children = dict(children or {})

    def joinpath(self, part: str):
        return self._children.get(part, _Node(part))

    def iterdir(self):
        return list(self._children.values())

    def is_file(self) -> bool:
        return self._data is not None

    def open(self, mode: str = "r", encoding: str | None = "utf-8", errors: str | None = None):
        if self._data is None:
            raise IsADirectoryError(self.name)
        if "b" in mode:
            return io.BytesIO(self._data)
        text = self._data.decode(encoding or "utf-8", errors=errors or "strict")
        return io.StringIO(text)


root = _Node(
    "root",
    children={
        "data.txt": _Node("data.txt", data=b"alpha\n"),
        "nested": _Node(
            "nested",
            children={"leaf.bin": _Node("leaf.bin", data=b"\x00\x01\x02")},
        ),
    },
)


class _Reader:
    def files(self):
        return root


class _Loader:
    def __init__(self, reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "nodepkg":
            return self._reader
        return None


module = types.ModuleType("nodepkg")
module.__name__ = "nodepkg"
module.__package__ = "nodepkg"
module.__file__ = "<nodepkg>"
module.__spec__ = types.SimpleNamespace(
    name="nodepkg",
    loader=_Loader(_Reader()),
    submodule_search_locations=[],
)

traversable = importlib.resources.files(module)
entries = sorted(entry.name for entry in traversable.iterdir())
nested_entries = sorted(entry.name for entry in traversable.joinpath("nested").iterdir())
text = importlib.resources.read_text(module, "data.txt").strip()
raw = importlib.resources.read_binary(module, "data.txt")
leaf = traversable.joinpath("nested").joinpath("leaf.bin").read_bytes()

print(entries == ["data.txt", "nested"])
print(nested_entries == ["leaf.bin"])
print(text == "alpha")
print(raw == b"alpha\n")
print(leaf == b"\x00\x01\x02")
print(importlib.resources.is_resource(module, "data.txt"))
print(not importlib.resources.is_resource(module, "nested"))
