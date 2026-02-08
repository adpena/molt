"""Purpose: intrinsic-backed pathlib reverse division parity."""

from pathlib import Path


print(("a" / Path("b")).as_posix())
print(("a" / Path("/b")).as_posix())
print((Path("root") / Path("leaf")).as_posix())
print(("prefix" / Path("nested") / "tail").as_posix())
