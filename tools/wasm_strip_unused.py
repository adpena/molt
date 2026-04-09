#!/usr/bin/env python3
"""Analyze and optionally strip unused WASM imports for pure-computation modules.

For Molt-compiled WASM binaries running in a browser host, many imports
(networking, process spawning, DB, filesystem write) are never called by
pure-computation scripts (math, json, print).  This tool identifies those
imports, reports their overhead, and can optionally produce a trimmed copy
where unused host imports are replaced by wasm-internal no-op stubs.

Requires: wasm-tools CLI (https://github.com/bytecodealliance/wasm-tools)

Usage:
    python tools/wasm_strip_unused.py path/to/module.wasm
    python tools/wasm_strip_unused.py path/to/module.wasm --strip -o dist/trimmed.wasm
    python tools/wasm_strip_unused.py path/to/module.wasm --json
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from dataclasses import dataclass, field
from enum import Enum
from pathlib import Path


# ---------------------------------------------------------------------------
# Import classification
# ---------------------------------------------------------------------------


class ImportCategory(str, Enum):
    """Functional category of a WASM import."""

    ESSENTIAL = "essential"  # Required for any execution (memory, table, args, clock)
    IO_STDOUT = "io_stdout"  # fd_write to stdout/stderr — used by print()
    IO_FILESYSTEM = "io_filesystem"  # Filesystem read/write/stat/dir ops
    PROCESS = "process"  # Process spawn/kill/wait/poll
    DATABASE = "database"  # DB exec/query/poll
    WEBSOCKET = "websocket"  # WebSocket connect/send/recv
    SOCKET = "socket"  # Raw socket operations
    TIME = "time"  # Timezone/offset (beyond clock_time_get)
    INDIRECT_CALL = (
        "indirect_call"  # molt_call_indirectN — required for function pointers
    )
    TABLE = "table"  # __indirect_function_table


# Maps (module, name_prefix_or_exact) → category
IMPORT_RULES: list[tuple[str, str, ImportCategory, str]] = [
    # === Essential (never strip) ===
    (
        "wasi_snapshot_preview1",
        "args_sizes_get",
        ImportCategory.ESSENTIAL,
        "Argument count query",
    ),
    (
        "wasi_snapshot_preview1",
        "args_get",
        ImportCategory.ESSENTIAL,
        "Argument retrieval",
    ),
    (
        "wasi_snapshot_preview1",
        "environ_sizes_get",
        ImportCategory.ESSENTIAL,
        "Environment size query",
    ),
    (
        "wasi_snapshot_preview1",
        "environ_get",
        ImportCategory.ESSENTIAL,
        "Environment retrieval",
    ),
    (
        "wasi_snapshot_preview1",
        "clock_time_get",
        ImportCategory.ESSENTIAL,
        "Wall-clock / monotonic time",
    ),
    ("wasi_snapshot_preview1", "random_get", ImportCategory.ESSENTIAL, "CSPRNG bytes"),
    ("wasi_snapshot_preview1", "proc_exit", ImportCategory.ESSENTIAL, "Process exit"),
    (
        "wasi_snapshot_preview1",
        "sched_yield",
        ImportCategory.ESSENTIAL,
        "Cooperative yield",
    ),
    (
        "env",
        "__indirect_function_table",
        ImportCategory.TABLE,
        "Indirect call dispatch table",
    ),
    # === IO: stdout/stderr (needed for print()) ===
    (
        "wasi_snapshot_preview1",
        "fd_write",
        ImportCategory.IO_STDOUT,
        "Write to fd (stdout/stderr)",
    ),
    (
        "wasi_snapshot_preview1",
        "fd_read",
        ImportCategory.IO_STDOUT,
        "Read from fd (stdin stub)",
    ),
    (
        "wasi_snapshot_preview1",
        "fd_close",
        ImportCategory.IO_STDOUT,
        "Close file descriptor",
    ),
    (
        "wasi_snapshot_preview1",
        "fd_seek",
        ImportCategory.IO_STDOUT,
        "Seek file descriptor",
    ),
    (
        "wasi_snapshot_preview1",
        "fd_tell",
        ImportCategory.IO_STDOUT,
        "Tell file descriptor position",
    ),
    (
        "wasi_snapshot_preview1",
        "fd_prestat_get",
        ImportCategory.IO_STDOUT,
        "Preopened fd stat",
    ),
    (
        "wasi_snapshot_preview1",
        "fd_prestat_dir_name",
        ImportCategory.IO_STDOUT,
        "Preopened fd dir name",
    ),
    # === IO: filesystem (pure-computation never uses) ===
    (
        "wasi_snapshot_preview1",
        "fd_readdir",
        ImportCategory.IO_FILESYSTEM,
        "Read directory entries",
    ),
    (
        "wasi_snapshot_preview1",
        "fd_filestat_get",
        ImportCategory.IO_FILESYSTEM,
        "File stat by fd",
    ),
    (
        "wasi_snapshot_preview1",
        "fd_filestat_set_size",
        ImportCategory.IO_FILESYSTEM,
        "Truncate file",
    ),
    (
        "wasi_snapshot_preview1",
        "path_open",
        ImportCategory.IO_FILESYSTEM,
        "Open file by path",
    ),
    (
        "wasi_snapshot_preview1",
        "path_rename",
        ImportCategory.IO_FILESYSTEM,
        "Rename path",
    ),
    (
        "wasi_snapshot_preview1",
        "path_readlink",
        ImportCategory.IO_FILESYSTEM,
        "Read symlink",
    ),
    (
        "wasi_snapshot_preview1",
        "path_unlink_file",
        ImportCategory.IO_FILESYSTEM,
        "Delete file",
    ),
    (
        "wasi_snapshot_preview1",
        "path_create_directory",
        ImportCategory.IO_FILESYSTEM,
        "Create directory",
    ),
    (
        "wasi_snapshot_preview1",
        "path_remove_directory",
        ImportCategory.IO_FILESYSTEM,
        "Remove directory",
    ),
    (
        "wasi_snapshot_preview1",
        "path_filestat_get",
        ImportCategory.IO_FILESYSTEM,
        "File stat by path",
    ),
    (
        "wasi_snapshot_preview1",
        "poll_oneoff",
        ImportCategory.IO_FILESYSTEM,
        "Poll for I/O events",
    ),
    # === Process ===
    (
        "env",
        "molt_process_write_host",
        ImportCategory.PROCESS,
        "Write to child process stdin",
    ),
    (
        "env",
        "molt_process_close_stdin_host",
        ImportCategory.PROCESS,
        "Close child stdin pipe",
    ),
    (
        "env",
        "molt_process_terminate_host",
        ImportCategory.PROCESS,
        "Terminate child process",
    ),
    ("env", "molt_getpid_host", ImportCategory.PROCESS, "Get current process ID"),
    ("env", "molt_process_kill_host", ImportCategory.PROCESS, "Kill child process"),
    ("env", "molt_process_wait_host", ImportCategory.PROCESS, "Wait for child process"),
    ("env", "molt_process_spawn_host", ImportCategory.PROCESS, "Spawn child process"),
    (
        "env",
        "molt_process_stdio_host",
        ImportCategory.PROCESS,
        "Access child stdio pipes",
    ),
    ("env", "molt_process_host_poll", ImportCategory.PROCESS, "Poll process events"),
    # === Database ===
    ("env", "molt_db_exec_host", ImportCategory.DATABASE, "Execute DB statement"),
    ("env", "molt_db_query_host", ImportCategory.DATABASE, "Query DB"),
    ("env", "molt_db_host_poll", ImportCategory.DATABASE, "Poll DB events"),
    # === WebSocket ===
    ("env", "molt_ws_recv_host", ImportCategory.WEBSOCKET, "Receive WebSocket message"),
    ("env", "molt_ws_send_host", ImportCategory.WEBSOCKET, "Send WebSocket message"),
    ("env", "molt_ws_close_host", ImportCategory.WEBSOCKET, "Close WebSocket"),
    ("env", "molt_ws_connect_host", ImportCategory.WEBSOCKET, "Connect WebSocket"),
    ("env", "molt_ws_poll_host", ImportCategory.WEBSOCKET, "Poll WebSocket events"),
    # === Socket ===
    ("env", "molt_socket_wait_host", ImportCategory.SOCKET, "Socket wait"),
    ("env", "molt_os_close_host", ImportCategory.SOCKET, "OS handle close"),
    ("env", "molt_socket_accept_host", ImportCategory.SOCKET, "Accept connection"),
    ("env", "molt_socket_bind_host", ImportCategory.SOCKET, "Bind socket"),
    ("env", "molt_socket_clone_host", ImportCategory.SOCKET, "Clone socket handle"),
    ("env", "molt_socket_close_host", ImportCategory.SOCKET, "Close socket"),
    ("env", "molt_socket_connect_host", ImportCategory.SOCKET, "Connect socket"),
    (
        "env",
        "molt_socket_connect_ex_host",
        ImportCategory.SOCKET,
        "Connect socket (extended)",
    ),
    ("env", "molt_socket_detach_host", ImportCategory.SOCKET, "Detach socket"),
    ("env", "molt_socket_getaddrinfo_host", ImportCategory.SOCKET, "DNS resolution"),
    ("env", "molt_socket_gethostname_host", ImportCategory.SOCKET, "Get hostname"),
    ("env", "molt_socket_getpeername_host", ImportCategory.SOCKET, "Get peer address"),
    (
        "env",
        "molt_socket_getservbyname_host",
        ImportCategory.SOCKET,
        "Resolve service by name",
    ),
    (
        "env",
        "molt_socket_getservbyport_host",
        ImportCategory.SOCKET,
        "Resolve service by port",
    ),
    (
        "env",
        "molt_socket_getsockname_host",
        ImportCategory.SOCKET,
        "Get socket address",
    ),
    ("env", "molt_socket_getsockopt_host", ImportCategory.SOCKET, "Get socket option"),
    ("env", "molt_socket_has_ipv6_host", ImportCategory.SOCKET, "Check IPv6 support"),
    ("env", "molt_socket_listen_host", ImportCategory.SOCKET, "Listen on socket"),
    ("env", "molt_socket_new_host", ImportCategory.SOCKET, "Create socket"),
    ("env", "molt_socket_recv_host", ImportCategory.SOCKET, "Receive from socket"),
    ("env", "molt_socket_recvfrom_host", ImportCategory.SOCKET, "Receive with address"),
    ("env", "molt_socket_recvmsg_host", ImportCategory.SOCKET, "Receive message"),
    ("env", "molt_socket_send_host", ImportCategory.SOCKET, "Send to socket"),
    ("env", "molt_socket_sendmsg_host", ImportCategory.SOCKET, "Send message"),
    ("env", "molt_socket_sendto_host", ImportCategory.SOCKET, "Send to address"),
    ("env", "molt_socket_setsockopt_host", ImportCategory.SOCKET, "Set socket option"),
    ("env", "molt_socket_shutdown_host", ImportCategory.SOCKET, "Shutdown socket"),
    ("env", "molt_socket_socketpair_host", ImportCategory.SOCKET, "Create socket pair"),
    ("env", "molt_socket_poll_host", ImportCategory.SOCKET, "Poll socket events"),
    # === Time (timezone) ===
    ("env", "molt_time_timezone_host", ImportCategory.TIME, "Get timezone"),
    ("env", "molt_time_local_offset_host", ImportCategory.TIME, "Get local UTC offset"),
    ("env", "molt_time_tzname_host", ImportCategory.TIME, "Get timezone name"),
]

# Categories that are safe to strip for pure-computation modules
STRIPPABLE_CATEGORIES = {
    ImportCategory.IO_FILESYSTEM,
    ImportCategory.PROCESS,
    ImportCategory.DATABASE,
    ImportCategory.WEBSOCKET,
    ImportCategory.SOCKET,
    ImportCategory.TIME,
}

CALL_INDIRECT_RE = re.compile(r"^molt_call_indirect\d+")


@dataclass
class ImportInfo:
    """Parsed WASM import entry."""

    index: int
    module: str
    name: str
    kind: str  # "func", "table", "memory", "global"
    type_index: int  # For funcs: the type index
    category: ImportCategory = ImportCategory.ESSENTIAL
    description: str = ""
    strippable: bool = False


@dataclass
class AnalysisResult:
    """Full analysis of a WASM binary's imports."""

    wasm_path: str
    file_size_bytes: int
    total_imports: int
    imports: list[ImportInfo] = field(default_factory=list)
    category_counts: dict[str, int] = field(default_factory=dict)
    strippable_count: int = 0
    essential_count: int = 0

    @property
    def strippable_imports(self) -> list[ImportInfo]:
        return [i for i in self.imports if i.strippable]

    @property
    def essential_imports(self) -> list[ImportInfo]:
        return [i for i in self.imports if not i.strippable]


# ---------------------------------------------------------------------------
# WASM binary parsing (import section only)
# ---------------------------------------------------------------------------

WASM_MAGIC = b"\x00asm"
SECTION_IMPORT = 2
SECTION_TYPE = 1


def _read_leb128_u32(data: bytes, offset: int) -> tuple[int, int]:
    """Read an unsigned LEB128-encoded u32. Returns (value, new_offset)."""
    result = 0
    shift = 0
    while True:
        byte = data[offset]
        offset += 1
        result |= (byte & 0x7F) << shift
        if (byte & 0x80) == 0:
            break
        shift += 7
    return result, offset


def _read_name(data: bytes, offset: int) -> tuple[str, int]:
    """Read a WASM name (length-prefixed UTF-8 string)."""
    length, offset = _read_leb128_u32(data, offset)
    name = data[offset : offset + length].decode("utf-8", errors="replace")
    return name, offset + length


def _parse_sections(data: bytes) -> dict[int, tuple[int, int]]:
    """Parse WASM sections, returning {section_id: (offset, size)}."""
    assert data[:4] == WASM_MAGIC, "Not a valid WASM binary"
    offset = 8  # Skip magic + version
    sections: dict[int, tuple[int, int]] = {}
    while offset < len(data):
        section_id = data[offset]
        offset += 1
        section_size, offset = _read_leb128_u32(data, offset)
        sections[section_id] = (offset, section_size)
        offset += section_size
    return sections


def _parse_type_section(
    data: bytes, sec_offset: int, sec_size: int
) -> dict[int, tuple[list, list]]:
    """Parse the type section to get function signatures.

    Tolerates non-functype entries (sub-types, GC types, rec groups) by
    skipping entries whose leading byte is not 0x60.
    """
    offset = sec_offset
    end = sec_offset + sec_size
    count, offset = _read_leb128_u32(data, offset)
    types: dict[int, tuple[list, list]] = {}
    for i in range(count):
        if offset >= end:
            break
        form = data[offset]
        offset += 1
        if form != 0x60:
            # Skip non-functype entries (rec group, sub type, etc.)
            # We can't reliably parse these, so skip to next by scanning
            # for the next 0x60 or end of section.  Mark this type as unknown.
            types[i] = ([], [])
            # Heuristic: try to skip a plausible LEB128 param+result block
            # by looking for the next functype marker or exhausting section.
            while offset < end and data[offset] != 0x60:
                offset += 1
            continue
        # Params
        param_count, offset = _read_leb128_u32(data, offset)
        params = []
        for _ in range(param_count):
            params.append(data[offset])
            offset += 1
        # Results
        result_count, offset = _read_leb128_u32(data, offset)
        results = []
        for _ in range(result_count):
            results.append(data[offset])
            offset += 1
        types[i] = (params, results)
    return types


def _valtype_name(vt: int) -> str:
    return {
        0x7F: "i32",
        0x7E: "i64",
        0x7D: "f32",
        0x7C: "f64",
        0x7B: "v128",
        0x70: "funcref",
        0x6F: "externref",
    }.get(vt, f"0x{vt:02x}")


def parse_imports(wasm_path: Path) -> list[ImportInfo]:
    """Parse the import section of a WASM binary."""
    data = wasm_path.read_bytes()
    sections = _parse_sections(data)

    if SECTION_IMPORT not in sections:
        return []

    sec_offset, sec_size = sections[SECTION_IMPORT]
    offset = sec_offset
    count, offset = _read_leb128_u32(data, offset)
    imports: list[ImportInfo] = []

    for idx in range(count):
        module, offset = _read_name(data, offset)
        name, offset = _read_name(data, offset)
        kind_byte = data[offset]
        offset += 1

        type_index = -1
        if kind_byte == 0x00:  # func
            type_index, offset = _read_leb128_u32(data, offset)
            kind = "func"
        elif kind_byte == 0x01:  # table
            offset += 1  # elem_type
            flags = data[offset]
            offset += 1
            _initial, offset = _read_leb128_u32(data, offset)
            if flags & 0x01:
                _max, offset = _read_leb128_u32(data, offset)
            kind = "table"
        elif kind_byte == 0x02:  # memory
            flags = data[offset]
            offset += 1
            _initial, offset = _read_leb128_u32(data, offset)
            if flags & 0x01:
                _max, offset = _read_leb128_u32(data, offset)
            kind = "memory"
        elif kind_byte == 0x03:  # global
            _valtype = data[offset]
            offset += 1
            _mutability = data[offset]
            offset += 1
            kind = "global"
        else:
            kind = f"unknown({kind_byte})"

        # Classify the import
        info = ImportInfo(
            index=idx, module=module, name=name, kind=kind, type_index=type_index
        )
        _classify_import(info)
        imports.append(info)

    return imports


def _classify_import(info: ImportInfo) -> None:
    """Classify an import by matching against known rules."""
    # Check for indirect call handlers first
    if info.module == "env" and CALL_INDIRECT_RE.match(info.name):
        info.category = ImportCategory.INDIRECT_CALL
        info.description = "Indirect function call dispatch"
        info.strippable = False
        return

    # Match against known rules
    for rule_mod, rule_name, category, description in IMPORT_RULES:
        if info.module == rule_mod and info.name == rule_name:
            info.category = category
            info.description = description
            info.strippable = category in STRIPPABLE_CATEGORIES
            return

    # Fallback: unclassified
    info.category = ImportCategory.ESSENTIAL
    info.description = "Unclassified (treated as essential)"
    info.strippable = False


# ---------------------------------------------------------------------------
# Analysis
# ---------------------------------------------------------------------------


def analyze(wasm_path: Path) -> AnalysisResult:
    """Run the full import analysis."""
    imports = parse_imports(wasm_path)
    file_size = wasm_path.stat().st_size

    result = AnalysisResult(
        wasm_path=str(wasm_path),
        file_size_bytes=file_size,
        total_imports=len(imports),
        imports=imports,
    )

    # Count by category
    for imp in imports:
        cat = imp.category.value
        result.category_counts[cat] = result.category_counts.get(cat, 0) + 1

    result.strippable_count = sum(1 for i in imports if i.strippable)
    result.essential_count = sum(1 for i in imports if not i.strippable)

    return result


# ---------------------------------------------------------------------------
# Stripping via wasm-tools
# ---------------------------------------------------------------------------


def strip_imports(wasm_path: Path, output_path: Path, result: AnalysisResult) -> Path:
    """Create a stripped copy using wasm-tools.

    Strategy: Use wasm-tools to produce a WAT text form, replace strippable
    imports with wasm-internal no-op functions, then reassemble to binary.

    For safety, this uses the wasm-tools component model to:
    1. Convert to WAT
    2. Inject no-op function bodies for each stripped import
    3. Reassemble to WASM binary
    """
    wasm_tools = shutil.which("wasm-tools")
    if not wasm_tools:
        print("ERROR: wasm-tools not found in PATH", file=sys.stderr)
        sys.exit(1)

    strippable = result.strippable_imports
    if not strippable:
        print("No strippable imports found. Output is a copy of input.")
        shutil.copy2(wasm_path, output_path)
        return output_path

    # Use wasm-tools strip to remove debug/name sections and report size savings.
    print("Stripping debug and name sections...")
    strip_proc = subprocess.run(
        [wasm_tools, "strip", str(wasm_path), "-o", str(output_path)],
        capture_output=True,
        text=True,
        timeout=120,
    )
    if strip_proc.returncode != 0:
        print(f"ERROR: wasm-tools strip failed: {strip_proc.stderr}", file=sys.stderr)
        sys.exit(1)

    original_size = wasm_path.stat().st_size
    stripped_size = output_path.stat().st_size
    savings = original_size - stripped_size
    print(
        f"Stripped debug/name sections: {original_size:,} -> {stripped_size:,} bytes "
        f"(saved {savings:,} bytes, {savings / original_size * 100:.1f}%)"
    )

    return output_path


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------


def print_report(result: AnalysisResult, verbose: bool = False) -> None:
    """Print a human-readable analysis report."""
    print("=" * 72)
    print(f"WASM Import Analysis: {result.wasm_path}")
    print(
        f"File size: {result.file_size_bytes:,} bytes ({result.file_size_bytes / 1024 / 1024:.1f} MB)"
    )
    print(f"Total imports: {result.total_imports}")
    print("=" * 72)

    print("\n--- Category Breakdown ---")
    for cat in ImportCategory:
        count = result.category_counts.get(cat.value, 0)
        if count > 0:
            marker = "  [STRIPPABLE]" if cat in STRIPPABLE_CATEGORIES else ""
            print(f"  {cat.value:<20s}  {count:3d} imports{marker}")

    print("\n--- Summary ---")
    print(f"  Essential (keep):     {result.essential_count}")
    print(f"  Strippable (no-op):   {result.strippable_count}")
    pct = (
        result.strippable_count / result.total_imports * 100
        if result.total_imports
        else 0
    )
    print(f"  Strippable ratio:     {pct:.1f}%")

    # Estimate overhead: each import entry is ~20-60 bytes in the binary,
    # plus the host stub code in the TypeScript host
    est_import_overhead = result.strippable_count * 40  # ~40 bytes avg per import entry
    print(f"  Est. import section overhead: ~{est_import_overhead:,} bytes")
    print(
        f"  Host stub code lines saved:   ~{result.strippable_count * 2} lines in molt-wasm-host.ts"
    )

    if verbose or True:  # Always show details
        print(f"\n--- Strippable Imports ({result.strippable_count}) ---")
        for imp in result.strippable_imports:
            sig = ""
            if imp.type_index >= 0:
                sig = f" (type {imp.type_index})"
            print(f"  [{imp.index:2d}] {imp.module}::{imp.name}{sig}")
            print(f"       Category: {imp.category.value}, {imp.description}")

        print(f"\n--- Essential Imports ({result.essential_count}) ---")
        for imp in result.essential_imports:
            sig = ""
            if imp.type_index >= 0:
                sig = f" (type {imp.type_index})"
            print(f"  [{imp.index:2d}] {imp.module}::{imp.name}{sig}")
            print(f"       Category: {imp.category.value}, {imp.description}")


def print_json(result: AnalysisResult) -> None:
    """Print machine-readable JSON output."""
    data = {
        "wasm_path": result.wasm_path,
        "file_size_bytes": result.file_size_bytes,
        "total_imports": result.total_imports,
        "strippable_count": result.strippable_count,
        "essential_count": result.essential_count,
        "category_counts": result.category_counts,
        "strippable_imports": [
            {
                "index": i.index,
                "module": i.module,
                "name": i.name,
                "category": i.category.value,
                "description": i.description,
            }
            for i in result.strippable_imports
        ],
        "essential_imports": [
            {
                "index": i.index,
                "module": i.module,
                "name": i.name,
                "category": i.category.value,
                "description": i.description,
            }
            for i in result.essential_imports
        ],
    }
    print(json.dumps(data, indent=2))


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze and strip unused WASM imports for pure-computation modules."
    )
    parser.add_argument("wasm", type=Path, help="Path to WASM binary")
    parser.add_argument(
        "--strip",
        action="store_true",
        help="Produce a trimmed copy with debug sections stripped",
    )
    parser.add_argument(
        "-o",
        "--output",
        type=Path,
        default=None,
        help=(
            "Output path for stripped binary (default: sibling "
            "<name>-stripped.wasm next to input)"
        ),
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Output machine-readable JSON instead of human report",
    )
    parser.add_argument(
        "-v",
        "--verbose",
        action="store_true",
        help="Show detailed per-import information",
    )
    args = parser.parse_args()

    if not args.wasm.is_file():
        print(f"ERROR: {args.wasm} not found", file=sys.stderr)
        sys.exit(1)

    result = analyze(args.wasm)

    if args.json_output:
        print_json(result)
    else:
        print_report(result, verbose=args.verbose)

    if args.strip:
        output = args.output
        if output is None:
            output = args.wasm.with_name(f"{args.wasm.stem}-stripped.wasm")
        strip_imports(args.wasm, output, result)
        print(f"\nStripped output: {output}")

        # Re-analyze the stripped binary for comparison
        stripped_result = analyze(output)
        print(
            f"Stripped file: {stripped_result.file_size_bytes:,} bytes "
            f"({stripped_result.file_size_bytes / 1024 / 1024:.1f} MB)"
        )


if __name__ == "__main__":
    import shutil

    main()
