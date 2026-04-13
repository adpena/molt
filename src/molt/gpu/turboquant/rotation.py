"""Structured rotation plans for TurboQuant."""

from __future__ import annotations

import math


def is_power_of_two(value: int) -> bool:
    return value > 0 and (value & (value - 1)) == 0


def _lcg_next(state: int) -> int:
    return (1664525 * state + 1013904223) & 0xFFFFFFFF


def random_signs(dim: int, seed: int) -> list[float]:
    state = seed & 0xFFFFFFFF
    out = []
    for _ in range(dim):
        state = _lcg_next(state)
        out.append(-1.0 if state & 1 else 1.0)
    return out


def _normalized_hadamard(values) -> list[float]:
    out = [float(value) for value in values]
    size = len(out)
    span = 1
    while span < size:
        step = span * 2
        for start in range(0, size, step):
            stop = start + span
            for index in range(start, stop):
                left = out[index]
                right = out[index + span]
                out[index] = left + right
                out[index + span] = left - right
        span = step
    scale = 1.0 / math.sqrt(size)
    return [value * scale for value in out]


class HadamardRotationPlan:
    """Randomized Hadamard rotation used by the practical TurboQuant path."""

    def __init__(self, dim: int, seed: int) -> None:
        if not is_power_of_two(dim):
            raise ValueError(
                "TurboQuant hadamard rotation currently requires a power-of-two dimension"
            )
        self.dim = int(dim)
        self.seed = int(seed)
        self.signs = random_signs(dim, seed)

    def apply(self, values) -> list[float]:
        signed = [float(values[index]) * self.signs[index] for index in range(self.dim)]
        return _normalized_hadamard(signed)

    def invert(self, values) -> list[float]:
        rotated = _normalized_hadamard(values)
        return [rotated[index] * self.signs[index] for index in range(self.dim)]
