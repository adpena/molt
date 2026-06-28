"""Intrinsic-backed ndimage primitives used by Molt numeric kernels.

This module is intentionally small: it is a Molt-owned compatibility facade for
the specific ``scipy.ndimage`` operations that have real compiler/runtime
authority. Unsupported operations fail closed instead of falling through to
host SciPy.
"""

from __future__ import annotations

from _intrinsics import require_intrinsic as _require_intrinsic


_DISTANCE_TRANSFORM_EDT = _require_intrinsic(
    "molt_scipy_ndimage_distance_transform_edt"
)


def distance_transform_edt(
    input,
    sampling=None,
    return_distances: bool = True,
    return_indices: bool = False,
    distances=None,
    indices=None,
):
    """Return the exact Euclidean distance to the nearest background pixel.

    Implemented surface: 2D boolean-like input, unit sampling, distances only.
    The lower-envelope algorithm and list materialization live in the runtime
    intrinsic so compiled builds keep one authority for native and WASM.
    """
    if sampling not in (None, 1, 1.0, (1, 1), (1.0, 1.0), [1, 1], [1.0, 1.0]):
        raise NotImplementedError(
            "distance_transform_edt currently supports unit sampling only"
        )
    if not return_distances or return_indices:
        raise NotImplementedError(
            "distance_transform_edt currently returns distances only"
        )
    if distances is not None or indices is not None:
        raise NotImplementedError(
            "distance_transform_edt output buffers are not supported yet"
        )
    return _DISTANCE_TRANSFORM_EDT(input)


def gaussian_filter(*_args, **_kwargs):
    raise NotImplementedError(
        "scipy.ndimage.gaussian_filter is not implemented by Molt yet"
    )


def maximum_filter(*_args, **_kwargs):
    raise NotImplementedError(
        "scipy.ndimage.maximum_filter is not implemented by Molt yet"
    )


def minimum_filter(*_args, **_kwargs):
    raise NotImplementedError(
        "scipy.ndimage.minimum_filter is not implemented by Molt yet"
    )


def label(*_args, **_kwargs):
    raise NotImplementedError("scipy.ndimage.label is not implemented by Molt yet")


__all__ = [
    "distance_transform_edt",
    "gaussian_filter",
    "label",
    "maximum_filter",
    "minimum_filter",
]
