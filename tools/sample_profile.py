"""Read macOS `/usr/bin/sample` reports: the self-time leaderboard and the call graph.

This is the one parser for every Molt tool that samples a process. A report has
a "Call graph:" tree, whose lines carry inclusive sample counts and indent by
depth, and a "Sort by top of stack" leaderboard of self-time counts. Symbols
may be Rust v0-mangled; callers match them by substring and demangle them
for display.
"""

from __future__ import annotations

import re
from collections import Counter
from dataclasses import dataclass
from pathlib import Path

_FRAME = re.compile(r"^([\s+!:|]*)(\d+) (.+?)  \(in ([^)]+)\)")


@dataclass(frozen=True)
class Frame:
    depth: int
    samples: int
    symbol: str
    lib: str


def self_time(text: str, *, top_n: int | None = None) -> list[dict]:
    """The self-time leaders as ``{symbol, self_samples, lib}``, heaviest first.

    The leaderboard line form is ``<symbol>  (in <lib>)  <count>`` or
    ``<symbol>  <count>``; the count is the trailing token.
    """
    lines = text.splitlines()
    start = next(
        (i + 1 for i, line in enumerate(lines) if "Sort by top of stack" in line),
        None,
    )
    if start is None:
        return []
    out: list[dict] = []
    for line in lines[start:]:
        stripped = line.strip()
        if not stripped:
            if out:
                break
            continue
        if stripped.startswith("Binary Images"):
            break
        tokens = stripped.rsplit(None, 1)
        if len(tokens) != 2 or not tokens[1].isdigit():
            continue
        rest = tokens[0].strip()
        lib = None
        symbol = rest
        if "(in " in rest:
            head, _, tail = rest.partition("(in ")
            symbol = head.strip()
            lib = tail.split(")")[0].strip()
        out.append({"symbol": symbol, "self_samples": int(tokens[1]), "lib": lib})
        if top_n is not None and len(out) >= top_n:
            break
    return out


def read_self_time(path: Path, *, top_n: int | None = None) -> list[dict]:
    """`self_time` of a report file; an unreadable file has no leaders."""
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return []
    return self_time(text, top_n=top_n)


def call_frames(text: str) -> list[Frame]:
    """Every frame of the call graph, in report order."""
    lines = text.splitlines()
    try:
        start = lines.index("Call graph:")
    except ValueError:
        return []
    frames: list[Frame] = []
    for line in lines[start + 1 :]:
        if line.startswith("Total number"):
            break
        match = _FRAME.match(line)
        if match:
            frames.append(
                Frame(
                    depth=len(match.group(1)),
                    samples=int(match.group(2)),
                    symbol=match.group(3),
                    lib=match.group(4),
                )
            )
    return frames


def self_time_from_frames(frames: list[Frame]) -> list[dict]:
    """Self samples per (symbol, lib) from the call graph, heaviest first.

    A frame's self samples are its samples minus its direct children's. Unlike
    the "Sort by top of stack" leaderboard, the call graph keeps full symbol
    names and drops no frame below a count threshold.
    """
    totals: Counter[tuple[str, str]] = Counter()
    stack: list[tuple[Frame, int]] = []  # (frame, samples claimed by children)

    def close(entry: tuple[Frame, int]) -> None:
        frame, children = entry
        totals[(frame.symbol, frame.lib)] += max(frame.samples - children, 0)

    for frame in frames:
        while stack and stack[-1][0].depth >= frame.depth:
            close(stack.pop())
        if stack:
            parent, children = stack[-1]
            stack[-1] = (parent, children + frame.samples)
        stack.append((frame, 0))
    while stack:
        close(stack.pop())
    return [
        {"symbol": symbol, "self_samples": samples, "lib": lib}
        for (symbol, lib), samples in totals.most_common()
        if samples
    ]


def direct_callers(frames: list[Frame], target: str) -> Counter[str]:
    """Inclusive samples of frames matching ``target``, keyed by direct caller.

    A frame matches when ``target`` occurs in its symbol. Recursion through the
    target counts once, at the outermost matching frame.
    """
    callers: Counter[str] = Counter()
    stack: list[Frame] = []
    for frame in frames:
        while stack and stack[-1].depth >= frame.depth:
            stack.pop()
        if target in frame.symbol and stack and target not in stack[-1].symbol:
            callers[stack[-1].symbol] += frame.samples
        stack.append(frame)
    return callers
