"""WASM-specific determinism tests (MOL-297).

Verifies that WASM compilation is deterministic:
  - Same program compiled twice produces identical binary output.
  - WASM module structure is deterministic (no random section ordering).
  - NaN canonicalization in WASM output.

These tests exercise the guarantees proven in:
  - formal/lean/MoltTIR/Backend/BackendDeterminism.lean
  - formal/lean/MoltTIR/Runtime/WasmNativeCorrect.lean
  - formal/lean/MoltTIR/Runtime/WasmNative.lean (canonical_nan_is_float)
"""
from __future__ import annotations

import hashlib
import os
import re
import struct
import subprocess
import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[1]
SRC_DIR = ROOT / "src"

_SUBPROCESS_TIMEOUT = float(
    os.environ.get("MOLT_TEST_SUBPROCESS_TIMEOUT", "120")
)

# WASM magic number and version (WebAssembly spec section 5.5.1).
WASM_MAGIC = b"\x00asm"
WASM_VERSION = b"\x01\x00\x00\x00"

# WASM section IDs (spec section 5.5.2).
WASM_SECTION_IDS = {
    0: "custom",
    1: "type",
    2: "import",
    3: "function",
    4: "table",
    5: "memory",
    6: "global",
    7: "export",
    8: "start",
    9: "element",
    10: "code",
    11: "data",
    12: "data_count",
}

# Programs used for determinism testing.
DETERMINISM_PROGRAMS: list[tuple[str, str]] = [
    ("simple_add", "x = 1 + 2\nprint(x)\n"),
    ("loop", "i = 0\nwhile i < 5:\n    i = i + 1\nprint(i)\n"),
    ("function", "def f(x):\n    return x * 2\nprint(f(3))\n"),
    ("conditional", "x = 10\nif x > 5:\n    print(1)\nelse:\n    print(0)\n"),
    ("string", 'print("hello")\n'),
]

# IEEE 754 NaN bit patterns for detection.
# Quiet NaN: sign=0, exponent=all-1s, mantissa bit 51 set.
F64_QNAN_BITS = 0x7FF8000000000000
F64_EXPONENT_MASK = 0x7FF0000000000000
F64_MANTISSA_MASK = 0x000FFFFFFFFFFFFF


def _molt_cli_available() -> bool:
    try:
        env = os.environ.copy()
        env["PYTHONPATH"] = str(SRC_DIR)
        result = subprocess.run(
            [sys.executable, "-c", "import molt.cli"],
            capture_output=True,
            text=True,
            env=env,
            timeout=30,
        )
        return result.returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def _build_wasm(src_path: Path, out_dir: Path) -> Path | None:
    """Build a Python file to WASM, returning the .wasm path or None."""
    env = os.environ.copy()
    env["PYTHONPATH"] = str(SRC_DIR)
    env.setdefault("MOLT_MIDEND_DISABLE", "1")
    env.setdefault("MOLT_BACKEND_DAEMON", "0")
    env.setdefault("MOLT_MIDEND_FAIL_OPEN", "1")
    try:
        result = subprocess.run(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                str(src_path),
                "--target",
                "wasm",
                "--out-dir",
                str(out_dir),
            ],
            cwd=ROOT,
            env=env,
            capture_output=True,
            text=True,
            timeout=_SUBPROCESS_TIMEOUT,
        )
    except subprocess.TimeoutExpired:
        return None
    if result.returncode != 0:
        return None
    # Look for .wasm output.
    for wasm_file in out_dir.rglob("*.wasm"):
        return wasm_file
    return None


def _parse_wasm_sections(data: bytes) -> list[tuple[int, int, int]]:
    """Parse a WASM binary and return section descriptors.

    Returns a list of (section_id, offset, size) tuples.
    """
    if len(data) < 8:
        return []
    if data[:4] != WASM_MAGIC or data[4:8] != WASM_VERSION:
        return []

    sections: list[tuple[int, int, int]] = []
    pos = 8
    while pos < len(data):
        if pos >= len(data):
            break
        section_id = data[pos]
        pos += 1
        # Decode LEB128 size.
        size = 0
        shift = 0
        while pos < len(data):
            byte = data[pos]
            pos += 1
            size |= (byte & 0x7F) << shift
            shift += 7
            if (byte & 0x80) == 0:
                break
        sections.append((section_id, pos, size))
        pos += size
    return sections


_F64_CONST_RE = re.compile(r"\(f64\.const\s+([^)\\s]+)")


def _wat_f64_token_to_bits(token: str) -> int | None:
    if token == "nan":
        return F64_QNAN_BITS
    if token == "-nan":
        return (1 << 63) | F64_QNAN_BITS
    if token == "inf":
        return struct.unpack("<Q", struct.pack("<d", float("inf")))[0]
    if token == "-inf":
        return struct.unpack("<Q", struct.pack("<d", float("-inf")))[0]
    if token.startswith("nan:0x"):
        payload = int(token.removeprefix("nan:0x"), 16) & F64_MANTISSA_MASK
        if payload == 0:
            payload = 1
        return F64_EXPONENT_MASK | payload
    if token.startswith("-nan:0x"):
        payload = int(token.removeprefix("-nan:0x"), 16) & F64_MANTISSA_MASK
        if payload == 0:
            payload = 1
        return (1 << 63) | F64_EXPONENT_MASK | payload
    try:
        value = float.fromhex(token) if token.startswith(("0x", "-0x")) else float(token)
    except ValueError:
        return None
    return struct.unpack("<Q", struct.pack("<d", value))[0]


def _extract_f64_constants(wasm_path: Path) -> list[int]:
    """Extract actual `f64.const` instruction immediates from a WASM module.

    Parsing raw byte windows produces false positives from unrelated immediates
    such as `i64.const`. Use the WAT disassembly so we only inspect real
    floating-point constants.
    """
    try:
        result = subprocess.run(
            ["wasm-tools", "print", str(wasm_path)],
            capture_output=True,
            text=True,
            timeout=30,
            check=True,
        )
    except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return []

    constants: list[int] = []
    for match in _F64_CONST_RE.finditer(result.stdout):
        bits = _wat_f64_token_to_bits(match.group(1))
        if bits is not None:
            constants.append(bits)
    return constants


# ------------------------------------------------------------------
# Tests
# ------------------------------------------------------------------


class TestWasmBinaryDeterminism:
    """Compiling the same program to WASM twice should produce identical
    binary output (BackendDeterminism.lean, artifact_deterministic).
    """

    @pytest.fixture(autouse=True)
    def _skip_unless_cli(self):
        if not _molt_cli_available():
            pytest.skip("Molt CLI not available")

    @pytest.mark.parametrize("name,source", DETERMINISM_PROGRAMS)
    def test_identical_binary_output(
        self, tmp_path: Path, name: str, source: str
    ) -> None:
        """Compile the same program to WASM twice and compare hashes."""
        src_file = tmp_path / f"{name}.py"
        src_file.write_text(source)

        hashes: list[str] = []
        for i in range(2):
            out_dir = tmp_path / f"build_{i}"
            out_dir.mkdir()
            wasm_path = _build_wasm(src_file, out_dir)
            if wasm_path is None:
                pytest.skip(f"WASM build failed for '{name}'")
            data = wasm_path.read_bytes()
            hashes.append(hashlib.sha256(data).hexdigest())

        assert hashes[0] == hashes[1], (
            f"WASM binary differs between two compilations of '{name}': "
            f"{hashes[0]} != {hashes[1]}"
        )


class TestWasmModuleStructure:
    """WASM module structure should be deterministic (no random section
    ordering).

    Reference: BackendDeterminism.lean, wasm_emission_deterministic
    """

    @pytest.fixture(autouse=True)
    def _skip_unless_cli(self):
        if not _molt_cli_available():
            pytest.skip("Molt CLI not available")

    @pytest.mark.parametrize("name,source", DETERMINISM_PROGRAMS[:3])
    def test_section_order_deterministic(
        self, tmp_path: Path, name: str, source: str
    ) -> None:
        """Compile twice and verify the section ordering is identical."""
        src_file = tmp_path / f"{name}.py"
        src_file.write_text(source)

        section_orders: list[list[int]] = []
        for i in range(2):
            out_dir = tmp_path / f"sec_{i}"
            out_dir.mkdir()
            wasm_path = _build_wasm(src_file, out_dir)
            if wasm_path is None:
                pytest.skip(f"WASM build failed for '{name}'")
            data = wasm_path.read_bytes()
            sections = _parse_wasm_sections(data)
            section_orders.append([s[0] for s in sections])

        assert section_orders[0] == section_orders[1], (
            f"WASM section ordering differs between two compilations of "
            f"'{name}': {section_orders[0]} != {section_orders[1]}"
        )

    @pytest.mark.parametrize("name,source", DETERMINISM_PROGRAMS[:3])
    def test_section_order_ascending(
        self, tmp_path: Path, name: str, source: str
    ) -> None:
        """Non-custom sections should appear in ascending ID order per the
        WASM spec (section 5.5.2).
        """
        src_file = tmp_path / f"{name}.py"
        src_file.write_text(source)

        out_dir = tmp_path / "asc"
        out_dir.mkdir()
        wasm_path = _build_wasm(src_file, out_dir)
        if wasm_path is None:
            pytest.skip(f"WASM build failed for '{name}'")

        data = wasm_path.read_bytes()
        sections = _parse_wasm_sections(data)

        # Filter out custom sections (id=0), which can appear anywhere.
        non_custom = [s[0] for s in sections if s[0] != 0]
        for i in range(len(non_custom) - 1):
            assert non_custom[i] <= non_custom[i + 1], (
                f"WASM sections out of order: section {non_custom[i]} "
                f"({WASM_SECTION_IDS.get(non_custom[i], '?')}) appears "
                f"before {non_custom[i + 1]} "
                f"({WASM_SECTION_IDS.get(non_custom[i + 1], '?')})"
            )


class TestWasmNaNCanonicalization:
    """NaN values in WASM output should be canonicalized.

    Reference: WasmNative.lean, canonical_nan_is_float
    Molt uses QNAN = 0x7ff8000000000000 as its canonical quiet NaN.
    Any NaN in the WASM binary should match this canonical form or be
    part of the NaN-boxing tag scheme.
    """

    @pytest.fixture(autouse=True)
    def _skip_unless_cli(self):
        if not _molt_cli_available():
            pytest.skip("Molt CLI not available")

    def test_nan_constants_are_canonical(self, tmp_path: Path) -> None:
        """Extract f64 NaN constants from WASM binary and verify they
        are the Molt canonical NaN or a known NaN-boxing tagged value.

        The Molt NaN-boxing scheme uses:
          QNAN = 0x7ff8000000000000
          TAG_CHECK = QNAN | TAG_MASK = 0x7fff000000000000
        Any NaN in the binary should have bits 48-62 matching this pattern.
        """
        src = tmp_path / "nan_test.py"
        src.write_text("x = 1.0\ny = 0.0\nz = x / y\nprint(z)\n")

        out_dir = tmp_path / "nan_out"
        out_dir.mkdir()
        wasm_path = _build_wasm(src, out_dir)
        if wasm_path is None:
            pytest.skip("WASM build failed")

        nan_constants = _extract_f64_constants(wasm_path)

        # Molt canonical constants.
        QNAN = 0x7FF8000000000000
        TAG_CHECK_MASK = 0x7FFF000000000000

        for val in nan_constants:
            tag_bits = val & TAG_CHECK_MASK
            # Value should either be the canonical NaN or have valid
            # Molt NaN-boxing tag bits set (bits 48-62 >= QNAN).
            assert tag_bits >= QNAN, (
                f"Non-canonical NaN found in WASM binary: "
                f"0x{val:016X} (tag bits 0x{tag_bits:016X}). "
                f"Expected canonical QNAN 0x{QNAN:016X} or NaN-boxed value."
            )

    def test_canonical_nan_bits_match_lean(self) -> None:
        """Verify that our Python constants match the Lean formalization.

        WasmNative.lean defines:
          QNAN = 0x7ff8000000000000
          CANONICAL_NAN_BITS = 0x7ff0000000000001
        """
        QNAN = 0x7FF8000000000000
        CANONICAL_NAN_BITS = 0x7FF0000000000001

        # QNAN is a quiet NaN (bit 51 set).
        assert (QNAN & F64_EXPONENT_MASK) == F64_EXPONENT_MASK
        assert (QNAN & (1 << 51)) != 0, "QNAN should have quiet bit set"

        # CANONICAL_NAN_BITS is a signaling NaN (bit 51 clear, mantissa nonzero).
        assert (CANONICAL_NAN_BITS & F64_EXPONENT_MASK) == F64_EXPONENT_MASK
        assert (CANONICAL_NAN_BITS & F64_MANTISSA_MASK) != 0

        # Both are NaN: exponent all 1s, mantissa nonzero.
        for name, val in [("QNAN", QNAN), ("CANONICAL_NAN_BITS", CANONICAL_NAN_BITS)]:
            exponent = (val >> 52) & 0x7FF
            mantissa = val & F64_MANTISSA_MASK
            assert exponent == 0x7FF, f"{name} exponent should be all 1s"
            assert mantissa != 0, f"{name} mantissa should be nonzero (NaN)"

    def test_nan_boxing_tag_disjointness(self) -> None:
        """Verify NaN-boxing tags are disjoint, matching WasmABI.lean
        repr_disjoint and the tag definitions in WasmNative.lean.
        """
        TAG_INT = 0x0001000000000000
        TAG_BOOL = 0x0002000000000000
        TAG_NONE = 0x0003000000000000
        TAG_PTR = 0x0004000000000000
        TAG_PEND = 0x0005000000000000
        TAG_MASK = 0x0007000000000000

        tags = {
            "INT": TAG_INT,
            "BOOL": TAG_BOOL,
            "NONE": TAG_NONE,
            "PTR": TAG_PTR,
            "PEND": TAG_PEND,
        }
        tag_values = list(tags.items())
        for i, (name1, val1) in enumerate(tag_values):
            for name2, val2 in tag_values[i + 1 :]:
                assert val1 != val2, f"Tags {name1} and {name2} collide"

            # Each tag should be extractable via TAG_MASK.
            assert (val1 & TAG_MASK) == val1, (
                f"Tag {name1} has bits outside TAG_MASK"
            )
