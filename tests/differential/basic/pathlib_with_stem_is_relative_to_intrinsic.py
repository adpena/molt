from pathlib import Path


renamed = Path("alpha/bravo.txt").with_stem("charlie")
with_stem_ok = renamed == Path("alpha/charlie.txt")

empty_stem_error = None
try:
    Path("alpha.txt").with_stem("")
except Exception as exc:  # parity: concrete exception type/value
    empty_stem_error = type(exc).__name__

missing_arg_type_error = False
try:
    Path("alpha").is_relative_to()
except TypeError:
    missing_arg_type_error = True

print(
    "PATHLIB",
    with_stem_ok,
    empty_stem_error,
    missing_arg_type_error,
    Path("alpha/bravo").is_relative_to("alpha"),
    Path("alpha/bravo").is_relative_to("delta"),
)
