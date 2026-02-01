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


__all__ = ["ModuleSpec", "MOLT_LOADER", "MoltLoader"]
