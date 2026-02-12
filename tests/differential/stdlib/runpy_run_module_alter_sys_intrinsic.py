# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: runpy.run_module(alter_sys=True) uses intrinsic-backed sys.modules swap+restore."""

import os
import runpy
import sys
import tempfile
import types


with tempfile.TemporaryDirectory() as tmp:
    mod_path = os.path.join(tmp, "modalter.py")
    with open(mod_path, "w", encoding="utf-8") as handle:
        handle.write("value = 17\n")

    original_path = list(sys.path)
    prior_argv0 = sys.argv[0]
    sentinel = types.SimpleNamespace(name="sentinel")
    sys.modules["alias.runner"] = sentinel
    try:
        sys.path.insert(0, tmp)
        ns = runpy.run_module("modalter", run_name="alias.runner", alter_sys=True)
        print(ns.get("value"))
        print(ns.get("__name__"))
        print("alias.runner" in sys.modules)
        print(sys.modules.get("alias.runner") is sentinel)
        print(sys.argv[0] == prior_argv0)
    finally:
        sys.path[:] = original_path
        sys.modules.pop("modalter", None)
        sys.modules.pop("alias.runner", None)
