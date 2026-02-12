# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: runpy.run_path executes intrinsic-backed source payloads."""

import os
import runpy
import tempfile


root = tempfile.mkdtemp()
path = os.path.join(root, "mod.py")
with open(path, "w", encoding="utf-8") as handle:
    handle.write("value = 7\n")

ns = runpy.run_path(path)
print(ns.get("value"))
print(ns.get("__name__"))
print(bool(ns.get("__file__", "").endswith("mod.py")))
print(ns.get("__package__"))

ns2 = runpy.run_path(path, init_globals={"seed": 99}, run_name="pkg.tool")
print(ns2.get("seed"))
print(ns2.get("__name__"))
print(ns2.get("__package__"))
