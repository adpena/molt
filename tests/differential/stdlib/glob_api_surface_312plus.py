# MOLT_META: min_py=3.13
# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write
"""Differential coverage for CPython 3.12+ glob API surface."""

from __future__ import annotations

import glob
import os
import tempfile
import warnings


with tempfile.TemporaryDirectory(prefix="molt_glob_api_") as root:
    with open(os.path.join(root, "a.txt"), "w", encoding="utf-8") as handle:
        handle.write("a")
    with open(os.path.join(root, "b.py"), "w", encoding="utf-8") as handle:
        handle.write("b")
    with open(os.path.join(root, ".hidden.py"), "w", encoding="utf-8") as handle:
        handle.write("h")

    print("escape_str", glob.escape("a*b?[c]/d"))
    print("escape_bytes", glob.escape(b"a*b?[c]/d"))
    print("__all__", glob.__all__)

    print("translate_default", glob.translate("*.py"))
    print("translate_recursive", glob.translate("**/*.py", recursive=True))
    print(
        "translate_recursive_hidden",
        glob.translate("**/*.py", recursive=True, include_hidden=True),
    )
    print("translate_seps", glob.translate("a*b", seps="/:"))

    for payload in (b"*.py", bytearray(b"*.py")):
        try:
            glob.translate(payload)
        except Exception as exc:
            print(
                "translate_bad_type",
                type(payload).__name__,
                type(exc).__name__,
                str(exc),
            )

    with warnings.catch_warnings(record=True) as records:
        warnings.simplefilter("always", DeprecationWarning)
        print("glob0_literal", glob.glob0(root, "a.txt"))
        print("glob0_empty", glob.glob0(root, ""))
        print("glob1_wild", sorted(glob.glob1(root, "*.py")))
        print(
            "glob_depr_count",
            sum(1 for rec in records if issubclass(rec.category, DeprecationWarning)),
        )
