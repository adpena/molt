def main() -> None:
    n: int = 5
    x: list[float] = [0.0, 4.841, 8.343, 12.894, 15.379]
    y: list[float] = [0.0, -1.160, 4.124, -15.111, -25.919]
    z: list[float] = [0.0, -0.103, -0.403, -0.223, 0.179]
    vx: list[float] = [0.0, 0.00166, -0.00276, 0.00298, 0.00026]
    vy: list[float] = [0.0, 0.00769, 0.00499, 0.00237, 0.00168]
    vz: list[float] = [0.0, -0.0000690, 0.0000230, -0.0000296, -0.0000395]
    mass: list[float] = [39.478, 0.03770, 0.01129, 0.001724, 0.002033]

    dt: float = 0.01
    steps: int = 100000

    step: int = 0
    while step < steps:
        i: int = 0
        while i < n:
            j: int = i + 1
            while j < n:
                dx: float = x[i] - x[j]
                dy: float = y[i] - y[j]
                dz: float = z[i] - z[j]
                dist_sq: float = dx * dx + dy * dy + dz * dz
                dist: float = dist_sq**0.5
                mag: float = dt / (dist_sq * dist)

                vx[i] = vx[i] - dx * mass[j] * mag
                vy[i] = vy[i] - dy * mass[j] * mag
                vz[i] = vz[i] - dz * mass[j] * mag
                vx[j] = vx[j] + dx * mass[i] * mag
                vy[j] = vy[j] + dy * mass[i] * mag
                vz[j] = vz[j] + dz * mass[i] * mag
                j = j + 1
            i = i + 1

        i = 0
        while i < n:
            x[i] = x[i] + dt * vx[i]
            y[i] = y[i] + dt * vy[i]
            z[i] = z[i] + dt * vz[i]
            i = i + 1
        step = step + 1

    energy: float = 0.0
    i: int = 0
    while i < n:
        energy = energy + 0.5 * mass[i] * (
            vx[i] * vx[i] + vy[i] * vy[i] + vz[i] * vz[i]
        )
        j: int = i + 1
        while j < n:
            dx: float = x[i] - x[j]
            dy: float = y[i] - y[j]
            dz: float = z[i] - z[j]
            dist: float = (dx * dx + dy * dy + dz * dz) ** 0.5
            energy = energy - (mass[i] * mass[j]) / dist
            j = j + 1
        i = i + 1

    print(energy)


main()
