"""Canonical GPU backend request parsing.

Every Molt GPU surface that reads ``MOLT_GPU_BACKEND`` must use this module so
backend spelling, whitespace, and capability checks cannot drift by subsystem.
"""

from __future__ import annotations

import os
from collections.abc import Mapping


MOLT_GPU_BACKEND_ENV = "MOLT_GPU_BACKEND"
GPU_DEVICE_BACKENDS = frozenset(
    {
        "metal",
        "webgpu",
    }
)


def normalize_gpu_backend(backend: str | None) -> str | None:
    if backend is None:
        return None
    if not isinstance(backend, str):
        raise TypeError("gpu backend must be a string when set")
    normalized = backend.strip().lower()
    return normalized or None


def requested_gpu_backend(
    environ: Mapping[str, str] | None = None,
) -> str | None:
    source = os.environ if environ is None else environ
    return normalize_gpu_backend(source.get(MOLT_GPU_BACKEND_ENV))


def has_gpu_backend_request(backend: str | None = None) -> bool:
    return normalize_gpu_backend(backend) is not None


def is_gpu_device_backend(backend: str | None) -> bool:
    return normalize_gpu_backend(backend) in GPU_DEVICE_BACKENDS
