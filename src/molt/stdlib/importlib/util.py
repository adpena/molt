"""Import-only importlib.util stubs for Molt."""

from __future__ import annotations

# TODO(stdlib-compat, owner:stdlib, milestone:SL3): implement spec helpers + loader plumbing.

__all__ = [
    "find_spec",
    "module_from_spec",
    "spec_from_file_location",
    "spec_from_loader",
]


def find_spec(_name: str, _package: str | None = None):
    return None


def module_from_spec(_spec):
    raise ImportError("importlib.util.module_from_spec is not supported in Molt")


def spec_from_loader(_name: str, _loader, _origin: str | None = None, _is_package=None):
    raise ImportError("importlib.util.spec_from_loader is not supported in Molt")


def spec_from_file_location(
    _name: str,
    _location,
    _loader=None,
    _submodule_search_locations=None,
):
    raise ImportError("importlib.util.spec_from_file_location is not supported in Molt")
