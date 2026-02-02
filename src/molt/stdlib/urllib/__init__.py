"""Minimal urllib package for Molt."""

from __future__ import annotations

import importlib as _importlib

parse = _importlib.import_module("urllib.parse")

__all__ = ["parse"]
