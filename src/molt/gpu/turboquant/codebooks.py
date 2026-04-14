"""Deterministic scalar codebook construction for TurboQuant."""

from __future__ import annotations

import math


_CODEBOOK_CACHE = {}


def _coordinate_density(value: float, dim: int) -> float:
    value = float(value)
    dim = float(dim)
    base = 1.0 - (value * value)
    if base <= 0.0:
        base = 1e-12
    exponent = 0.5 * (dim - 3.0)
    log_norm = (
        math.lgamma(0.5 * dim)
        - 0.5 * math.log(math.pi)
        - math.lgamma(0.5 * (dim - 1.0))
    )
    return math.exp(log_norm + (exponent * math.log(base)))


def _grid(dim: int, grid_size: int) -> tuple[list[float], list[float]]:
    xs = []
    ws = []
    if grid_size < 3:
        grid_size = 3
    for index in range(grid_size):
        x = -1.0 + (2.0 * index / float(grid_size - 1))
        xs.append(x)
        ws.append(_coordinate_density(x, dim))
    return xs, ws


def _initial_centroids(xs, ws, levels: int) -> list[float]:
    total = sum(ws)
    centroids = []
    for level in range(levels):
        target = ((level + 0.5) / float(levels)) * total
        running = 0.0
        selected = xs[-1]
        for index, weight in enumerate(ws):
            running += weight
            if running >= target:
                selected = xs[index]
                break
        centroids.append(selected)
    return centroids


def build_codebook(dim: int, bits: int) -> tuple[float, ...]:
    cache_key = (int(dim), int(bits))
    cached = _CODEBOOK_CACHE.get(cache_key)
    if cached is not None:
        return cached

    if bits <= 0:
        raise ValueError("TurboQuant codebooks require at least 1 stage bit")

    levels = 1 << bits
    xs, ws = _grid(dim, 4097)
    centroids = _initial_centroids(xs, ws, levels)

    for _ in range(32):
        bounds = [-1.0]
        for index in range(levels - 1):
            bounds.append(0.5 * (centroids[index] + centroids[index + 1]))
        bounds.append(1.0)

        updated = []
        for level in range(levels):
            left = bounds[level]
            right = bounds[level + 1]
            numerator = 0.0
            denominator = 0.0
            for index, x in enumerate(xs):
                if left <= x <= right:
                    weight = ws[index]
                    numerator += x * weight
                    denominator += weight
            if denominator == 0.0:
                updated.append(0.5 * (left + right))
            else:
                updated.append(numerator / denominator)
        centroids = updated

    result = tuple(sorted(float(value) for value in centroids))
    _CODEBOOK_CACHE[cache_key] = result
    return result
