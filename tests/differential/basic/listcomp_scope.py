"""Purpose: differential coverage for listcomp scope."""

i = 3
vals = [i for i in range(i)]
print(f"shadow_vals:{vals}")
print(f"shadow_i:{i}")

x = 99
vals2 = [x for x in range(2)]
print(f"outer_x:{x}")
print(f"vals2:{vals2}")

vals3 = [(i, j) for i in range(3) for j in range(i)]
print(f"nested:{vals3}")

try:
    [y for y in range(1)]
    print(y)
except NameError as exc:
    print(f"leak_err:{exc}")
