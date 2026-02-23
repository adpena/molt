# MOLT_ENV: MOLT_CAPABILITIES=fs.read,env.read
"""Purpose: validate intrinsic-backed importlib.resources._functional path API."""

import os
import warnings

import importlib.resources._functional as functional


with functional.open_text(
    "tests.differential.stdlib", "res_pkg", "data.txt", encoding="utf-8"
) as handle:
    print("open_text", handle.read().strip())

print(
    "read_text",
    functional.read_text(
        "tests.differential.stdlib", "res_pkg", "data.txt", encoding="utf-8"
    ).strip(),
)

print(
    "read_binary",
    functional.read_binary("tests.differential.stdlib", "res_pkg", "data.txt")[:5],
)

with functional.open_binary("tests.differential.stdlib", "res_pkg", "data.txt") as handle:
    print("open_binary", handle.read(5))

print(
    "is_resource_file",
    functional.is_resource("tests.differential.stdlib", "res_pkg", "data.txt"),
)
print("is_resource_dir", functional.is_resource("tests.differential.stdlib", "res_pkg"))

with warnings.catch_warnings(record=True) as captured:
    warnings.simplefilter("always", DeprecationWarning)
    names = sorted(list(functional.contents("tests.differential.stdlib", "res_pkg")))
print("contents_has_data", "data.txt" in names)
print(
    "contents_warning",
    bool(captured),
    captured[0].category.__name__ if captured else "",
)

with functional.path("tests.differential.stdlib", "res_pkg", "data.txt") as value:
    path = os.fspath(value)
    print("path_exists", os.path.exists(path))
