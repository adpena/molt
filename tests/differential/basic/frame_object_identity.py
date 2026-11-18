# MOLT_META: expect_fail=molt expect_fail_reason=open_finding finding=HF-903 expect_molt_returncode=0 expect_molt_stdout_sha256=83593c70a26a059c7586465234d7611bd0fc8b4a262b7038bb8ef88c6b96c036
"""One frame object per activation, for ordinary frames and fused generator frames."""

import sys


def same_frame():
    return sys._getframe() is sys._getframe()


def child():
    return sys._getframe(0)


def parent():
    first = child()
    second = child()
    return first.f_back is second.f_back, first.f_back is sys._getframe()


def frames(n):
    for i in range(n):
        yield sys._getframe() is sys._getframe()


def consumer():
    out = []
    for same in frames(2):
        out.append(same)
    return out


print(same_frame(), parent(), consumer())
