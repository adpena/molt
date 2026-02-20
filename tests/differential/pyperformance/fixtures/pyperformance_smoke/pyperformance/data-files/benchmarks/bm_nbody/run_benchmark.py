"""nbody benchmark kernel (derived from pyperformance, MIT)."""

import pyperf


DEFAULT_REFERENCE = "sun"
PI = 3.14159265358979323
SOLAR_MASS = 4 * PI * PI
DAYS_PER_YEAR = 365.24

BODIES = {
    "sun": ([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], SOLAR_MASS),
    "jupiter": (
        [4.8414314424647209, -1.1603200440274283, -0.1036220444711231],
        [
            0.001660076642744037 * DAYS_PER_YEAR,
            0.007699011184197404 * DAYS_PER_YEAR,
            -0.0000690460016972063 * DAYS_PER_YEAR,
        ],
        0.0009547919384243266 * SOLAR_MASS,
    ),
    "saturn": (
        [8.34336671824458, 4.124798564124305, -0.4035234171143214],
        [
            -0.002767425107268624 * DAYS_PER_YEAR,
            0.004998528012349172 * DAYS_PER_YEAR,
            0.0000230417297573764 * DAYS_PER_YEAR,
        ],
        0.0002858859806661308 * SOLAR_MASS,
    ),
    "uranus": (
        [12.89436956213913, -15.11115140169863, -0.2233075788926557],
        [
            0.002964601375647616 * DAYS_PER_YEAR,
            0.002378471739594809 * DAYS_PER_YEAR,
            -0.0000296589568540238 * DAYS_PER_YEAR,
        ],
        0.00004366244043351563 * SOLAR_MASS,
    ),
    "neptune": (
        [15.379697114850916, -25.919314609987964, 0.1792587729503712],
        [
            0.002680677724903893 * DAYS_PER_YEAR,
            0.001628241700382423 * DAYS_PER_YEAR,
            -0.0000951592254519716 * DAYS_PER_YEAR,
        ],
        0.00005151389020466115 * SOLAR_MASS,
    ),
}

SYSTEM = list(BODIES.values())
PAIRS = [
    (SYSTEM[i], SYSTEM[j])
    for i in range(len(SYSTEM) - 1)
    for j in range(i + 1, len(SYSTEM))
]


def advance(dt: float, iterations: int, bodies=SYSTEM, pairs=PAIRS) -> None:
    for _ in range(iterations):
        for ([x1, y1, z1], v1, m1), ([x2, y2, z2], v2, m2) in pairs:
            dx = x1 - x2
            dy = y1 - y2
            dz = z1 - z2
            mag = dt * ((dx * dx + dy * dy + dz * dz) ** (-1.5))
            b1m = m1 * mag
            b2m = m2 * mag
            v1[0] -= dx * b2m
            v1[1] -= dy * b2m
            v1[2] -= dz * b2m
            v2[0] += dx * b1m
            v2[1] += dy * b1m
            v2[2] += dz * b1m
        for r, [vx, vy, vz], _mass in bodies:
            r[0] += dt * vx
            r[1] += dt * vy
            r[2] += dt * vz


def report_energy(bodies=SYSTEM, pairs=PAIRS, energy: float = 0.0) -> float:
    for ((x1, y1, z1), _v1, m1), ((x2, y2, z2), _v2, m2) in pairs:
        dx = x1 - x2
        dy = y1 - y2
        dz = z1 - z2
        energy -= (m1 * m2) / ((dx * dx + dy * dy + dz * dz) ** 0.5)
    for _r, [vx, vy, vz], mass in bodies:
        energy += mass * (vx * vx + vy * vy + vz * vz) / 2.0
    return energy


def offset_momentum(
    ref, bodies=SYSTEM, px: float = 0.0, py: float = 0.0, pz: float = 0.0
) -> None:
    for _r, [vx, vy, vz], mass in bodies:
        px -= vx * mass
        py -= vy * mass
        pz -= vz * mass
    (_r, v, mass) = ref
    v[0] = px / mass
    v[1] = py / mass
    v[2] = pz / mass


def bench_nbody(loops: int, reference: str, iterations: int) -> float:
    offset_momentum(BODIES[reference])
    start = pyperf.perf_counter()
    for _ in range(loops):
        report_energy()
        advance(0.01, iterations)
        report_energy()
    return pyperf.perf_counter() - start
