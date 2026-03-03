#!/usr/bin/env python3
"""Analyze compiled Molt binaries for codegen quality metrics.

Extracts .text section size, per-function sizes, branch instruction density,
call instruction counts, and identifies the largest generated functions.

Usage:
    python tools/check_codegen_quality.py path/to/binary
    python tools/check_codegen_quality.py --json path/to/binary
    python tools/check_codegen_quality.py --json-out metrics.json path/to/binary
    python tools/check_codegen_quality.py --top 20 path/to/binary

Output JSON schema:
    {
        "binary": str,
        "arch": str,
        "text_bytes": int,
        "func_count": int,
        "branch_density_per_kb": float,
        "call_count": int,
        "top_functions": [{"name": str, "size_bytes": int}, ...]
    }

Exit codes:
    0 -- success
    1 -- error (missing binary, tool not found, parse failure)
"""

from __future__ import annotations

import argparse
import json
import platform
import re
import shutil
import subprocess
import sys
from dataclasses import asdict, dataclass, field
from pathlib import Path


# ---------------------------------------------------------------------------
# Result types
# ---------------------------------------------------------------------------


@dataclass
class FunctionInfo:
    name: str
    size_bytes: int


@dataclass
class CodegenMetrics:
    binary: str
    arch: str
    text_bytes: int
    func_count: int
    branch_density_per_kb: float
    call_count: int
    top_functions: list[FunctionInfo] = field(default_factory=list)

    def to_dict(self) -> dict:
        d = asdict(self)
        d["top_functions"] = [asdict(f) for f in self.top_functions]
        return d


# ---------------------------------------------------------------------------
# Architecture detection
# ---------------------------------------------------------------------------

ARCH_X86_64 = "x86_64"
ARCH_ARM64 = "arm64"

# Branch mnemonics per architecture.
# x86_64: jmp and all conditional jump variants (jcc family).
# arm64: unconditional b, conditional b.<cond>, cbz, cbnz, tbz, tbnz.
X86_BRANCH_RE = re.compile(
    r"\b(jmp|je|jne|jz|jnz|jg|jge|jl|jle|ja|jae|jb|jbe|jc|jnc|jo|jno"
    r"|js|jns|jp|jnp|jcxz|jecxz|jrcxz)\b"
)
ARM64_BRANCH_RE = re.compile(r"\b(b|b\.\w+|bl|blr|br|cbz|cbnz|tbz|tbnz|ret)\b")

X86_CALL_RE = re.compile(r"\bcall\b")
ARM64_CALL_RE = re.compile(r"\bbl\b")


def detect_arch() -> str:
    """Detect the host architecture."""
    machine = platform.machine().lower()
    if machine in ("x86_64", "amd64"):
        return ARCH_X86_64
    if machine in ("arm64", "aarch64"):
        return ARCH_ARM64
    # Fallback: return raw machine string, callers handle unknown gracefully.
    return machine


# ---------------------------------------------------------------------------
# Tool resolution
# ---------------------------------------------------------------------------


def find_objdump() -> str | None:
    """Find the best available objdump. Prefer llvm-objdump."""
    for candidate in ("llvm-objdump", "objdump"):
        path = shutil.which(candidate)
        if path is not None:
            return path
    # On macOS with Homebrew LLVM, check the brew prefix.
    brew_llvm = shutil.which("brew")
    if brew_llvm:
        try:
            prefix = subprocess.check_output(
                ["brew", "--prefix", "llvm"],
                stderr=subprocess.DEVNULL,
                text=True,
            ).strip()
            candidate = Path(prefix) / "bin" / "llvm-objdump"
            if candidate.is_file():
                return str(candidate)
        except (subprocess.CalledProcessError, FileNotFoundError):
            pass
    return None


def find_nm() -> str | None:
    """Find nm (llvm-nm preferred)."""
    for candidate in ("llvm-nm", "nm"):
        path = shutil.which(candidate)
        if path is not None:
            return path
    return None


def find_size() -> str | None:
    """Find size (llvm-size preferred)."""
    for candidate in ("llvm-size", "size"):
        path = shutil.which(candidate)
        if path is not None:
            return path
    return None


# ---------------------------------------------------------------------------
# Metric extraction
# ---------------------------------------------------------------------------


def get_text_section_size(binary: Path, objdump: str) -> int | None:
    """Extract .text section size in bytes from objdump -h output.

    Works for both Mach-O (macOS) and ELF (Linux) binaries.
    For Mach-O, the section is listed as __text; for ELF it is .text.
    """
    try:
        output = subprocess.check_output(
            [objdump, "-h", str(binary)],
            stderr=subprocess.DEVNULL,
            text=True,
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None

    # objdump -h output format (columns vary slightly):
    # Idx Name          Size     VMA              ...
    #   0 .text         0001a4b0 0000000000001000 ...
    # On Mach-O with llvm-objdump:
    #   0 __text        0001a4b0 0000000100003a50 ...
    for line in output.splitlines():
        stripped = line.strip()
        # Match lines containing .text or __text as the section name.
        if not stripped:
            continue
        parts = stripped.split()
        if len(parts) < 3:
            continue
        section_name = parts[1] if parts[0].isdigit() else parts[0]
        if section_name in (".text", "__text"):
            # Size is the next field after the name (hex).
            size_idx = 2 if parts[0].isdigit() else 1
            try:
                return int(parts[size_idx], 16)
            except (ValueError, IndexError):
                continue

    return None


def get_text_section_size_fallback(binary: Path, size_tool: str) -> int | None:
    """Fallback: use `size` to get approximate text size."""
    try:
        output = subprocess.check_output(
            [size_tool, str(binary)],
            stderr=subprocess.DEVNULL,
            text=True,
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None

    # `size` output:
    # __TEXT  __DATA  __OBJC  others  dec     hex
    # 172032  8192    0       4096    184320  2d000
    # or (Linux):
    # text    data    bss     dec     hex     filename
    # 172032  8192    0       180224  2c000   binary
    lines = output.strip().splitlines()
    if len(lines) >= 2:
        parts = lines[1].split()
        if parts:
            try:
                return int(parts[0])
            except ValueError:
                pass
    return None


def get_function_sizes(binary: Path, nm_tool: str) -> list[FunctionInfo]:
    """Extract per-function sizes from the symbol table via nm -S.

    Returns a list of FunctionInfo sorted by size descending.
    On macOS (Mach-O), nm does not support -S. We fall back to parsing
    nm output and estimating sizes from address gaps.
    """
    functions: list[FunctionInfo] = []

    # Try nm -S first (works on ELF / llvm-nm).
    try:
        output = subprocess.check_output(
            [nm_tool, "-S", "--defined-only", str(binary)],
            stderr=subprocess.DEVNULL,
            text=True,
        )
        # Format: <address> <size> <type> <name>
        for line in output.splitlines():
            parts = line.strip().split()
            if len(parts) >= 4:
                sym_type = parts[2]
                # T/t = text (code) symbols.
                if sym_type in ("T", "t"):
                    try:
                        size = int(parts[1], 16)
                        name = parts[3]
                        if size > 0:
                            functions.append(FunctionInfo(name=name, size_bytes=size))
                    except ValueError:
                        continue
    except (subprocess.CalledProcessError, FileNotFoundError):
        pass

    if functions:
        functions.sort(key=lambda f: f.size_bytes, reverse=True)
        return functions

    # Fallback: plain nm, estimate sizes from address deltas.
    try:
        output = subprocess.check_output(
            [nm_tool, "--defined-only", "-n", str(binary)],
            stderr=subprocess.DEVNULL,
            text=True,
        )
    except (subprocess.CalledProcessError, FileNotFoundError):
        return []

    text_syms: list[tuple[int, str]] = []
    for line in output.splitlines():
        parts = line.strip().split()
        if len(parts) >= 3:
            sym_type = parts[1]
            if sym_type in ("T", "t"):
                try:
                    addr = int(parts[0], 16)
                    name = parts[2]
                    text_syms.append((addr, name))
                except ValueError:
                    continue

    # Sort by address and compute deltas.
    text_syms.sort()
    for i in range(len(text_syms) - 1):
        addr, name = text_syms[i]
        next_addr = text_syms[i + 1][0]
        size = next_addr - addr
        if size > 0:
            functions.append(FunctionInfo(name=name, size_bytes=size))

    functions.sort(key=lambda f: f.size_bytes, reverse=True)
    return functions


def disassemble_text(binary: Path, objdump: str) -> str:
    """Disassemble the .text section and return the raw output."""
    # Use --disassemble to get all code sections.
    try:
        output = subprocess.check_output(
            [objdump, "-d", "--no-show-raw-insn", str(binary)],
            stderr=subprocess.DEVNULL,
            text=True,
        )
        return output
    except (subprocess.CalledProcessError, FileNotFoundError):
        return ""


def count_instructions(disasm: str, arch: str) -> tuple[int, int]:
    """Count branch and call instructions in disassembly output.

    Returns (branch_count, call_count).
    """
    if arch == ARCH_X86_64:
        branch_re = X86_BRANCH_RE
        call_re = X86_CALL_RE
    elif arch == ARCH_ARM64:
        branch_re = ARM64_BRANCH_RE
        call_re = ARM64_CALL_RE
    else:
        # Unknown arch: return zeros rather than crashing.
        return 0, 0

    branch_count = 0
    call_count = 0

    for line in disasm.splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("Disassembly") or ":" not in stripped:
            continue
        # Instruction lines look like:
        #   100003a50:  mov  x0, x1
        # After --no-show-raw-insn, we get the mnemonic directly.
        # Split on the first colon to get the instruction part.
        parts = stripped.split(":", 1)
        if len(parts) < 2:
            continue
        instr_part = parts[1].strip()
        if not instr_part:
            continue

        # Extract the mnemonic (first whitespace-delimited token).
        mnemonic = instr_part.split()[0] if instr_part.split() else ""
        if not mnemonic:
            continue

        if branch_re.match(mnemonic):
            # For arm64, exclude bl (that is a call, not a branch).
            if arch == ARCH_ARM64 and mnemonic == "bl":
                call_count += 1
            else:
                branch_count += 1
        elif call_re.match(mnemonic):
            call_count += 1

    return branch_count, call_count


# ---------------------------------------------------------------------------
# Main analysis
# ---------------------------------------------------------------------------


def analyze_binary(binary: Path, top_n: int = 10) -> CodegenMetrics:
    """Run the full codegen quality analysis on a binary."""
    arch = detect_arch()

    # Resolve tools.
    objdump = find_objdump()
    nm_tool = find_nm()
    size_tool = find_size()

    if objdump is None:
        print(
            "error: no objdump or llvm-objdump found; install Xcode CLI tools or LLVM",
            file=sys.stderr,
        )
        sys.exit(1)

    if nm_tool is None:
        print("error: no nm found", file=sys.stderr)
        sys.exit(1)

    # 1. Text section size.
    text_bytes = get_text_section_size(binary, objdump)
    if text_bytes is None and size_tool is not None:
        text_bytes = get_text_section_size_fallback(binary, size_tool)
    if text_bytes is None:
        text_bytes = 0

    # 2. Per-function sizes.
    functions = get_function_sizes(binary, nm_tool)
    func_count = len(functions)
    top_functions = functions[:top_n]

    # 3. Disassemble and count branch/call instructions.
    disasm = disassemble_text(binary, objdump)
    branch_count, call_count = count_instructions(disasm, arch)

    # 4. Branch density per KB of .text.
    if text_bytes > 0:
        branch_density_per_kb = round(branch_count / (text_bytes / 1024), 2)
    else:
        branch_density_per_kb = 0.0

    return CodegenMetrics(
        binary=str(binary),
        arch=arch,
        text_bytes=text_bytes,
        func_count=func_count,
        branch_density_per_kb=branch_density_per_kb,
        call_count=call_count,
        top_functions=top_functions,
    )


def print_human_summary(metrics: CodegenMetrics) -> None:
    """Print a human-readable summary to stdout."""
    print(f"Codegen Quality Report: {metrics.binary}")
    print(f"  Architecture:          {metrics.arch}")
    print(f"  .text section size:    {metrics.text_bytes:,} bytes")
    print(f"  Function count:        {metrics.func_count:,}")
    print(f"  Branch density:        {metrics.branch_density_per_kb:.2f} per KB")
    print(f"  Call instructions:     {metrics.call_count:,}")

    if metrics.top_functions:
        print(f"\n  Top {len(metrics.top_functions)} largest functions:")
        for i, func in enumerate(metrics.top_functions, 1):
            print(f"    {i:3d}. {func.size_bytes:>8,} bytes  {func.name}")


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyze compiled Molt binaries for codegen quality metrics.",
    )
    parser.add_argument(
        "binary",
        type=Path,
        help="Path to the compiled binary to analyze",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_stdout",
        help="Print JSON to stdout instead of human-readable summary",
    )
    parser.add_argument(
        "--json-out",
        metavar="FILE",
        type=Path,
        help="Write JSON results to FILE",
    )
    parser.add_argument(
        "--top",
        type=int,
        default=10,
        metavar="N",
        help="Number of largest functions to include (default: 10)",
    )
    args = parser.parse_args()

    binary: Path = args.binary
    if not binary.is_file():
        print(f"error: binary not found: {binary}", file=sys.stderr)
        sys.exit(1)

    metrics = analyze_binary(binary, top_n=args.top)

    # Output.
    if args.json_stdout:
        print(json.dumps(metrics.to_dict(), indent=2))
    else:
        print_human_summary(metrics)

    if args.json_out is not None:
        args.json_out.parent.mkdir(parents=True, exist_ok=True)
        with open(args.json_out, "w") as f:
            json.dump(metrics.to_dict(), f, indent=2)
            f.write("\n")
        if not args.json_stdout:
            print(f"\nJSON written to {args.json_out}")


if __name__ == "__main__":
    main()
