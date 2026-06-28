"""Molt-owned SciPy compatibility facade.

Only explicitly implemented submodules are importable. The package exists so
numeric kernels can compile against a small, tree-shaken surface instead of
pulling the full SciPy package graph into browser/WASM builds.
"""

from . import ndimage

__all__ = ["ndimage"]
