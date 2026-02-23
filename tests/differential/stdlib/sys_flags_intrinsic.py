"""Purpose: differential coverage for intrinsic-backed sys.flags semantics."""

import sys


flags = sys.flags
sequence_fields = (
    "debug",
    "inspect",
    "interactive",
    "optimize",
    "dont_write_bytecode",
    "no_user_site",
    "no_site",
    "ignore_environment",
    "verbose",
    "bytes_warning",
    "quiet",
    "hash_randomization",
    "isolated",
    "dev_mode",
    "utf8_mode",
    "warn_default_encoding",
    "safe_path",
    "int_max_str_digits",
)

assert isinstance(flags, tuple)
assert len(flags) == len(sequence_fields)

for index, name in enumerate(sequence_fields):
    value = getattr(flags, name)
    assert flags[index] == value
    assert isinstance(value, int)

assert tuple(flags) == tuple(getattr(flags, name) for name in sequence_fields)

rendered = repr(flags)
assert isinstance(rendered, str)
if rendered.startswith("sys.flags(") and rendered.endswith(")"):
    last_position = -1
    for name in sequence_fields:
        marker = f"{name}="
        position = rendered.find(marker)
        assert position > last_position, (name, rendered)
        last_position = position

if hasattr(flags, "gil"):
    gil_value = getattr(flags, "gil")
    assert isinstance(gil_value, int)

assert isinstance(flags.n_fields, int)
assert flags.n_fields >= len(sequence_fields)
assert flags.n_sequence_fields == len(sequence_fields)
assert isinstance(flags.n_unnamed_fields, int)
assert flags.n_unnamed_fields >= 0

print("ok")
