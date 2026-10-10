"""Minimal `trace` subset for Molt."""

from __future__ import annotations


class Trace:
    def __init__(
        self,
        count: bool = True,
        trace: bool = True,
        countfuncs: bool = False,
        countcallers: bool = False,
        ignoremods: tuple[str, ...] = (),
        ignoredirs: tuple[str, ...] = (),
        infile: object | None = None,
        outfile: object | None = None,
        timing: bool = False,
    ) -> None:
        self.count = bool(count)
        self.trace = bool(trace)
        self.countfuncs = bool(countfuncs)
        self.countcallers = bool(countcallers)
        self.ignoremods = tuple(ignoremods)
        self.ignoredirs = tuple(ignoredirs)
        self.infile = infile
        self.outfile = outfile
        self.timing = bool(timing)

    def runfunc(self, func, /, *args, **kwargs):
        return func(*args, **kwargs)


__all__ = ["Trace"]
