"""Benchmark: Procedural zone generation (3D noise sampling).

Mirrors the Vertigo site generator.py workload — iterates a 3D grid,
samples smooth noise at each cell, and accumulates platform/crystal/anchor
lists.  Measures the tight compute loop that Molt compiles to native/WASM.
"""

import math


def hash_coord(x: int, y: int, z: int, seed: int) -> float:
    h = seed ^ x * 374761393 ^ y * 668265263 ^ z * 2246822519
    h = (h ^ (h >> 13)) * 3266489917
    h = (h ^ (h >> 16))
    return (h & 0xFFFFFFFF) / 4294967295.0


def interpolate(a: float, b: float, w: float) -> float:
    return (b - a) * (3.0 - w * 2.0) * w * w + a


def smooth_noise(x: float, y: float, z: float, seed: int) -> float:
    ix = int(math.floor(x))
    iy = int(math.floor(y))
    iz = int(math.floor(z))
    fx = x - ix
    fy = y - iy
    fz = z - iz
    v000 = hash_coord(ix, iy, iz, seed)
    v100 = hash_coord(ix + 1, iy, iz, seed)
    v010 = hash_coord(ix, iy + 1, iz, seed)
    v110 = hash_coord(ix + 1, iy + 1, iz, seed)
    v001 = hash_coord(ix, iy, iz + 1, seed)
    v101 = hash_coord(ix + 1, iy, iz + 1, seed)
    v011 = hash_coord(ix, iy + 1, iz + 1, seed)
    v111 = hash_coord(ix + 1, iy + 1, iz + 1, seed)
    i1 = interpolate(v000, v100, fx)
    i2 = interpolate(v010, v110, fx)
    j1 = interpolate(i1, i2, fy)
    i3 = interpolate(v001, v101, fx)
    i4 = interpolate(v011, v111, fx)
    j2 = interpolate(i3, i4, fy)
    return interpolate(j1, j2, fz)


def main() -> None:
    seed = 1337
    base_y = 222
    depth = 64
    step = 8
    threshold = 0.55
    platform_count = 0
    crystal_count = 0
    anchor_count = 0

    for y in range(base_y, base_y + depth, step):
        for x in range(-50, 50, step):
            for z in range(-50, 50, step):
                density = smooth_noise(x * 0.1, y * 0.1, z * 0.1, seed)
                if density > threshold:
                    platform_count += 1
                    chance = hash_coord(x, y, z, seed + 1)
                    if chance > 0.95:
                        crystal_count += 1
                    if chance < 0.02:
                        anchor_count += 1

    print(platform_count)


if __name__ == "__main__":
    main()
