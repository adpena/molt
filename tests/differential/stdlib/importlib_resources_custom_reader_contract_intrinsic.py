"""Purpose: validate importlib.resources custom ResourceReader contract parity."""

import importlib.resources
import io
import os
import tempfile
import types


root = tempfile.mkdtemp(prefix="molt_resources_custom_reader_")
resource_path = os.path.join(root, "data.txt")
with open(resource_path, "w", encoding="utf-8") as handle:
    handle.write("reader-main\n")


class _Reader:
    def __init__(self, path: str) -> None:
        self._path = path
        self._resources = {
            "data.txt": b"reader-main\n",
            "inner.txt": b"reader-inner\n",
            "sub/leaf.txt": b"reader-sub\n",
        }

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
            return self._path
        raise FileNotFoundError(name)


class _Loader:
    def __init__(self, reader: _Reader) -> None:
        self._reader = reader

    def get_resource_reader(self, fullname: str):
        if fullname == "readerpkg":
            return self._reader
        return None


module = types.ModuleType("readerpkg")
module.__name__ = "readerpkg"
module.__package__ = "readerpkg"
module.__file__ = "<readerpkg>"
module.__spec__ = types.SimpleNamespace(
    name="readerpkg",
    loader=_Loader(_Reader(resource_path)),
    submodule_search_locations=[],
)

traversable = importlib.resources.files(module)
entry_names = sorted(entry.name for entry in traversable.iterdir())
main_text = traversable.joinpath("data.txt").read_text().strip()
inner_text = traversable.joinpath("inner.txt").read_text().strip()
sub_names = sorted(entry.name for entry in traversable.joinpath("sub").iterdir())
sub_text = traversable.joinpath("sub").joinpath("leaf.txt").read_text().strip()
main_fspath = traversable.joinpath("data.txt").__fspath__()

print("data.txt" in entry_names)
print("inner.txt" in entry_names)
print(main_text == "reader-main")
print(inner_text == "reader-inner")
print(sub_names == ["leaf.txt"])
print(sub_text == "reader-sub")
print(main_fspath.endswith("data.txt"))
print(importlib.resources.is_resource(module, "data.txt"))
print(not importlib.resources.is_resource(module, "missing.txt"))
