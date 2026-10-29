# MOLT_ENV: PYTHONPATH=src:tests/differential/stdlib
# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-530 expect_molt_returncode=1 expect_molt_stdout_sha256=a9ac0c3ac83c40e1b4c3416066d63d324ee9f8c144641dfeed72d140b6557245
"""Purpose: differential coverage for importlib spec from file."""

import importlib.util
import os


path = os.path.join(os.path.dirname(__file__), "res_pkg", "__init__.py")
spec = importlib.util.spec_from_file_location("res_pkg_copy", path)
print(spec is not None)
if spec and spec.loader:
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    print(getattr(module, "VALUE", None))
