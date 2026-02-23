# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: differential coverage for importlib.util.cache_from_source intrinsic."""

import importlib.util
import os
import re
import tempfile


def _canonical_cache_basename(cache_path: str) -> str:
    return re.sub(
        r"(?:\.)?cpython-\d+(?:\.opt-[^.]+)?",
        "",
        os.path.basename(cache_path),
    )


with tempfile.TemporaryDirectory(prefix="molt_importlib_cache_from_source_") as root:
    py_path = os.path.join(root, "name.py")
    non_py_path = os.path.join(root, "name")
    with open(py_path, "w", encoding="utf-8") as handle:
        handle.write("value = 1\n")
    with open(non_py_path, "w", encoding="utf-8") as handle:
        handle.write("value\n")

    py_cache = importlib.util.cache_from_source(py_path)
    py_layout = os.path.join(
        os.path.basename(os.path.dirname(py_cache)),
        _canonical_cache_basename(py_cache),
    )
    py_layout_ok = py_layout == os.path.join("__pycache__", "name.pyc")
    print("py_layout", py_layout)
    print("py_layout_ok", py_layout_ok)
    assert py_layout_ok

    non_py_cache = importlib.util.cache_from_source(non_py_path)
    non_py_suffix_c = non_py_cache.endswith("c")
    print("non_py_suffix_c", non_py_suffix_c)
    assert non_py_suffix_c

    optimized_a = importlib.util.cache_from_source(
        py_path,
        optimization="deterministic",
    )
    optimized_b = importlib.util.cache_from_source(
        py_path,
        optimization="deterministic",
    )
    optimization_deterministic = optimized_a == optimized_b
    print("optimization_deterministic", optimization_deterministic)
    assert optimization_deterministic
