"""Purpose: linecache lazy loader source lookup parity via runtime intrinsic."""

import linecache
import types


class _Loader:
    def __init__(self) -> None:
        self.calls = 0

    def get_source(self, name: str):
        self.calls += 1
        if name == "virtual_mod":
            return "value = 42\n"
        return None


class _MissingLoader:
    def __init__(self) -> None:
        self.calls = 0

    def get_source(self, _name: str):
        self.calls += 1
        raise ImportError("missing source")


linecache.clearcache()

loader = _Loader()
globals_with_loader = {
    "__name__": "virtual_mod",
    "__spec__": types.SimpleNamespace(name="virtual_mod", loader=loader),
}
registered = linecache.lazycache("virtual_mod.py", globals_with_loader)
line = linecache.getline("virtual_mod.py", 1, globals_with_loader)

missing_loader = _MissingLoader()
globals_missing_loader = {
    "__name__": "virtual_missing",
    "__spec__": types.SimpleNamespace(name="virtual_missing", loader=missing_loader),
}
registered_missing = linecache.lazycache("virtual_missing.py", globals_missing_loader)
missing_line = linecache.getline("virtual_missing.py", 1, globals_missing_loader)

print(registered)
print(line.strip() == "value = 42")
print(loader.calls >= 1)
print(registered_missing)
print(missing_line == "")
print(missing_loader.calls >= 1)
