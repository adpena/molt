# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-207 expect_molt_returncode=1 expect_molt_stdout_sha256=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855
"""Purpose: copy.Error is an ordinary module class and construction has no side effects."""

import copy
import sys


class Uncopyable:
    __reduce_ex__ = None
    __reduce__ = None


def probe(label, func, value):
    try:
        func(value)
    except copy.Error as exc:
        print(label, "copy.Error", type(exc) is copy.Error, str(exc).split(" of ")[0])
    except Exception as exc:  # noqa: BLE001 - the observation is the escape
        print(label, "escaped", type(exc).__name__)
    else:
        print(label, "no error")


def main():
    print("alias", copy.error is copy.Error)
    print("base", copy.Error.__mro__[1].__name__, copy.Error.__module__)
    error = copy.Error("constructed")
    print("constructed", error.args, sys.exc_info()[0])
    try:
        raise copy.Error("raised")
    except copy.Error as exc:
        print("raised", exc.args, exc.__context__)
    probe("copy", copy.copy, Uncopyable())
    probe("deepcopy", copy.deepcopy, Uncopyable())


if __name__ == "__main__":
    main()
