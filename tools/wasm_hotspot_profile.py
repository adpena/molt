#!/usr/bin/env python3
"""WASM hotspot profiler for Molt (MOL-212).

Compiles Python programs to WASM via the Molt pipeline, performs static
analysis of per-function code sizes from the WASM binary, and (when WASM
execution is available) profiles execution with Node.js ``--cpu-prof``
to identify per-function runtime hotspots.

Ranks functions by code size and, when profile data is available, by
cumulative sample time.  Identifies p95 offenders and cross-references
size with execution time to flag "big and slow" functions for best
optimization ROI.

Usage::

    python tools/wasm_hotspot_profile.py examples/hello.py
    python tools/wasm_hotspot_profile.py examples/hello.py --verbose
    python tools/wasm_hotspot_profile.py --suite
    python tools/wasm_hotspot_profile.py --suite --out bench/wasm_hotspot_baseline.json
    python tools/wasm_hotspot_profile.py examples/hello.py --json
"""

from __future__ import annotations

import argparse
import glob as globmod
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

# ---------------------------------------------------------------------------
# Project layout
# ---------------------------------------------------------------------------

MOLT_ROOT = Path(__file__).resolve().parent.parent
TOOLS_DIR = MOLT_ROOT / "tools"
RUN_WASM_JS = MOLT_ROOT / "wasm/run_wasm.js"

# Make tools/ importable
sys.path.insert(0, str(TOOLS_DIR))

# Programs used for baseline profiling (subset of bench/wasm_bench.py list).
DEFAULT_PROGRAMS: list[str] = [
    "examples/hello.py",
    "examples/simple_ret.py",
    "tests/benchmarks/bench_fib.py",
    "tests/benchmarks/bench_sum.py",
    "tests/benchmarks/bench_deeply_nested_loop.py",
    "tests/benchmarks/bench_matrix_math.py",
    "tests/benchmarks/bench_str_find.py",
    "tests/benchmarks/bench_str_count.py",
    "tests/benchmarks/bench_bytes_find.py",
    "tests/benchmarks/bench_struct.py",
]

# Inline programs used when benchmark files are not available.
INLINE_PROGRAMS: dict[str, str] = {
    "fib": """\
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)

print(fib(20))
""",
    "sum_range": """\
total = 0
for i in range(1000):
    total += i
print(total)
""",
    "nested_loop": """\
total = 0
for i in range(50):
    for j in range(50):
        total += i * j
print(total)
""",
    "hello": 'print("hello world")\n',
}

# WASM binary constants
WASM_MAGIC = b"\x00asm"
SECTION_CODE = 10
SECTION_CUSTOM = 0


# ---------------------------------------------------------------------------
# Node.js resolution
# ---------------------------------------------------------------------------

_NODE_BIN_CACHE: str | None = None


def _resolve_node() -> str | None:
    """Find a Node.js binary >= 18.  Returns None if not found."""
    global _NODE_BIN_CACHE
    if _NODE_BIN_CACHE is not None:
        return _NODE_BIN_CACHE

    env_node = os.environ.get("MOLT_NODE_BIN")
    if env_node and shutil.which(env_node):
        _NODE_BIN_CACHE = env_node
        return env_node

    node = shutil.which("node")
    if node:
        _NODE_BIN_CACHE = node
        return node

    return None


# ---------------------------------------------------------------------------
# WASM binary parsing helpers
# ---------------------------------------------------------------------------


def _read_leb128_u32(data: bytes, offset: int) -> tuple[int, int]:
    """Read an unsigned LEB128 value. Returns (value, new_offset)."""
    result = 0
    shift = 0
    while offset < len(data):
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if (byte & 0x80) == 0:
            break
        shift += 7
    return result, offset


def _read_wasm_string(data: bytes, offset: int) -> tuple[str, int]:
    """Read a length-prefixed UTF-8 string. Returns (string, new_offset)."""
    length, offset = _read_leb128_u32(data, offset)
    s = data[offset : offset + length].decode("utf-8", errors="replace")
    return s, offset + length


def parse_wasm_sections(wasm_path: Path) -> list[tuple[int, int, int]]:
    """Parse WASM section headers. Returns list of (sec_id, data_start, size)."""
    data = wasm_path.read_bytes()
    if len(data) < 8 or data[:4] != WASM_MAGIC:
        return []

    sections: list[tuple[int, int, int]] = []
    scan = 8  # skip magic + version
    while scan < len(data):
        sec_id = data[scan]
        scan += 1
        sec_size, scan = _read_leb128_u32(data, scan)
        sections.append((sec_id, scan, sec_size))
        scan += sec_size
    return sections


def parse_function_sizes(wasm_path: Path) -> list[dict[str, Any]]:
    """Parse the WASM code section to extract per-function body sizes.

    Returns a list of dicts with keys: index, offset, body_size_bytes.
    If a "name" custom section exists, function names are also included.
    """
    data = wasm_path.read_bytes()
    if len(data) < 8 or data[:4] != WASM_MAGIC:
        return []

    sections = parse_wasm_sections(wasm_path)
    code_functions: list[dict[str, Any]] = []
    name_map: dict[int, str] = {}

    # Parse sections from raw data (sections reference the full data)
    raw_sections: list[tuple[int, int, int]] = []
    scan = 8
    while scan < len(data):
        sec_id = data[scan]
        scan += 1
        sec_size, scan = _read_leb128_u32(data, scan)
        raw_sections.append((sec_id, scan, sec_size))
        scan += sec_size

    # Parse name custom section (if present)
    for sec_id, sec_start, sec_size in raw_sections:
        if sec_id == SECTION_CUSTOM and sec_size > 0:
            try:
                name, name_end = _read_wasm_string(data, sec_start)
                if name == "name":
                    _parse_name_section(data, name_end, sec_start + sec_size, name_map)
            except (IndexError, UnicodeDecodeError):
                pass

    # Count imported functions
    import_func_count = _count_imported_functions(data, raw_sections)

    # Parse code section
    for sec_id, sec_start, sec_size in raw_sections:
        if sec_id == SECTION_CODE:
            pos = sec_start
            func_count, pos = _read_leb128_u32(data, pos)
            for i in range(func_count):
                body_size, pos = _read_leb128_u32(data, pos)
                func_idx = import_func_count + i
                entry: dict[str, Any] = {
                    "index": func_idx,
                    "offset": pos,
                    "body_size_bytes": body_size,
                }
                if func_idx in name_map:
                    entry["name"] = name_map[func_idx]
                code_functions.append(entry)
                pos += body_size
            break

    return code_functions


def _count_imported_functions(
    data: bytes,
    sections: list[tuple[int, int, int]],
) -> int:
    """Count imported functions (section 2) to offset code-section indices."""
    SECTION_IMPORT = 2
    count = 0
    for sec_id, sec_start, sec_size in sections:
        if sec_id == SECTION_IMPORT:
            pos = sec_start
            num_imports, pos = _read_leb128_u32(data, pos)
            for _ in range(num_imports):
                _mod_name, pos = _read_wasm_string(data, pos)
                _field_name, pos = _read_wasm_string(data, pos)
                kind = data[pos]
                pos += 1
                if kind == 0x00:  # function
                    count += 1
                    _type_idx, pos = _read_leb128_u32(data, pos)
                elif kind == 0x01:  # table
                    pos += 1  # elem type
                    _flags = data[pos]
                    pos += 1
                    _initial, pos = _read_leb128_u32(data, pos)
                    if _flags & 0x01:
                        _max, pos = _read_leb128_u32(data, pos)
                elif kind == 0x02:  # memory
                    _flags = data[pos]
                    pos += 1
                    _initial, pos = _read_leb128_u32(data, pos)
                    if _flags & 0x01:
                        _max, pos = _read_leb128_u32(data, pos)
                elif kind == 0x03:  # global
                    pos += 1  # value type
                    pos += 1  # mutability
                else:
                    break
            break
    return count


def _parse_name_section(
    data: bytes,
    start: int,
    end: int,
    name_map: dict[int, str],
) -> None:
    """Parse the 'name' custom section to build a function index -> name map."""
    pos = start
    while pos < end:
        try:
            subsection_id = data[pos]
            pos += 1
            subsection_size, pos = _read_leb128_u32(data, pos)
            subsection_end = pos + subsection_size

            if subsection_id == 1:  # function names
                count, pos = _read_leb128_u32(data, pos)
                for _ in range(count):
                    if pos >= subsection_end:
                        break
                    func_idx, pos = _read_leb128_u32(data, pos)
                    func_name, pos = _read_wasm_string(data, pos)
                    name_map[func_idx] = func_name
            pos = subsection_end
        except (IndexError, UnicodeDecodeError):
            break


def parse_section_sizes(wasm_path: Path) -> dict[str, int]:
    """Quick section-level size breakdown (reuses wasm_size_audit logic)."""
    SECTION_NAMES: dict[int, str] = {
        0: "custom", 1: "type", 2: "import", 3: "function", 4: "table",
        5: "memory", 6: "global", 7: "export", 8: "start", 9: "element",
        10: "code", 11: "data", 12: "data_count",
    }
    data = wasm_path.read_bytes()
    if len(data) < 8 or data[:4] != WASM_MAGIC:
        return {}
    by_type: dict[str, int] = {}
    scan = 8
    while scan < len(data):
        sec_id = data[scan]
        scan += 1
        sec_size, scan = _read_leb128_u32(data, scan)
        name = SECTION_NAMES.get(sec_id, f"unknown({sec_id})")
        by_type[name] = by_type.get(name, 0) + sec_size
        scan += sec_size
    return by_type


# ---------------------------------------------------------------------------
# V8 CPU profile parsing
# ---------------------------------------------------------------------------


@dataclass
class FunctionSample:
    """Aggregated sample data for one function."""
    name: str
    func_index: int | None = None
    total_samples: int = 0
    self_samples: int = 0
    body_size_bytes: int = 0

    def to_dict(self, total_sample_count: int) -> dict[str, Any]:
        total_pct = (
            (self.total_samples / total_sample_count * 100)
            if total_sample_count > 0
            else 0.0
        )
        self_pct = (
            (self.self_samples / total_sample_count * 100)
            if total_sample_count > 0
            else 0.0
        )
        d: dict[str, Any] = {
            "name": self.name,
            "total_samples": self.total_samples,
            "total_pct": round(total_pct, 2),
            "self_samples": self.self_samples,
            "self_pct": round(self_pct, 2),
            "body_size_bytes": self.body_size_bytes,
        }
        if self.func_index is not None:
            d["func_index"] = self.func_index
        return d


def parse_v8_cpuprofile(profile_path: Path) -> dict[str, FunctionSample]:
    """Parse a V8 CPU profile JSON (from Node --cpu-prof) into per-function samples.

    The V8 CPU profile format has:
    - nodes: array of {id, callFrame: {functionName, ...}, children: [...]}
    - samples: array of node IDs (leaf of call stack at each sample tick)
    - timeDeltas: array of time deltas between samples
    """
    raw = json.loads(profile_path.read_text())
    functions: dict[str, FunctionSample] = {}

    nodes = raw.get("nodes", [])
    samples = raw.get("samples", [])

    if not nodes or not samples:
        return functions

    node_by_id: dict[int, dict] = {}
    parent_of: dict[int, int | None] = {}
    for node in nodes:
        nid = node["id"]
        node_by_id[nid] = node
        for child_id in node.get("children", []):
            parent_of[child_id] = nid

    def _node_name(node: dict) -> str:
        cf = node.get("callFrame", {})
        name = cf.get("functionName", "")
        if not name:
            name = "(anonymous)"
        return name

    def _walk_to_root(node_id: int) -> list[int]:
        path: list[int] = []
        visited: set[int] = set()
        current: int | None = node_id
        while current is not None and current not in visited:
            visited.add(current)
            path.append(current)
            current = parent_of.get(current)
        return path

    for sample_node_id in samples:
        if sample_node_id not in node_by_id:
            continue

        leaf_node = node_by_id[sample_node_id]
        leaf_name = _node_name(leaf_node)
        if leaf_name not in functions:
            functions[leaf_name] = FunctionSample(name=leaf_name)
        functions[leaf_name].self_samples += 1

        path = _walk_to_root(sample_node_id)
        seen_names: set[str] = set()
        for nid in path:
            node = node_by_id.get(nid)
            if node is None:
                continue
            fname = _node_name(node)
            if fname in seen_names:
                continue
            seen_names.add(fname)
            if fname not in functions:
                functions[fname] = FunctionSample(name=fname)
            functions[fname].total_samples += 1

    return functions


# ---------------------------------------------------------------------------
# Compilation and profiling
# ---------------------------------------------------------------------------


def _compile_wasm(src: Path, out_dir: Path) -> tuple[bool, Path, str, float]:
    """Compile a Python file to WASM. Returns (ok, wasm_path, error, elapsed_s)."""
    out_dir.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    env["MOLT_WASM_LINKED"] = "0"
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_MIDEND_DISABLE", "1")
    # Determinism/bound env from bench_wasm.py
    env.setdefault("MOLT_SCCP_MAX_ITERS", "8")
    env.setdefault("MOLT_CSE_MAX_ITERS", "8")
    env.setdefault("MOLT_MIDEND_MAX_ROUNDS", "3")
    env.setdefault("MOLT_MIDEND_FAIL_OPEN", "1")
    # Ensure molt is importable
    pythonpath = str(MOLT_ROOT / "src")
    existing = env.get("PYTHONPATH", "")
    if pythonpath not in existing:
        env["PYTHONPATH"] = f"{pythonpath}:{existing}" if existing else pythonpath

    uv = shutil.which("uv")
    if uv:
        python_cmd = [uv, "run", "python", "-m", "molt.cli"]
    else:
        python_cmd = [sys.executable, "-m", "molt.cli"]

    t0 = time.monotonic()
    try:
        r = subprocess.run(
            python_cmd + [
                "build",
                str(src),
                "--target",
                "wasm",
                "--emit",
                "wasm",
                "--out-dir",
                str(out_dir),
            ],
            cwd=MOLT_ROOT,
            capture_output=True,
            text=True,
            env=env,
            timeout=180,
        )
    except subprocess.TimeoutExpired:
        return False, out_dir / "output.wasm", "compile timeout (180s)", time.monotonic() - t0

    elapsed = time.monotonic() - t0
    wasm = out_dir / "output.wasm"
    if r.returncode != 0 or not wasm.exists():
        return False, wasm, (r.stderr or r.stdout)[:500], elapsed
    return True, wasm, "", elapsed


def _try_profile_wasm_node(
    wasm_path: Path,
    profile_dir: Path,
    *,
    timeout_s: int = 30,
    sample_interval_us: int = 1000,
) -> tuple[bool, Path | None, str]:
    """Attempt to run WASM under Node.js with --cpu-prof via wasm/run_wasm.js.

    Returns (ok, profile_path, error_message).
    This is best-effort: if the WASM runtime is broken, it returns ok=False
    but the caller can still report static analysis.
    """
    node = _resolve_node()
    if node is None:
        return False, None, "node not found in PATH"

    if not RUN_WASM_JS.exists():
        return False, None, f"wasm/run_wasm.js not found at {RUN_WASM_JS}"

    profile_dir.mkdir(parents=True, exist_ok=True)

    env = os.environ.copy()
    env["MOLT_WASM_PATH"] = str(wasm_path)
    env["MOLT_WASM_PREFER_LINKED"] = "0"
    env["NODE_NO_WARNINGS"] = "1"
    # Set runtime WASM path
    runtime_wasm = MOLT_ROOT / "wasm" / "molt_runtime.wasm"
    if runtime_wasm.exists():
        env["MOLT_RUNTIME_WASM"] = str(runtime_wasm)
    env.pop("PYTHONPATH", None)
    env.pop("PYTHONHASHSEED", None)

    cmd = [
        node,
        "--no-warnings",
        "--cpu-prof",
        f"--cpu-prof-dir={profile_dir}",
        f"--cpu-prof-interval={sample_interval_us}",
        str(RUN_WASM_JS),
    ]

    r = None
    try:
        r = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            env=env,
            cwd=MOLT_ROOT,
            timeout=timeout_s + 10,
        )
    except subprocess.TimeoutExpired:
        pass  # Profile may still have been written

    # Find the .cpuprofile file Node wrote
    profiles = sorted(globmod.glob(str(profile_dir / "*.cpuprofile")))
    if not profiles:
        err = ""
        if r is not None and r.returncode != 0:
            err = (r.stderr or r.stdout).strip()[:300]
        return False, None, f"No .cpuprofile written. {err}"

    profile_path = Path(profiles[-1])
    if profile_path.stat().st_size == 0:
        return False, None, "Empty .cpuprofile file"

    # Check if Node exited cleanly (WASM ran to completion)
    ran_ok = r is not None and r.returncode == 0
    return True, profile_path, "" if ran_ok else "WASM execution failed but profile captured"


# ---------------------------------------------------------------------------
# Static analysis: size-based hotspot identification
# ---------------------------------------------------------------------------


@dataclass
class FunctionSizeEntry:
    """A function's code size from static WASM analysis."""
    index: int
    name: str
    body_size_bytes: int
    pct_of_code: float = 0.0

    def to_dict(self) -> dict[str, Any]:
        return {
            "index": self.index,
            "name": self.name,
            "body_size_bytes": self.body_size_bytes,
            "body_size_kb": round(self.body_size_bytes / 1024, 1),
            "pct_of_code": round(self.pct_of_code, 2),
        }


def static_size_analysis(func_sizes: list[dict[str, Any]]) -> dict[str, Any]:
    """Analyze function sizes from WASM binary for size-based hotspot detection."""
    total_code = sum(f["body_size_bytes"] for f in func_sizes)
    if total_code == 0:
        return {"functions": [], "p95_by_size": [], "size_distribution": {}}

    entries: list[FunctionSizeEntry] = []
    for f in func_sizes:
        pct = f["body_size_bytes"] / total_code * 100 if total_code > 0 else 0
        entries.append(FunctionSizeEntry(
            index=f["index"],
            name=f.get("name", f"func_{f['index']}"),
            body_size_bytes=f["body_size_bytes"],
            pct_of_code=pct,
        ))

    # Sort by size descending
    entries.sort(key=lambda e: e.body_size_bytes, reverse=True)

    # p95 by size: functions that together account for 95% of total code
    p95_entries: list[FunctionSizeEntry] = []
    accumulated = 0
    for e in entries:
        if accumulated >= total_code * 0.95:
            break
        p95_entries.append(e)
        accumulated += e.body_size_bytes

    # Size distribution buckets
    buckets = {"<1KB": 0, "1-10KB": 0, "10-100KB": 0, ">100KB": 0}
    for e in entries:
        sz = e.body_size_bytes
        if sz < 1024:
            buckets["<1KB"] += 1
        elif sz < 10240:
            buckets["1-10KB"] += 1
        elif sz < 102400:
            buckets["10-100KB"] += 1
        else:
            buckets[">100KB"] += 1

    return {
        "total_code_bytes": total_code,
        "total_code_kb": round(total_code / 1024, 1),
        "function_count": len(entries),
        "functions_top20": [e.to_dict() for e in entries[:20]],
        "p95_by_size": [e.to_dict() for e in p95_entries],
        "p95_function_count": len(p95_entries),
        "size_distribution": buckets,
    }


# ---------------------------------------------------------------------------
# Analysis
# ---------------------------------------------------------------------------


@dataclass
class HotspotResult:
    """Complete hotspot analysis for one program."""
    program: str
    ok: bool
    error: str = ""
    compile_s: float = 0.0
    wasm_size_bytes: int = 0
    wasm_function_count: int = 0
    section_sizes: dict[str, int] = field(default_factory=dict)
    # Static analysis (always available)
    static_analysis: dict[str, Any] = field(default_factory=dict)
    # Dynamic profiling (available when WASM execution works)
    has_profile: bool = False
    profile_note: str = ""
    total_samples: int = 0
    profiled_functions: list[dict[str, Any]] = field(default_factory=list)
    p95_offenders: list[dict[str, Any]] = field(default_factory=list)
    big_and_slow: list[dict[str, Any]] = field(default_factory=list)

    def to_dict(self) -> dict[str, Any]:
        d: dict[str, Any] = {
            "program": self.program,
            "ok": self.ok,
        }
        if self.error:
            d["error"] = self.error
        if self.ok:
            d["compile_s"] = round(self.compile_s, 3)
            d["wasm_size_bytes"] = self.wasm_size_bytes
            d["wasm_size_kb"] = round(self.wasm_size_bytes / 1024, 1)
            d["wasm_function_count"] = self.wasm_function_count
            d["section_sizes"] = self.section_sizes
            d["static_analysis"] = self.static_analysis
            d["has_profile"] = self.has_profile
            if self.profile_note:
                d["profile_note"] = self.profile_note
            if self.has_profile:
                d["total_samples"] = self.total_samples
                d["profiled_functions_top20"] = self.profiled_functions[:20]
                d["p95_offenders"] = self.p95_offenders
                d["big_and_slow"] = self.big_and_slow
        return d


def analyze_hotspots(
    src: Path,
    *,
    timeout_s: int = 30,
    sample_interval_us: int = 1000,
    verbose: bool = False,
    skip_profile: bool = False,
) -> HotspotResult:
    """Compile, analyze, and (optionally) profile a single program."""
    program_name = src.stem

    with tempfile.TemporaryDirectory(prefix=f"molt-hotspot-{program_name}-") as tmpdir:
        work = Path(tmpdir)

        # Step 1: Compile
        if verbose:
            print(f"    Compiling {src.name}...", end=" ", flush=True)
        ok, wasm_path, err, compile_s = _compile_wasm(src, work / "build")
        if not ok:
            if verbose:
                print("FAIL")
            return HotspotResult(program=program_name, ok=False, error=err)
        wasm_size = wasm_path.stat().st_size
        if verbose:
            print(f"{wasm_size / 1024:.1f}KB in {compile_s:.1f}s", flush=True)

        # Step 2: Parse function sizes from WASM binary
        if verbose:
            print("    Parsing WASM functions...", end=" ", flush=True)
        func_sizes = parse_function_sizes(wasm_path)
        section_sizes = parse_section_sizes(wasm_path)
        if verbose:
            print(f"{len(func_sizes)} functions", flush=True)

        # Build lookup maps
        size_by_name: dict[str, int] = {}
        size_by_index: dict[int, int] = {}
        for fs in func_sizes:
            size_by_index[fs["index"]] = fs["body_size_bytes"]
            if "name" in fs:
                size_by_name[fs["name"]] = fs["body_size_bytes"]

        # Step 3: Static analysis (always available)
        if verbose:
            print("    Running static size analysis...", end=" ", flush=True)
        static = static_size_analysis(func_sizes)
        if verbose:
            print(f"{static.get('p95_function_count', 0)} p95-by-size functions", flush=True)

        # Step 4: Attempt dynamic profiling
        has_profile = False
        profile_note = ""
        total_samples = 0
        profiled_functions: list[dict[str, Any]] = []
        p95_offenders: list[dict[str, Any]] = []
        big_and_slow: list[dict[str, Any]] = []

        if not skip_profile:
            if verbose:
                print("    Profiling with Node.js --cpu-prof...", end=" ", flush=True)
            profile_dir = work / "profiles"
            prof_ok, profile_path, prof_err = _try_profile_wasm_node(
                wasm_path,
                profile_dir,
                timeout_s=timeout_s,
                sample_interval_us=sample_interval_us,
            )
            if prof_ok and profile_path is not None:
                if verbose:
                    profile_size = profile_path.stat().st_size
                    print(f"{profile_size / 1024:.1f}KB profile", flush=True)

                func_samples = parse_v8_cpuprofile(profile_path)

                # Merge size data
                for fname, fsample in func_samples.items():
                    if fname in size_by_name:
                        fsample.body_size_bytes = size_by_name[fname]
                    if "wasm-function[" in fname:
                        try:
                            idx = int(fname.split("[")[1].rstrip("]"))
                            fsample.func_index = idx
                            if idx in size_by_index:
                                fsample.body_size_bytes = size_by_index[idx]
                        except (ValueError, IndexError):
                            pass
                    for known_name, known_size in size_by_name.items():
                        if known_name in fname and fsample.body_size_bytes == 0:
                            fsample.body_size_bytes = known_size
                            break

                total_samples = sum(f.self_samples for f in func_samples.values())
                if total_samples > 0:
                    has_profile = True
                    ranked = sorted(
                        func_samples.values(),
                        key=lambda f: f.total_samples,
                        reverse=True,
                    )
                    profiled_functions = [f.to_dict(total_samples) for f in ranked]
                    p95_offenders = _compute_p95_offenders(ranked, total_samples)
                    big_and_slow = _compute_big_and_slow(ranked, total_samples)
                    if prof_err:
                        profile_note = prof_err
                else:
                    profile_note = "Profile captured but 0 samples (program too short)"
            else:
                if verbose:
                    print(f"SKIP ({prof_err[:60]})", flush=True)
                profile_note = f"Profiling unavailable: {prof_err}"
        else:
            profile_note = "Profiling skipped (--no-profile)"

        if verbose and has_profile:
            print(
                f"    {total_samples} samples, "
                f"{len(p95_offenders)} p95 offenders, "
                f"{len(big_and_slow)} big-and-slow",
                flush=True,
            )

        return HotspotResult(
            program=program_name,
            ok=True,
            compile_s=compile_s,
            wasm_size_bytes=wasm_size,
            wasm_function_count=len(func_sizes),
            section_sizes=section_sizes,
            static_analysis=static,
            has_profile=has_profile,
            profile_note=profile_note,
            total_samples=total_samples,
            profiled_functions=profiled_functions,
            p95_offenders=p95_offenders,
            big_and_slow=big_and_slow,
        )


def _compute_p95_offenders(
    ranked: list[FunctionSample],
    total_samples: int,
) -> list[dict[str, Any]]:
    """Find functions whose self-time puts them in the p95 tail."""
    if total_samples == 0:
        return []

    by_self = sorted(ranked, key=lambda f: f.self_samples, reverse=True)
    threshold = total_samples * 0.95
    accumulated = 0
    offenders: list[dict[str, Any]] = []

    for f in by_self:
        if accumulated >= threshold:
            break
        offenders.append(f.to_dict(total_samples))
        accumulated += f.self_samples

    return offenders


def _compute_big_and_slow(
    ranked: list[FunctionSample],
    total_samples: int,
) -> list[dict[str, Any]]:
    """Find functions that are both large (code size) and hot (samples)."""
    if total_samples == 0:
        return []

    candidates = [f for f in ranked if f.self_samples > 0 and f.body_size_bytes > 0]
    if not candidates:
        return []

    sizes = sorted(f.body_size_bytes for f in candidates)
    self_counts = sorted(f.self_samples for f in candidates)

    size_p75 = sizes[int(len(sizes) * 0.75)] if sizes else 0
    sample_median = self_counts[int(len(self_counts) * 0.5)] if self_counts else 0

    result: list[dict[str, Any]] = []
    for f in candidates:
        if f.body_size_bytes >= size_p75 and f.self_samples >= sample_median:
            d = f.to_dict(total_samples)
            d["body_size_kb"] = round(f.body_size_bytes / 1024, 1)
            d["roi_score"] = round(
                (f.self_samples / total_samples) * (f.body_size_bytes / 1024), 2
            )
            result.append(d)

    result.sort(key=lambda d: d["roi_score"], reverse=True)
    return result


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def print_summary(result: HotspotResult) -> None:
    """Print a human-readable hotspot summary."""
    print(f"\n{'=' * 72}")
    print(f"  WASM Hotspot Profile: {result.program}")
    print(f"{'=' * 72}")

    if not result.ok:
        print(f"  FAILED: {result.error}")
        return

    print(f"  WASM size: {result.wasm_size_bytes / 1024:.1f} KB")
    print(f"  Compile time: {result.compile_s:.2f}s")
    print(f"  WASM functions: {result.wasm_function_count}")

    # Section breakdown
    if result.section_sizes:
        code_kb = result.section_sizes.get("code", 0) / 1024
        data_kb = result.section_sizes.get("data", 0) / 1024
        print(f"  Code section: {code_kb:.1f} KB | Data section: {data_kb:.1f} KB")

    # Static analysis: top functions by size
    static = result.static_analysis
    if static:
        top_funcs = static.get("functions_top20", [])
        p95_count = static.get("p95_function_count", 0)
        total_funcs = static.get("function_count", 0)
        print("\n  --- Static Size Analysis ---")
        print(f"  Total code: {static.get('total_code_kb', 0)} KB across {total_funcs} functions")
        print(f"  p95 by size: {p95_count} functions cover 95% of code section")

        dist = static.get("size_distribution", {})
        if dist:
            parts = [f"{k}: {v}" for k, v in dist.items()]
            print(f"  Distribution: {', '.join(parts)}")

        print(f"\n  {'Rank':<6s} {'Size':>8s} {'%Code':>7s} {'Function'}")
        print(f"  {'-' * 6} {'-' * 8} {'-' * 7} {'-' * 40}")
        for i, f in enumerate(top_funcs[:15], 1):
            print(
                f"  {i:<6d} {f['body_size_kb']:>6.1f}KB {f['pct_of_code']:>6.1f}% "
                f"{f['name'][:50]}"
            )

    # Dynamic profiling results
    if result.has_profile:
        print("\n  --- Dynamic Profile (Node.js --cpu-prof) ---")
        print(f"  Total samples: {result.total_samples}")
        if result.profile_note:
            print(f"  Note: {result.profile_note}")

        print(f"\n  {'Rank':<6s} {'Total%':>7s} {'Self%':>7s} {'Size':>8s} {'Function'}")
        print(f"  {'-' * 6} {'-' * 7} {'-' * 7} {'-' * 8} {'-' * 40}")
        for i, f in enumerate(result.profiled_functions[:15], 1):
            size_str = (
                f"{f['body_size_bytes'] / 1024:.1f}KB"
                if f.get("body_size_bytes", 0) > 0
                else "-"
            )
            print(
                f"  {i:<6d} {f['total_pct']:>6.1f}% {f['self_pct']:>6.1f}% "
                f"{size_str:>8s} {f['name'][:60]}"
            )

        if result.p95_offenders:
            print(f"\n  --- p95 Offenders ({len(result.p95_offenders)} functions) ---")
            for f in result.p95_offenders[:10]:
                print(f"    {f['self_pct']:>5.1f}%  {f['name'][:60]}")

        if result.big_and_slow:
            print("\n  --- Big & Slow (best optimization ROI) ---")
            print(f"  {'ROI':>6s} {'Self%':>7s} {'Size':>8s} {'Function'}")
            print(f"  {'-' * 6} {'-' * 7} {'-' * 8} {'-' * 40}")
            for f in result.big_and_slow[:10]:
                print(
                    f"  {f['roi_score']:>6.2f} {f['self_pct']:>6.1f}% "
                    f"{f['body_size_kb']:>6.1f}KB {f['name'][:50]}"
                )
    else:
        print("\n  --- Dynamic Profile: unavailable ---")
        if result.profile_note:
            print(f"  {result.profile_note}")

    print()


def build_baseline_report(results: list[HotspotResult]) -> dict[str, Any]:
    """Build a JSON baseline report from multiple program profiles."""
    git_rev = ""
    try:
        r = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            capture_output=True,
            text=True,
            cwd=MOLT_ROOT,
        )
        if r.returncode == 0:
            git_rev = r.stdout.strip()
    except OSError:
        pass

    node_ver = "unknown"
    node = _resolve_node()
    if node:
        try:
            r = subprocess.run(
                [node, "--version"], capture_output=True, text=True, timeout=5
            )
            if r.returncode == 0:
                node_ver = r.stdout.strip()
        except Exception:
            pass

    report: dict[str, Any] = {
        "schema_version": 1,
        "tool": "wasm_hotspot_profile",
        "task": "MOL-212",
        "created_at": time.strftime("%Y-%m-%dT%H:%M:%S+00:00", time.gmtime()),
        "git_rev": git_rev,
        "system": {
            "platform": platform.platform(),
            "machine": platform.machine(),
            "python": platform.python_version(),
            "node": node_ver,
        },
        "profiles": {},
        "summary": {},
    }

    ok_count = 0
    fail_count = 0
    profile_count = 0

    # Aggregate biggest functions across all programs (static analysis)
    all_large_funcs: list[dict[str, Any]] = []

    for result in results:
        report["profiles"][result.program] = result.to_dict()
        if result.ok:
            ok_count += 1
            if result.has_profile:
                profile_count += 1
            # Collect top functions from static analysis
            static = result.static_analysis
            for f in static.get("functions_top20", [])[:10]:
                f_copy = dict(f)
                f_copy["program"] = result.program
                all_large_funcs.append(f_copy)
        else:
            fail_count += 1

    # Sort aggregate by size
    all_large_funcs.sort(key=lambda f: f.get("body_size_bytes", 0), reverse=True)

    report["summary"] = {
        "programs_ok": ok_count,
        "programs_failed": fail_count,
        "programs_with_profile": profile_count,
        "aggregate_largest_functions": all_large_funcs[:30],
    }

    return report


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="WASM hotspot profiler for Molt (MOL-212)"
    )
    parser.add_argument(
        "source",
        nargs="?",
        type=Path,
        help="Python source file to profile",
    )
    parser.add_argument(
        "--suite",
        action="store_true",
        help="Run against the default benchmark suite",
    )
    parser.add_argument(
        "--out",
        type=Path,
        default=None,
        help="Output JSON path (default: bench/wasm_hotspot_baseline.json for --suite)",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Machine-readable JSON output",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=30,
        help="WASM execution timeout in seconds (default: 30)",
    )
    parser.add_argument(
        "--interval",
        type=int,
        default=1000,
        help="Profiling sample interval in microseconds (default: 1000)",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Verbose progress output",
    )
    parser.add_argument(
        "--no-profile",
        action="store_true",
        help="Skip dynamic profiling (static analysis only, faster)",
    )
    parser.add_argument(
        "--programs",
        nargs="*",
        default=None,
        help="Override the default program list (for --suite mode)",
    )
    args = parser.parse_args()

    if not args.suite and not args.source:
        parser.error("Provide a source file or use --suite")

    if args.suite:
        programs = args.programs if args.programs else DEFAULT_PROGRAMS
        results: list[HotspotResult] = []

        print(f"WASM Hotspot Profiler (MOL-212) -- {len(programs)} programs")
        print(f"  timeout={args.timeout}s, interval={args.interval}us")
        if args.no_profile:
            print("  mode: static analysis only (--no-profile)")
        print()

        for prog_path in programs:
            src = MOLT_ROOT / prog_path
            if not src.exists():
                name = Path(prog_path).stem
                if name in INLINE_PROGRAMS:
                    with tempfile.NamedTemporaryFile(
                        mode="w", suffix=".py", prefix=f"molt_{name}_", delete=False
                    ) as f:
                        f.write(INLINE_PROGRAMS[name])
                        src = Path(f.name)
                else:
                    print(f"  SKIP {prog_path} (not found)")
                    continue

            print(f"  [{prog_path}]")
            result = analyze_hotspots(
                src,
                timeout_s=args.timeout,
                sample_interval_us=args.interval,
                verbose=args.verbose,
                skip_profile=args.no_profile,
            )
            results.append(result)

            if not args.json_output:
                print_summary(result)

        # Build and write baseline report
        report = build_baseline_report(results)
        out_path = args.out or (MOLT_ROOT / "bench" / "wasm_hotspot_baseline.json")
        out_path.parent.mkdir(parents=True, exist_ok=True)
        out_path.write_text(json.dumps(report, indent=2) + "\n")
        print(f"\nBaseline written to {out_path}")

        if args.json_output:
            print(json.dumps(report, indent=2))

        ok_count = sum(1 for r in results if r.ok)
        fail_count = len(results) - ok_count
        prof_count = sum(1 for r in results if r.has_profile)
        print(
            f"Results: {ok_count} OK, {fail_count} FAIL, "
            f"{prof_count} with dynamic profile out of {len(results)} programs"
        )

    else:
        assert args.source is not None
        if not args.source.is_file():
            print(f"ERROR: {args.source} not found", file=sys.stderr)
            sys.exit(1)

        result = analyze_hotspots(
            args.source,
            timeout_s=args.timeout,
            sample_interval_us=args.interval,
            verbose=args.verbose,
            skip_profile=args.no_profile,
        )

        if args.json_output:
            print(json.dumps(result.to_dict(), indent=2))
        else:
            print_summary(result)

        if args.out:
            report = build_baseline_report([result])
            args.out.parent.mkdir(parents=True, exist_ok=True)
            args.out.write_text(json.dumps(report, indent=2) + "\n")
            print(f"Report written to {args.out}")

        sys.exit(0 if result.ok else 1)


if __name__ == "__main__":
    main()
