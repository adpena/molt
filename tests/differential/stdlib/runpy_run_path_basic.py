# MOLT_ENV: MOLT_CAPABILITIES=fs.read,fs.write,env.read
"""Purpose: runpy.run_path executes intrinsic-backed source payloads."""

import os
import runpy
import sys
import tempfile


root = tempfile.mkdtemp()
path = os.path.join(root, "mod.py")
with open(path, "w", encoding="utf-8") as handle:
    handle.write("import sys\nvalue = 7\nargv0_seen = sys.argv[0]\n")

prior_argv0 = sys.argv[0]
ns = runpy.run_path(path)
print(ns.get("value"))
print(ns.get("__name__"))
print(bool(ns.get("__file__", "").endswith("mod.py")))
print(ns.get("__package__"))
print(str(ns.get("argv0_seen", "")).endswith("mod.py"))
print(sys.argv[0] == prior_argv0)

prior_argv0_2 = sys.argv[0]
ns2 = runpy.run_path(path, init_globals={"seed": 99}, run_name="pkg.tool")
print(ns2.get("seed"))
print(ns2.get("__name__"))
print(ns2.get("__package__"))
print(str(ns2.get("argv0_seen", "")).endswith("mod.py"))
print(sys.argv[0] == prior_argv0_2)
