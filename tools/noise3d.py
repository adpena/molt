"""
noise3d.py — Self-contained 3D simplex noise for Molt -> Luau compilation.

Ken Perlin simplex noise (Gustavson 2012 reference). No imports.

Molt codegen constraints observed and applied:
1. No math.floor — use custom _ifloor based on int()
2. No module-level list globals accessed inside functions — use local lists
3. No tuple returns — molt resets through-variable to nil after if/else
4. No if/elif/else chains in functions that return through a variable —
   molt's goto jump pattern is emitted as comments, not real Lua gotos.
   Use cascading if-only chains (like _grad_dot pattern in compiled output),
   where the final branch is the default (no guard). This avoids the nil reset.
"""


def _ifloor(x: float) -> int:
    n = int(x)
    if x < 0.0 and x != float(n):
        return n - 1
    return n


def _perm_lookup(i: int) -> int:
    n = i % 256
    if n < 32:
        t = [
            151,
            160,
            137,
            91,
            90,
            15,
            131,
            13,
            201,
            95,
            96,
            53,
            194,
            233,
            7,
            225,
            140,
            36,
            103,
            30,
            69,
            142,
            8,
            99,
            37,
            240,
            21,
            10,
            23,
            190,
            6,
            148,
        ]
        return t[n]
    if n < 64:
        t = [
            247,
            120,
            234,
            75,
            0,
            26,
            197,
            62,
            94,
            252,
            219,
            203,
            117,
            35,
            11,
            32,
            57,
            177,
            33,
            88,
            237,
            149,
            56,
            87,
            174,
            20,
            125,
            136,
            171,
            168,
            68,
            175,
        ]
        return t[n - 32]
    if n < 96:
        t = [
            74,
            165,
            71,
            134,
            139,
            48,
            27,
            166,
            77,
            146,
            158,
            231,
            83,
            111,
            229,
            122,
            60,
            211,
            133,
            230,
            220,
            105,
            92,
            41,
            55,
            46,
            245,
            40,
            244,
            102,
            143,
            54,
        ]
        return t[n - 64]
    if n < 128:
        t = [
            65,
            25,
            63,
            161,
            1,
            216,
            80,
            73,
            209,
            76,
            132,
            187,
            208,
            89,
            18,
            169,
            200,
            196,
            135,
            130,
            116,
            188,
            159,
            86,
            164,
            100,
            109,
            198,
            173,
            186,
            3,
            64,
        ]
        return t[n - 96]
    if n < 160:
        t = [
            52,
            217,
            226,
            250,
            124,
            123,
            5,
            202,
            38,
            147,
            118,
            126,
            255,
            82,
            85,
            212,
            207,
            206,
            59,
            227,
            47,
            16,
            58,
            17,
            182,
            189,
            28,
            42,
            223,
            183,
            170,
            213,
        ]
        return t[n - 128]
    if n < 192:
        t = [
            119,
            248,
            152,
            2,
            44,
            154,
            163,
            70,
            221,
            153,
            101,
            155,
            167,
            43,
            172,
            9,
            129,
            22,
            39,
            253,
            19,
            98,
            108,
            110,
            79,
            113,
            224,
            232,
            178,
            185,
            112,
            104,
        ]
        return t[n - 160]
    if n < 224:
        t = [
            218,
            246,
            97,
            228,
            251,
            34,
            242,
            193,
            238,
            210,
            144,
            12,
            191,
            179,
            162,
            241,
            81,
            51,
            145,
            235,
            249,
            14,
            239,
            107,
            49,
            192,
            214,
            31,
            181,
            199,
            106,
            157,
        ]
        return t[n - 192]
    t = [
        184,
        84,
        204,
        176,
        115,
        121,
        50,
        45,
        127,
        4,
        150,
        254,
        138,
        236,
        205,
        93,
        222,
        114,
        67,
        29,
        24,
        72,
        243,
        141,
        128,
        195,
        78,
        66,
        215,
        61,
        156,
        180,
    ]
    return t[n - 224]


# Gradient dot product using cascading if (no if/elif/else) so molt emits
# a non-goto pattern — same pattern as the working _grad_dot in compiled output.
def _grad_dot(gi: int, x: float, y: float, z: float) -> float:
    h = gi % 12
    r = -y - z  # default (h == 11)
    if h == 0:
        r = x + y
    if h == 1:
        r = -x + y
    if h == 2:
        r = x - y
    if h == 3:
        r = -x - y
    if h == 4:
        r = x + z
    if h == 5:
        r = -x + z
    if h == 6:
        r = x - z
    if h == 7:
        r = -x - z
    if h == 8:
        r = y + z
    if h == 9:
        r = -y + z
    if h == 10:
        r = y - z
    return r


# Simplex offset functions: using cascading if (no elif/else).
# Each function encodes one component of (i1,j1,k1,i2,j2,k2).
# The 6 simplex cases map to 3x2 comparisons:
#   case A: x0>=y0, y0>=z0  -> (1,0,0, 1,1,0)
#   case B: x0>=y0, x0>=z0  -> (1,0,0, 1,0,1)
#   case C: x0>=y0, else     -> (0,0,1, 1,0,1)
#   case D: x0<y0,  y0<z0   -> (0,0,1, 0,1,1)
#   case E: x0<y0,  x0<z0   -> (0,1,0, 0,1,1)
#   case F: x0<y0,  else     -> (0,1,0, 1,1,0)


def _sx_i1(x0: float, y0: float, z0: float) -> int:
    # 1 for cases A,B; 0 for C,D,E,F
    r = 0
    if x0 >= y0 and y0 >= z0:
        r = 1
    if x0 >= y0 and y0 < z0 and x0 >= z0:
        r = 1
    return r


def _sx_j1(x0: float, y0: float, z0: float) -> int:
    # 1 for cases E,F; 0 for A,B,C,D
    r = 0
    if x0 < y0 and y0 >= z0 and x0 < z0:
        r = 1
    if x0 < y0 and y0 >= z0 and x0 >= z0:
        r = 1
    return r


def _sx_k1(x0: float, y0: float, z0: float) -> int:
    # 1 for cases C,D; 0 for A,B,E,F
    r = 0
    if x0 >= y0 and y0 < z0 and x0 < z0:
        r = 1
    if x0 < y0 and y0 < z0:
        r = 1
    return r


def _sx_i2(x0: float, y0: float, z0: float) -> int:
    # 1 for cases A,B,C,F; 0 for D,E
    r = 0
    if x0 >= y0:
        r = 1
    if x0 < y0 and y0 >= z0 and x0 >= z0:
        r = 1
    return r


def _sx_j2(x0: float, y0: float, z0: float) -> int:
    # 1 for cases A,D,E,F; 0 for B,C
    r = 0
    if x0 >= y0 and y0 >= z0:
        r = 1
    if x0 < y0:
        r = 1
    return r


def _sx_k2(x0: float, y0: float, z0: float) -> int:
    # 1 for cases B,C,D,E; 0 for A,F
    r = 0
    if x0 >= y0 and y0 < z0 and x0 >= z0:
        r = 1
    if x0 >= y0 and y0 < z0 and x0 < z0:
        r = 1
    if x0 < y0 and y0 < z0:
        r = 1
    if x0 < y0 and y0 >= z0 and x0 < z0:
        r = 1
    return r


def noise3(x: float, y: float, z: float) -> float:
    F3 = 0.3333333333333333
    G3 = 0.16666666666666666

    s = (x + y + z) * F3
    i = _ifloor(x + s)
    j = _ifloor(y + s)
    k = _ifloor(z + s)

    t = (i + j + k) * G3
    x0 = x - (i - t)
    y0 = y - (j - t)
    z0 = z - (k - t)

    i1 = _sx_i1(x0, y0, z0)
    j1 = _sx_j1(x0, y0, z0)
    k1 = _sx_k1(x0, y0, z0)
    i2 = _sx_i2(x0, y0, z0)
    j2 = _sx_j2(x0, y0, z0)
    k2 = _sx_k2(x0, y0, z0)

    x1 = x0 - i1 + G3
    y1 = y0 - j1 + G3
    z1 = z0 - k1 + G3
    x2 = x0 - i2 + 2.0 * G3
    y2 = y0 - j2 + 2.0 * G3
    z2 = z0 - k2 + 2.0 * G3
    x3 = x0 - 1.0 + 3.0 * G3
    y3 = y0 - 1.0 + 3.0 * G3
    z3 = z0 - 1.0 + 3.0 * G3

    ii = i & 255
    jj = j & 255
    kk = k & 255

    gi0 = _perm_lookup(ii + _perm_lookup(jj + _perm_lookup(kk))) % 12
    gi1 = _perm_lookup(ii + i1 + _perm_lookup(jj + j1 + _perm_lookup(kk + k1))) % 12
    gi2 = _perm_lookup(ii + i2 + _perm_lookup(jj + j2 + _perm_lookup(kk + k2))) % 12
    gi3 = _perm_lookup(ii + 1 + _perm_lookup(jj + 1 + _perm_lookup(kk + 1))) % 12

    t0 = 0.6 - x0 * x0 - y0 * y0 - z0 * z0
    n0 = 0.0
    if t0 >= 0.0:
        t0 = t0 * t0
        n0 = t0 * t0 * _grad_dot(gi0, x0, y0, z0)

    t1 = 0.6 - x1 * x1 - y1 * y1 - z1 * z1
    n1 = 0.0
    if t1 >= 0.0:
        t1 = t1 * t1
        n1 = t1 * t1 * _grad_dot(gi1, x1, y1, z1)

    t2 = 0.6 - x2 * x2 - y2 * y2 - z2 * z2
    n2 = 0.0
    if t2 >= 0.0:
        t2 = t2 * t2
        n2 = t2 * t2 * _grad_dot(gi2, x2, y2, z2)

    t3 = 0.6 - x3 * x3 - y3 * y3 - z3 * z3
    n3 = 0.0
    if t3 >= 0.0:
        t3 = t3 * t3
        n3 = t3 * t3 * _grad_dot(gi3, x3, y3, z3)

    return 32.0 * (n0 + n1 + n2 + n3)


def fbm3(
    x: float, y: float, z: float, octaves: int, persistence: float, lacunarity: float
) -> float:
    total = 0.0
    amplitude = 1.0
    frequency = 1.0
    max_val = 0.0
    for _i in range(octaves):
        total = total + noise3(x * frequency, y * frequency, z * frequency) * amplitude
        max_val = max_val + amplitude
        amplitude = amplitude * persistence
        frequency = frequency * lacunarity
    if max_val == 0.0:
        return 0.0
    return total / max_val


if __name__ == "__main__":
    v1 = noise3(0.5, 1.5, 2.5)
    v2 = noise3(0.1, 0.2, 0.3)
    v3 = noise3(10.5, -3.2, 7.8)
    v4 = fbm3(0.5, 0.5, 0.5, 4, 0.5, 2.0)
    v5 = fbm3(1.3, 2.7, 0.4, 6, 0.5, 2.0)
    print("noise3(0.5, 1.5, 2.5) =", v1)
    print("noise3(0.1, 0.2, 0.3) =", v2)
    print("noise3(10.5, -3.2, 7.8) =", v3)
    print("fbm3(0.5, 0.5, 0.5, 4, 0.5, 2.0) =", v4)
    print("fbm3(1.3, 2.7, 0.4, 6, 0.5, 2.0) =", v5)
    all_valid = (
        v1 >= -1.01
        and v1 <= 1.01
        and v2 >= -1.01
        and v2 <= 1.01
        and v3 >= -1.01
        and v3 <= 1.01
        and v4 >= -1.01
        and v4 <= 1.01
        and v5 >= -1.01
        and v5 <= 1.01
    )
    print("all_in_range =", all_valid)
