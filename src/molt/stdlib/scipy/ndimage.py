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
_GAUSSIAN_FILTER = _require_intrinsic("molt_scipy_ndimage_gaussian_filter")
_MAXIMUM_FILTER = _require_intrinsic("molt_scipy_ndimage_maximum_filter")
_MINIMUM_FILTER = _require_intrinsic("molt_scipy_ndimage_minimum_filter")
_LABEL = _require_intrinsic("molt_scipy_ndimage_label")


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


def _require_reflect_mode(name: str, mode) -> None:
    if mode != "reflect":
        raise NotImplementedError(f"{name} currently supports mode='reflect' only")


def _require_zero_origin(name: str, origin) -> None:
    if origin not in (0, (0, 0), [0, 0]):
        raise NotImplementedError(f"{name} currently supports origin=0 only")


def _require_odd_square_size(name: str, size) -> int:
    if isinstance(size, (tuple, list)):
        if len(size) != 2 or size[0] != size[1]:
            raise NotImplementedError(f"{name} currently supports square filters only")
        size = size[0]
    if not isinstance(size, int) or size <= 0 or size % 2 == 0:
        raise NotImplementedError(f"{name} currently supports positive odd size only")
    return size


def gaussian_filter(
    input,
    sigma,
    order=0,
    output=None,
    mode="reflect",
    cval=0.0,
    truncate=4.0,
    *,
    radius=None,
    axes=None,
):
    """Return a 2D Gaussian filter using SciPy's default reflect boundary.

    Implemented surface: scalar sigma, order 0, mode="reflect", truncate=4.0,
    no output buffer, no explicit radius, and all axes.
    """
    if isinstance(sigma, (tuple, list)):
        if len(sigma) != 2 or sigma[0] != sigma[1]:
            raise NotImplementedError(
                "gaussian_filter currently supports scalar sigma only"
            )
        sigma = sigma[0]
    if order not in (0, (0, 0), [0, 0]):
        raise NotImplementedError("gaussian_filter currently supports order=0 only")
    if output is not None:
        raise NotImplementedError("gaussian_filter output buffers are not supported yet")
    _require_reflect_mode("gaussian_filter", mode)
    if cval not in (0, 0.0):
        raise NotImplementedError("gaussian_filter currently supports cval=0 only")
    if truncate != 4.0:
        raise NotImplementedError("gaussian_filter currently supports truncate=4.0 only")
    if radius is not None:
        raise NotImplementedError("gaussian_filter radius is not supported yet")
    if axes is not None:
        raise NotImplementedError("gaussian_filter axes is not supported yet")
    return _GAUSSIAN_FILTER(input, sigma)


def maximum_filter(
    input,
    size=None,
    footprint=None,
    output=None,
    mode="reflect",
    cval=0.0,
    origin=0,
    *,
    axes=None,
):
    """Return a 2D odd square maximum filter using reflect boundaries."""
    if size is None:
        raise TypeError("maximum_filter() missing required argument 'size'")
    if footprint is not None:
        raise NotImplementedError("maximum_filter footprint is not supported yet")
    if output is not None:
        raise NotImplementedError("maximum_filter output buffers are not supported yet")
    _require_reflect_mode("maximum_filter", mode)
    if cval not in (0, 0.0):
        raise NotImplementedError("maximum_filter currently supports cval=0 only")
    _require_zero_origin("maximum_filter", origin)
    if axes is not None:
        raise NotImplementedError("maximum_filter axes is not supported yet")
    return _MAXIMUM_FILTER(input, _require_odd_square_size("maximum_filter", size))


def minimum_filter(
    input,
    size=None,
    footprint=None,
    output=None,
    mode="reflect",
    cval=0.0,
    origin=0,
    *,
    axes=None,
):
    """Return a 2D odd square minimum filter using reflect boundaries."""
    if size is None:
        raise TypeError("minimum_filter() missing required argument 'size'")
    if footprint is not None:
        raise NotImplementedError("minimum_filter footprint is not supported yet")
    if output is not None:
        raise NotImplementedError("minimum_filter output buffers are not supported yet")
    _require_reflect_mode("minimum_filter", mode)
    if cval not in (0, 0.0):
        raise NotImplementedError("minimum_filter currently supports cval=0 only")
    _require_zero_origin("minimum_filter", origin)
    if axes is not None:
        raise NotImplementedError("minimum_filter axes is not supported yet")
    return _MINIMUM_FILTER(input, _require_odd_square_size("minimum_filter", size))


def label(input, structure=None, output=None):
    """Label non-zero 2D regions with SciPy's default 4-connectivity."""
    if structure is not None:
        raise NotImplementedError("label structure is not supported yet")
    if output is not None:
        raise NotImplementedError("label output buffers are not supported yet")
    return _LABEL(input)


__all__ = [
    "distance_transform_edt",
    "gaussian_filter",
    "label",
    "maximum_filter",
    "minimum_filter",
]
