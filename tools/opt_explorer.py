#!/usr/bin/env python3
"""Optimization explorer: what Molt emitted for a program, and where its time goes.

Commands:
  anatomy  Machine-code anatomy of each compiled function in a symbolized native
           build: instructions, branches and calls by callee, for the whole
           function and for each loop (a backward branch marks a loop body).
  profile  Sample one run (macOS `/usr/bin/sample`) and split self time into
           cost buckets: compiled code, runtime entry, reference counting,
           allocation, attribute access, call dispatch, exceptions, other
           runtime and system libraries; list the hottest symbols and, on
           request, the direct callers of a symbol.

The input is a Python file, which this tool builds with symbols kept, or an
existing binary. Compiled functions are the symbols `<module>__<qualname>`;
`--module` names the module (default: the input file's stem).

Usage:
    python3 tools/opt_explorer.py anatomy prog.py [--function REGEX] [--json]
    python3 tools/opt_explorer.py profile prog.py [--callers SYMBOL] [--json]
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
from collections import Counter
from collections.abc import Mapping
from dataclasses import dataclass, field
from enum import StrEnum
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools import sample_profile  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402

ROOT = Path(__file__).resolve().parents[1]
_COMMANDS = CommandExecutor.for_file(__file__, stem="MOLT_BENCH")

_HEADER = re.compile(r"^[0-9a-fA-F]+ <(.+)>:$")
_INSN = re.compile(r"^\s*([0-9a-fA-F]+):\s+(\S+)\s*(.*)$")
_ADDRESS = re.compile(r"(?<![#+\w])0x([0-9a-fA-F]+)")
_TARGET_SYMBOL = re.compile(r"<([^>+]+)(?:\+0x[0-9a-fA-F]+)?>")
_ARM64_BRANCHES = frozenset({"b", "br", "cbz", "cbnz", "tbz", "tbnz"})
_ARM64_INDIRECT_CALLS = frozenset({"blr", "blraa", "blraaz", "blrab", "blrabz"})


class CostBucket(StrEnum):
    COMPILED = "compiled code"
    ENTRY = "runtime entry"
    REFCOUNT = "reference counting"
    ALLOCATION = "allocation"
    ATTRIBUTE = "attribute access"
    CALL = "call dispatch"
    EXCEPTION = "exceptions"
    SYSTEM = "system libraries"
    OTHER = "other runtime"
    UNRESOLVED = "unresolved symbols"


# Ordered callable-path hints, after demangling and removing generic arguments.
# A type argument describes an instantiation, not the work the function does.
COST_BUCKETS: tuple[tuple[CostBucket, tuple[str, ...]], ...] = (
    (
        CostBucket.ENTRY,
        (
            "tlv_get_addr",
            "RuntimeExecutionGuard",
            "GilGuard",
            "RUNTIME_EXECUTION_LEASE",
            "runtime_state_for_gil",
            "touch_tls_guard",
            "concurrency",
            "thread::local",
            "state::tls::",
        ),
    ),
    (CostBucket.REFCOUNT, ("inc_ref", "dec_ref")),
    (
        CostBucket.ALLOCATION,
        ("heap_lifecycle", "take_best_buffer"),
    ),
    (
        CostBucket.ATTRIBUTE,
        (
            "attr",
            "field_sites",
            "field_storage",
            "accessors",
            "class_lookup",
            "instance_dict",
            "shape_id",
            "class_shape",
        ),
    ),
    (
        CostBucket.CALL,
        (
            "molt_call",
            "molt_function",
            "::call::",
            "frame_binding",
            "trampoline",
            "invocation",
        ),
    ),
    (CostBucket.EXCEPTION, ("exception",)),
)
_MANGLED_SYMBOL = re.compile(r"^(?:_+(?:R[0-9]*[CNMXYIB]|Z)|\?)")
_ALLOCATION_ACTION = re.compile(
    r"(?:^|_)(?:alloc|allocate|dealloc|deallocate|realloc|malloc|calloc|free)(?:_|$)"
)


@dataclass(frozen=True)
class Insn:
    address: int
    mnemonic: str
    operands: str


@dataclass
class Region:
    start: int
    end: int
    instructions: int = 0
    branches: int = 0
    calls: Counter[str] = field(default_factory=Counter)

    def to_json(self) -> dict:
        return {
            "start": hex(self.start),
            "end": hex(self.end),
            "instructions": self.instructions,
            "branches": self.branches,
            "calls": dict(self.calls.most_common()),
        }


@dataclass
class FunctionAnatomy:
    name: str
    whole: Region
    loops: list[Region]

    def to_json(self) -> dict:
        return {
            "name": self.name,
            **self.whole.to_json(),
            "loops": [loop.to_json() for loop in self.loops],
        }


def parse_disassembly(text: str) -> tuple[str, dict[str, list[Insn]]]:
    """(architecture, instructions by function) from `objdump -d` output."""
    arch = "unknown"
    functions: dict[str, list[Insn]] = {}
    current: list[Insn] | None = None
    for line in text.splitlines():
        if "file format" in line:
            fmt = line.rsplit("file format", 1)[1].strip()
            if "arm64" in fmt or "aarch64" in fmt:
                arch = "arm64"
            elif "x86-64" in fmt or "x86_64" in fmt:
                arch = "x86_64"
            continue
        if match := _HEADER.match(line.strip()):
            current = functions.setdefault(_plain_symbol(match.group(1)), [])
            continue
        if current is not None and (match := _INSN.match(line)):
            mnemonic = match.group(2)
            operands = match.group(3).split(";", 1)[0].strip()
            current.append(Insn(int(match.group(1), 16), mnemonic, operands))
    return arch, functions


def _plain_symbol(name: str) -> str:
    """A symbol without the Mach-O leading underscore."""
    return name[1:] if name.startswith("_") else name


_ARM64_ENDS = frozenset({"b", "br", "ret", "retaa", "retab"})
_X86_ENDS = frozenset({"jmp", "jmpq", "ret", "retq"})
_REGISTER = re.compile(r"^[xw](\d+)$")
_IMMEDIATE = re.compile(r"#(-?0x[0-9a-fA-F]+|-?\d+)")


def _is_call(arch: str, insn: Insn) -> bool:
    if arch == "arm64":
        return insn.mnemonic == "bl" or insn.mnemonic in _ARM64_INDIRECT_CALLS
    return insn.mnemonic in {"call", "callq"}


def _is_branch(arch: str, insn: Insn) -> bool:
    if arch == "arm64":
        return insn.mnemonic in _ARM64_BRANCHES or insn.mnemonic.startswith("b.")
    return insn.mnemonic.startswith("j")


def _ends_flow(arch: str, insn: Insn) -> bool:
    """No fallthrough: an unconditional branch or a return."""
    if arch == "arm64":
        return insn.mnemonic in _ARM64_ENDS
    return insn.mnemonic in _X86_ENDS


def _branch_target(insn: Insn) -> int | None:
    targets = _ADDRESS.findall(insn.operands)
    return int(targets[-1], 16) if targets else None


def _immediate(text: str) -> int:
    match = _IMMEDIATE.search(text)
    return int(match.group(1), 0) if match else 0


class SymbolTable:
    """Address to symbol for direct and register-materialized call targets."""

    def __init__(
        self,
        symbols: dict[int, str] | None = None,
        got: dict[int, str] | None = None,
    ) -> None:
        self.symbols = symbols or {}
        self.got = got or {}

    def name(self, address: int | None) -> str:
        if address is None:
            return "<indirect>"
        return self.symbols.get(address, f"<{address:#x}>")


def _callees(arch: str, insns: list[Insn], table: SymbolTable) -> list[str | None]:
    """The callee of each instruction (None for non-calls).

    On arm64 a call through a register is resolved when the function itself
    materialized the address: `adrp` + `add`, or `adrp` + `ldr` from a GOT slot.
    """
    out: list[str | None] = []
    registers: dict[str, int] = {}
    for insn in insns:
        operands = [part.strip() for part in insn.operands.split(",")]
        if arch == "arm64" and insn.mnemonic == "adrp" and len(operands) >= 2:
            page = _ADDRESS.search(operands[1])
            if page:
                registers[operands[0]] = int(page.group(1), 16)
        elif arch == "arm64" and insn.mnemonic == "add" and len(operands) == 3:
            if operands[1] in registers and operands[2].startswith("#"):
                registers[operands[0]] = registers[operands[1]] + _immediate(
                    operands[2]
                )
            else:
                registers.pop(operands[0], None)
        elif arch == "arm64" and insn.mnemonic == "ldr" and len(operands) >= 2:
            base = operands[1].strip("[]! ")
            slot = registers.get(base)
            registers.pop(operands[0], None)
            if slot is not None:
                slot += _immediate(insn.operands.split("[", 1)[-1])
                if slot in table.got:
                    registers[operands[0]] = -slot  # negative marks a GOT slot
        if not _is_call(arch, insn):
            if arch == "arm64" and operands and _REGISTER.match(operands[0] or ""):
                if insn.mnemonic not in {"adrp", "add", "ldr"}:
                    registers.pop(operands[0], None)
            out.append(None)
            continue
        if match := _TARGET_SYMBOL.search(insn.operands):
            out.append(_plain_symbol(match.group(1)))
        elif arch == "arm64" and operands[0] in registers:
            value = registers[operands[0]]
            out.append(table.got[-value] if value < 0 else table.name(value))
        else:
            out.append("<indirect>")
    return out


def _blocks(arch: str, insns: list[Insn]) -> tuple[list[int], dict[int, set[int]]]:
    """Basic-block leaders (instruction indexes) and successor edges."""
    index_of = {insn.address: i for i, insn in enumerate(insns)}
    leaders = {0}
    for i, insn in enumerate(insns):
        if _is_branch(arch, insn):
            target = _branch_target(insn)
            if target in index_of:
                leaders.add(index_of[target])
            if i + 1 < len(insns):
                leaders.add(i + 1)
    ordered = sorted(leaders)
    successors: dict[int, set[int]] = {}
    for position, start in enumerate(ordered):
        end = ordered[position + 1] if position + 1 < len(ordered) else len(insns)
        last = insns[end - 1]
        edges: set[int] = set()
        if _is_branch(arch, last):
            target = _branch_target(last)
            if target in index_of:
                edges.add(index_of[target])
        if not _ends_flow(arch, last) and end < len(insns):
            edges.add(end)
        successors[start] = edges
    return ordered, successors


def _natural_loops(
    leaders: list[int], successors: dict[int, set[int]]
) -> list[tuple[int, set[int]]]:
    """(header, member blocks) for each loop, from dominators and back edges."""
    entry = leaders[0]
    predecessors: dict[int, set[int]] = {block: set() for block in leaders}
    for block, targets in successors.items():
        for target in targets:
            predecessors[target].add(block)
    everything = set(leaders)
    dominators = {block: set(everything) for block in leaders}
    dominators[entry] = {entry}
    changed = True
    while changed:
        changed = False
        for block in leaders[1:]:
            preds = [dominators[p] for p in predecessors[block]]
            new = (set.intersection(*preds) if preds else set()) | {block}
            if new != dominators[block]:
                dominators[block] = new
                changed = True
    loops: dict[int, set[int]] = {}
    for block, targets in successors.items():
        for header in targets:
            if header in dominators[block]:
                body = loops.setdefault(header, {header})
                work = [block]
                while work:
                    node = work.pop()
                    if node not in body:
                        body.add(node)
                        work.extend(predecessors[node])
    return sorted(loops.items())


def _region(
    arch: str,
    insns: list[Insn],
    callees: list[str | None],
    indexes: list[int],
) -> Region:
    region = Region(insns[indexes[0]].address, insns[indexes[-1]].address)
    for i in indexes:
        region.instructions += 1
        if (callee := callees[i]) is not None:
            region.calls[callee] += 1
        elif _is_branch(arch, insns[i]):
            region.branches += 1
    return region


def analyze_function(
    arch: str, name: str, insns: list[Insn], table: SymbolTable | None = None
) -> FunctionAnatomy:
    """Counts for the whole function and for each natural loop."""
    if not insns:
        return FunctionAnatomy(name, Region(0, 0), [])
    callees = _callees(arch, insns, table or SymbolTable())
    whole = _region(arch, insns, callees, list(range(len(insns))))
    leaders, successors = _blocks(arch, insns)
    bounds = {
        start: (leaders[i + 1] if i + 1 < len(leaders) else len(insns))
        for i, start in enumerate(leaders)
    }
    loops = []
    for _header, members in _natural_loops(leaders, successors):
        indexes = sorted(i for block in members for i in range(block, bounds[block]))
        loops.append(_region(arch, insns, callees, indexes))
    loops.sort(key=lambda loop: loop.instructions)
    return FunctionAnatomy(name, whole, loops)


def _callable_path(symbol: str) -> str | None:
    """A readable callable path without template arguments or signature types.

    Preserve the self type in Rust's `<Self as Trait>::method` spelling, while
    removing its nested generic arguments. This is a linear, non-recursive
    projection, not a demangler. Unreadable or unbalanced names stay unresolved.
    """
    symbol = symbol.strip()
    if _MANGLED_SYMBOL.match(symbol):
        return None
    path: list[str] = []
    depth = 0
    qualified_self = False
    callable_start = 0
    for index, char in enumerate(symbol):
        if char == "<":
            if index == 0:
                qualified_self = True
            depth += 1
            continue
        if char == ">" and (index == 0 or symbol[index - 1] != "-"):
            if depth == 0:
                return None
            depth -= 1
            if depth == 0:
                qualified_self = False
            continue
        if char == "(":
            if depth == 0:
                break
            if qualified_self and depth == 1:
                # A function/tuple self type does not identify the callable's
                # implementation by the types in its signature.
                return None
        if depth == 0 or (qualified_self and depth == 1):
            path.append(char)
            if char.isspace() and depth == 0:
                # C++ demanglers can print a return type before the callable.
                # Spaces inside a qualified Rust self type remain its path.
                callable_start = len(path)
    if depth:
        return None
    return "".join(path[callable_start:]).strip() or None


def classify_symbol(
    symbol: str, lib: str | None, binary: str, module: str
) -> CostBucket:
    """A callable-name hint for one frame, never an allocation/entry proof."""
    path = _callable_path(symbol)
    if path is None:
        return CostBucket.UNRESOLVED
    if _plain_symbol(path).startswith(f"{module}__"):
        return CostBucket.COMPILED
    leaf = path.rsplit("::", 1)[-1]
    # Patterns first: `_tlv_get_addr` lives in libdyld but is runtime entry.
    for bucket, patterns in COST_BUCKETS:
        if any(pattern in path for pattern in patterns):
            return bucket
        if bucket is CostBucket.ALLOCATION and (
            _ALLOCATION_ACTION.search(leaf)
            or (
                path.startswith("alloc::")
                and leaf.startswith(("grow", "reserve", "try_reserve"))
            )
        ):
            return bucket
    if lib is not None and lib != binary:
        return CostBucket.SYSTEM
    return CostBucket.OTHER


def cost_split(
    leaders: list[dict], binary: str, module: str, names: Mapping[str, str]
) -> list[tuple[CostBucket, int, float]]:
    """(bucket, samples, share) for every bucket with samples, heaviest first."""
    totals: Counter[CostBucket] = Counter()
    for row in leaders:
        totals[
            classify_symbol(names[row["symbol"]], row.get("lib"), binary, module)
        ] += row["self_samples"]
    grand = sum(totals.values()) or 1
    return [(bucket, n, n / grand) for bucket, n in totals.most_common()]


def build_symbolized(source: Path, out_dir: Path) -> Path:
    """Build `source` as a release binary that keeps its symbols."""
    env = dict(os.environ, MOLT_KEEP_SYMBOLS="1")
    command = [sys.executable, "-m", "molt.cli", "build", "--build-profile", "release"]
    _COMMANDS.run(
        [*command, "--out-dir", str(out_dir), str(source)],
        env=env,
        check=True,
        timeout=3600,
    )
    binary = out_dir / f"{source.stem}_molt"
    if not binary.is_file():
        raise SystemExit(f"opt_explorer: the build produced no binary at {binary}")
    return binary


def _resolve_binary(target: Path, out_dir: Path | None) -> Path:
    if target.suffix != ".py":
        return target
    root = out_dir or Path(tempfile.mkdtemp(prefix="molt-opt-explorer-"))
    return build_symbolized(target, root)


def parse_nm(text: str) -> dict[int, str]:
    """Text-section symbols by address from `nm` output."""
    symbols: dict[int, str] = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 3 and parts[1] in {"t", "T"}:
            try:
                symbols.setdefault(int(parts[0], 16), _plain_symbol(parts[2]))
            except ValueError:
                continue
    return symbols


def parse_fixups(text: str, symbols: dict[int, str]) -> dict[int, str]:
    """GOT slot address to target symbol from `dyld_info -fixups` output."""
    got: dict[int, str] = {}
    for line in text.splitlines():
        parts = line.split()
        if len(parts) != 5 or parts[1] != "__got":
            continue
        try:
            slot = int(parts[2], 16)
        except ValueError:
            continue
        if parts[3] == "bind":
            got[slot] = _plain_symbol(parts[4].rsplit("/", 1)[-1])
        elif parts[3] == "rebase":
            target = int(parts[4], 16)
            got[slot] = symbols.get(target, f"<{target:#x}>")
    return got


def _symbol_table(binary: Path) -> SymbolTable:
    nm = _COMMANDS.check_output(
        ["nm", str(binary)], text=True, encoding="utf-8", timeout=120
    )
    assert isinstance(nm, str)
    symbols = parse_nm(nm)
    got: dict[int, str] = {}
    if Path("/usr/bin/dyld_info").is_file():
        fixups = _COMMANDS.check_output(
            ["/usr/bin/dyld_info", "-fixups", str(binary)],
            text=True,
            encoding="utf-8",
            timeout=120,
        )
        assert isinstance(fixups, str)
        got = parse_fixups(fixups, symbols)
    return SymbolTable(symbols, got)


def _disassemble(binary: Path, symbols: list[str]) -> str:
    args = ["objdump", "-d", "--no-show-raw-insn"]
    if symbols:
        args.append("--disassemble-symbols=" + ",".join(symbols))
    text = _COMMANDS.check_output(
        [*args, str(binary)], text=True, encoding="utf-8", timeout=600
    )
    assert isinstance(text, str)
    return text


def anatomy(binary: Path, module: str, pattern: str | None) -> list[FunctionAnatomy]:
    table = _symbol_table(binary)
    names = sorted(
        name for name in set(table.symbols.values()) if name.startswith(f"{module}__")
    )
    if pattern:
        names = [name for name in names if re.search(pattern, name)]
    if not names:
        raise SystemExit(f"opt_explorer: no compiled `{module}__*` symbols in {binary}")
    prefix = "_" if sys.platform == "darwin" else ""
    arch, functions = parse_disassembly(
        _disassemble(binary, [prefix + name for name in names])
    )
    return [
        analyze_function(arch, name, insns, table)
        for name, insns in functions.items()
        if name in set(names)
    ]


def render_anatomy(binary: Path, functions: list[FunctionAnatomy]) -> str:
    total = sum(f.whole.instructions for f in functions)
    lines = [
        f"{binary.name}: {len(functions)} compiled functions, {total} instructions"
    ]
    for function in sorted(functions, key=lambda f: -f.whole.instructions):
        whole = function.whole
        lines.append(
            f"{function.name}: {whole.instructions} insns, {whole.branches} branches, "
            f"{sum(whole.calls.values())} calls{_calls_text(whole.calls)}"
        )
        for loop in function.loops:
            lines.append(
                f"  loop {loop.start:#x}-{loop.end:#x}: {loop.instructions} insns, "
                f"{loop.branches} branches{_calls_text(loop.calls)}"
            )
    return "\n".join(lines)


def _calls_text(calls: Counter[str]) -> str:
    if not calls:
        return ""
    return " (" + ", ".join(f"{name} {n}" for name, n in calls.most_common()) + ")"


def sample_run(binary: Path, seconds: float, args: list[str]) -> str:
    """Run `binary` once under `/usr/bin/sample`; return the report text."""
    if sys.platform != "darwin" or not Path("/usr/bin/sample").is_file():
        raise SystemExit(
            "opt_explorer profile: needs macOS /usr/bin/sample; "
            "this host has no supported sampler"
        )
    report = Path(tempfile.mkdtemp(prefix="molt-opt-explorer-")) / "sample.txt"
    program = _COMMANDS.start_owned(
        [str(binary), *args], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    try:
        _COMMANDS.run(
            [
                "/usr/bin/sample",
                str(program.pid),
                str(seconds),
                "1",
                "-mayDie",
                "-file",
                str(report),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=seconds + 60,
        )
    finally:
        _COMMANDS.wait_owned(program, timeout=max(seconds, 1.0) + 600)
    return (
        report.read_text(encoding="utf-8", errors="replace") if report.is_file() else ""
    )


def demangle(symbols: list[str]) -> dict[str, str]:
    """Readable names through one batched `c++filt` call (Rust v0 and C++).

    Without `c++filt` the names stay mangled, which is still exact.
    """
    unique = sorted(set(symbols))
    if not unique:
        return {}
    try:
        out = _COMMANDS.check_output(
            ["c++filt"],
            input="\n".join(unique) + "\n",
            text=True,
            encoding="utf-8",
            timeout=60,
        )
    except (OSError, subprocess.CalledProcessError):
        return {symbol: symbol for symbol in unique}
    assert isinstance(out, str)
    names = out.splitlines()
    if len(names) != len(unique):
        return {symbol: symbol for symbol in unique}
    return dict(zip(unique, names, strict=True))


def _short(name: str, width: int = 100) -> str:
    return name if len(name) <= width else "..." + name[-(width - 3) :]


def render_profile(
    binary: Path,
    module: str,
    report: str,
    callers: list[str],
    top_n: int,
) -> tuple[str, dict]:
    frames = sample_profile.call_frames(report)
    leaders = sample_profile.self_time_from_frames(frames)
    total = sum(row["self_samples"] for row in leaders)
    caller_counts = {
        target: sample_profile.direct_callers(frames, target) for target in callers
    }
    names = demangle(
        [row["symbol"] for row in leaders]
        + [name for found in caller_counts.values() for name in found]
    )
    split = cost_split(leaders, binary.name, module, names)
    lines = [f"{binary.name}: {total} self-time samples"]
    for bucket, samples, share in split:
        lines.append(f"  {share:6.1%} {samples:7} {bucket}")
    lines.append("hottest symbols:")
    for row in leaders[:top_n]:
        readable = names[row["symbol"]]
        bucket = classify_symbol(readable, row.get("lib"), binary.name, module)
        lines.append(f"  {row['self_samples']:7} {_short(readable)}  [{bucket}]")
    caller_json = {}
    for target, found in caller_counts.items():
        caller_json[target] = {
            names.get(name, name): samples for name, samples in found.most_common()
        }
        lines.append(f"callers of {target} ({sum(found.values())} samples):")
        for name, samples in found.most_common(8):
            lines.append(f"  {samples:7} {_short(names.get(name, name))}")
    payload = {
        "binary": str(binary),
        "samples": total,
        "buckets": [
            {"bucket": b, "samples": n, "share": round(s, 4)} for b, n, s in split
        ],
        "top": leaders[:top_n],
        "callers": caller_json,
    }
    return "\n".join(lines), payload


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("anatomy", "profile"):
        command = sub.add_parser(name)
        command.add_argument("target", type=Path, help="a .py file or a built binary")
        command.add_argument("--module", help="compiled module name (default: stem)")
        command.add_argument(
            "--out-dir", type=Path, help="build directory for .py input"
        )
        command.add_argument("--json", action="store_true")
    sub.choices["anatomy"].add_argument("--function", help="regex over function names")
    profile = sub.choices["profile"]
    profile.add_argument("--seconds", type=float, default=30.0)
    profile.add_argument("--top", type=int, default=15)
    profile.add_argument("--callers", action="append", default=[])
    profile.add_argument("args", nargs="*", help="program arguments")
    args = parser.parse_args(argv)

    module = args.module or args.target.stem.removesuffix("_molt")
    binary = _resolve_binary(args.target, args.out_dir)
    if args.command == "anatomy":
        functions = anatomy(binary, module, args.function)
        if args.json:
            print(json.dumps([f.to_json() for f in functions], indent=2))
        else:
            print(render_anatomy(binary, functions))
        return 0
    report = sample_run(binary, args.seconds, args.args)
    text, payload = render_profile(binary, module, report, args.callers, args.top)
    print(json.dumps(payload, indent=2) if args.json else text)
    return 0


if __name__ == "__main__":
    sys.exit(main())
