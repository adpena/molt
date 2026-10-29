# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-383 expect_molt_returncode=0 expect_molt_stdout_sha256=8e386844c76b623adb454413edfbea7cb46ed673ea82998dd8c82c1e490addf4
"""Purpose: differential coverage for inspect getsource basic."""

import inspect


def foo():
    return 1


try:
    print(inspect.getsource(foo).strip().splitlines()[-1])
except Exception as exc:
    print(type(exc).__name__)
