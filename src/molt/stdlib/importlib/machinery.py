"""Minimal importlib.machinery support for Molt."""

from __future__ import annotations


class MoltLoader:
    def __repr__(self) -> str:
        return "<MoltLoader>"


MOLT_LOADER = MoltLoader()


class ModuleSpec:
    def __init__(
        self,
        name: str,
        loader: object | None = None,
        origin: str | None = None,
        is_package: bool | None = None,
    ) -> None:
        self.name = str(name)
        self.loader = loader
        self.origin = origin
        self.loader_state = None
        self.cached = None
        if is_package:
            self.submodule_search_locations = []
        else:
            self.submodule_search_locations = None
        self.has_location = origin is not None

    @property
    def parent(self) -> str:
        if self.submodule_search_locations is None:
            return self.name.rpartition(".")[0]
        return self.name

    def __repr__(self) -> str:
        return (
            "ModuleSpec("
            f"name={self.name!r}, "
            f"loader={self.loader!r}, "
            f"origin={self.origin!r})"
        )


class SourceFileLoader:
    def __init__(self, fullname: str, path: str) -> None:
        self.name = fullname
        self.path = str(path)

    def __repr__(self) -> str:
        return f"<MoltSourceFileLoader name={self.name!r} path={self.path!r}>"

    def get_filename(self, _fullname: str | None = None) -> str:
        return self.path

    def get_data(self, path: str) -> bytes:
        from molt import capabilities

        if not capabilities.trusted():
            capabilities.require("fs.read")
        with open(path, "rb") as handle:
            return handle.read()

    def create_module(self, _spec: ModuleSpec):
        return None

    def exec_module(self, module) -> None:
        from molt import capabilities
        import os
        import sys

        if not capabilities.trusted():
            capabilities.require("fs.read")
        path = self.path
        data = self.get_data(path)
        try:
            source = data.decode("utf-8", errors="surrogateescape")
        except Exception:
            source = data.decode("utf-8", errors="replace")
        spec = getattr(module, "__spec__", None)
        is_package = False
        if (
            spec is not None
            and getattr(spec, "submodule_search_locations", None) is not None
        ):
            is_package = True
        elif os.path.basename(path) == "__init__.py":
            is_package = True
        if spec is None:
            spec = ModuleSpec(
                module.__name__,
                loader=self,
                origin=path,
                is_package=is_package,
            )
            module.__spec__ = spec
        if getattr(module, "__loader__", None) is None:
            module.__loader__ = self
        module.__file__ = path
        module.__cached__ = None
        if is_package:
            pkg_root = os.path.dirname(path)
            module.__package__ = module.__name__
            module.__path__ = [pkg_root]
            if spec.submodule_search_locations is None:
                spec.submodule_search_locations = [pkg_root]
        else:
            module.__package__ = module.__name__.rpartition(".")[0]
        sys.modules[module.__name__] = module
        _exec_restricted(module, source, path)


def _parse_literal(text: str):
    def _is_digits(value: str) -> bool:
        if not value:
            return False
        for ch in value:
            if ch < "0" or ch > "9":
                return False
        return True

    if text in {"None", "True", "False"}:
        return None if text == "None" else text == "True"
    if text.startswith(("+", "-")) and _is_digits(text[1:]):
        return int(text)
    if _is_digits(text):
        return int(text)
    if any(ch in text for ch in (".", "e", "E")):
        try:
            return float(text)
        except Exception:
            pass
    if len(text) >= 2 and text[0] == text[-1] and text[0] in {"'", '"'}:
        inner = text[1:-1]
        inner = inner.replace("\\\\", "\\")
        inner = inner.replace("\\n", "\n")
        inner = inner.replace("\\t", "\t")
        inner = inner.replace("\\r", "\r")
        if text[0] == "'":
            inner = inner.replace("\\'", "'")
        if text[0] == '"':
            inner = inner.replace('\\"', '"')
        return inner
    return _MISSING


def _exec_restricted(module, source: str, filename: str) -> None:
    # TODO(semantics, owner:runtime, milestone:SL3, priority:P1, status:partial): replace restricted module exec with full code-object execution once eval/exec are supported.
    namespace = module.__dict__
    lines = source.splitlines()
    idx = 0
    saw_stmt = False
    while idx < len(lines):
        raw = lines[idx]
        idx += 1
        stripped = raw.strip()
        if not stripped or stripped.startswith("#"):
            continue
        if not saw_stmt and (stripped.startswith('"""') or stripped.startswith("'''")):
            quote = stripped[:3]
            if stripped.endswith(quote) and len(stripped) > 6:
                namespace["__doc__"] = stripped[3:-3]
                saw_stmt = True
                continue
            doc_lines = [stripped[3:]]
            while idx < len(lines):
                chunk = lines[idx]
                idx += 1
                end = chunk.find(quote)
                if end != -1:
                    doc_lines.append(chunk[:end])
                    break
                doc_lines.append(chunk)
            namespace["__doc__"] = "\n".join(doc_lines)
            saw_stmt = True
            continue
        saw_stmt = True
        if stripped == "pass":
            continue
        if "=" not in stripped or "==" in stripped or "!=" in stripped:
            raise NotImplementedError(f"unsupported module statement in {filename}")
        left, right = stripped.split("=", 1)
        target = left.strip()
        if not target.isidentifier():
            raise NotImplementedError(f"unsupported assignment target in {filename}")
        value = _parse_literal(right.strip())
        if value is _MISSING:
            raise NotImplementedError(f"unsupported assignment in {filename}")
        namespace[target] = value


_MISSING = object()


__all__ = ["ModuleSpec", "MOLT_LOADER", "MoltLoader", "SourceFileLoader"]
