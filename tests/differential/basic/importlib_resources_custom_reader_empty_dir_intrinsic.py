"""Purpose: validate importlib.resources reader files()-traversable empty-dir parity."""

import importlib.resources
import io
import types


class _Node:
    def __init__(
        self,
        name: str,
        *,
        is_dir: bool,
        data: bytes = b"",
        children: dict[str, "_Node"] | None = None,
    ) -> None:
        self.name = name
        self.is_dir = is_dir
        self.data = data
        self.children = children or {}


class _Traversable:
    def __init__(self, node: _Node) -> None:
        self._node = node

    @property
    def name(self) -> str:
        return self._node.name

    def joinpath(self, *parts: str):
        node = self._node
        for part in parts:
            node = node.children[part]
        return _Traversable(node)

    def iterdir(self):
        if not self._node.is_dir:
            return iter(())
        return (_Traversable(child) for child in self._node.children.values())

    def exists(self) -> bool:
        return True

    def is_dir(self) -> bool:
        return self._node.is_dir

    def is_file(self) -> bool:
        return not self._node.is_dir

    def open(self, mode: str = "rb", encoding: str = "utf-8", errors: str = "strict"):
        if self._node.is_dir:
            raise IsADirectoryError(self._node.name)
        if "b" in mode:
            return io.BytesIO(self._node.data)
        return io.StringIO(self._node.data.decode(encoding, errors=errors))


class _Reader:
    def __init__(self, root: _Node) -> None:
        self._root = root

    def files(self):
        return _Traversable(self._root)


class _Loader:
    def __init__(self, reader: _Reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "readerpkg_empty":
            return self._reader
        return None


root = _Node(
    "readerpkg_empty",
    is_dir=True,
    children={
        "empty": _Node("empty", is_dir=True, children={}),
        "data.txt": _Node("data.txt", is_dir=False, data=b"reader-empty\n"),
    },
)

module = types.ModuleType("readerpkg_empty")
module.__name__ = "readerpkg_empty"
module.__package__ = "readerpkg_empty"
module.__file__ = "<readerpkg_empty>"
module.__spec__ = types.SimpleNamespace(
    name="readerpkg_empty",
    loader=_Loader(_Reader(root)),
    submodule_search_locations=[],
)

traversable = importlib.resources.files(module)
empty = traversable.joinpath("empty")
data = traversable.joinpath("data.txt")

print(empty.exists())
print(empty.is_dir())
print(not empty.is_file())
print(list(empty.iterdir()) == [])
print(data.read_text().strip() == "reader-empty")
