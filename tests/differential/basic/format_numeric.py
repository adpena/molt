"""Purpose: differential coverage for format numeric."""

value = 12345

print(f"{value:08d}")
print(f"{value:+08d}")
print(f"{value:#010x}")
print(f"{value:_d}")
print(f"{value:,d}")

big = 1 << 60
print(f"{big:#x}")
print(f"{big:_d}")

f = 12.3456
print(f"{f:.2f}")
print(f"{f:8.2f}")
print(f"{f:+.3e}")
print(f"{f:.4g}")
print(f"{f:#.0f}")
print(f"{f:%}")

print(f"{'hello':.3s}")

s = "\u00e9"
print(f"{s!r}")
print(f"{s!a}")
print(f"{s!s}")
