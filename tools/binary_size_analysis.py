#!/usr/bin/env python3
"""Binary size breakdown analysis for Molt native and WASM binaries.

Reports exact file bytes and section coverage, name-based native attribution,
and declared symbol sizes where the object format provides them.  Supports a ``--compare`` mode for
measuring the effect of optimisations across two builds.

Usage::

    # Native (Mach-O / ELF) binary
    python tools/binary_size_analysis.py .molt_cache/home/bin/bench_sum_molt

    # WASM binary
    python tools/binary_size_analysis.py target/wasm32-wasip1/release/bench_sum.wasm

    # Compare two builds
    python tools/binary_size_analysis.py --compare before.bin after.bin

    # JSON output
    python tools/binary_size_analysis.py --json .molt_cache/home/bin/bench_sum_molt

    # Custom size budget
    python tools/binary_size_analysis.py --budget 25MB .molt_cache/home/bin/bench_sum_molt
"""

from __future__ import annotations

import argparse
import ast
from collections import Counter
import os
import tomllib
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
if str(REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(REPO_ROOT))
SRC_ROOT = REPO_ROOT / "src"
if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from tools import harness_memory_guard  # noqa: E402
from molt.cargo_workspace import workspace_member_manifests  # noqa: E402
from molt.exact_json import loads_exact  # noqa: E402
from molt.native_artifact_header import (  # noqa: E402
    MachOHeader,
    NativeArtifact,
    NativeFileKind,
    read_native_artifact,
)
from molt.toolchain_identity import (  # noqa: E402
    capture_stable_regular_file,
    stable_executable_probe,
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)
from molt.wasm_artifact import (  # noqa: E402
    WasmSectionSpan,
    read_wasm_code_metrics,
    read_wasm_section_spans,
)

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------

WASM_MAGIC = b"\x00asm"

# Default size budget (native ~30MB, WASM ~17MB — allow some headroom).
DEFAULT_BUDGET_NATIVE_MB = 35.0
DEFAULT_BUDGET_WASM_MB = 20.0


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _fmt_bytes(n: int) -> str:
    """Human-readable byte size."""
    if n >= 1024 * 1024:
        return f"{n / 1024 / 1024:.2f} MB"
    if n >= 1024:
        return f"{n / 1024:.1f} KB"
    return f"{n} B"


def _pct(part: int, total: int) -> str:
    if total == 0:
        return "0.0%"
    return f"{part / total * 100:.1f}%"


def _parse_size_spec(spec: str) -> int:
    """Parse '25MB', '512KB', etc. into bytes."""
    spec = spec.strip().upper()
    if spec.endswith("GB"):
        return int(float(spec[:-2]) * 1024 * 1024 * 1024)
    if spec.endswith("MB"):
        return int(float(spec[:-2]) * 1024 * 1024)
    if spec.endswith("KB"):
        return int(float(spec[:-2]) * 1024)
    if spec.endswith("B"):
        return int(spec[:-1])
    return int(spec)


# ---------------------------------------------------------------------------
# Binary format detection
# ---------------------------------------------------------------------------


def detect_format(path: Path) -> str:
    """Return 'wasm', 'macho', 'elf', or 'unknown'."""
    with open(path, "rb") as f:
        magic = f.read(8)
    if len(magic) < 4:
        return "unknown"
    if magic[:4] == WASM_MAGIC:
        return "wasm"
    # Mach-O: 0xFEEDFACE (32-bit), 0xFEEDFACF (64-bit), or fat binary 0xCAFEBABE
    if magic[:4] in (
        b"\xfe\xed\xfa\xce",
        b"\xfe\xed\xfa\xcf",
        b"\xce\xfa\xed\xfe",
        b"\xcf\xfa\xed\xfe",
        b"\xca\xfe\xba\xbe",
        b"\xbe\xba\xfe\xca",
        b"\xca\xfe\xba\xbf",
        b"\xbf\xba\xfe\xca",
    ):
        return "macho"
    # ELF
    if magic[:4] == b"\x7fELF":
        return "elf"
    return "unknown"


# ---------------------------------------------------------------------------
# Native binary analysis (Mach-O / ELF)
# ---------------------------------------------------------------------------


# The compiler inspector owns native decoding and lossless name transport.
# This projection owns disjoint file accounting and name-based attribution.
NATIVE_ATTRIBUTION_SCHEMA = 1


def _native_scanner(explicit: Path | None) -> tuple[Path, tuple[str, int] | None]:
    if explicit is not None:
        return explicit.absolute(), None
    from molt.cli.compiler_identity import (
        compiler_cargo_profile,
        installed_compiler_admission,
    )

    installed = installed_compiler_admission(REPO_ROOT)
    if installed is not None:
        return installed.compiler.binary, (
            installed.compiler.record["sha256"],
            installed.compiler.record["size"],
        )
    from molt.backend_executable_names import (
        DEFAULT_CODEGEN_BACKEND,
        backend_features_for_target,
    )
    from molt.cli.backend_execution import _backend_bin_path
    from molt.cli.backend_binary import _backend_fingerprint_path
    from molt.cli.runtime_fingerprints import (
        _read_runtime_fingerprint,
        _artifact_needs_rebuild,
    )

    profile = compiler_cargo_profile(os.environ)
    features = backend_features_for_target(
        is_wasm=False,
        is_luau_transpile=False,
        is_rust_transpile=False,
        codegen_backend=DEFAULT_CODEGEN_BACKEND,
    )
    selected = _backend_bin_path(REPO_ROOT, profile, features)
    receipt_path = _backend_fingerprint_path(REPO_ROOT, selected, profile)
    receipt = _read_runtime_fingerprint(receipt_path)
    if receipt is None or _artifact_needs_rebuild(selected, receipt, receipt):
        raise ValueError(
            "Native size analysis requires an admitted existing backend; prebuild it with molt internal-backend-build, or select --native-facts-scanner explicitly. Analysis never builds a compiler."
        )
    # Receipt parsing is owned by _read_runtime_fingerprint. Join its byte
    # identity to the captured executable immediately before execution; do not
    # re-read a second potentially different receipt or hash the image twice.
    content = receipt.get("artifact_content_identity", {})
    if "sha256" not in content or "size_bytes" not in content:
        raise ValueError("Native facts scanner lacks a published byte identity")
    return selected, (content["sha256"], content["size_bytes"])


def _native_facts_output(path: Path, scanner: Path | None) -> tuple[dict, dict]:
    selected, expected = _native_scanner(scanner)
    with stable_executable_probe(selected, label="native facts scanner") as (
        tool,
        identity,
    ):
        if expected is not None and (identity.sha256, identity.size) != expected:
            raise ValueError(
                "native facts scanner differs from its admitted compiler receipt"
            )
        command = [str(tool), "--scan-native-artifact-facts", str(path)]
        result = harness_memory_guard.guarded_completed_process(
            command,
            prefix="MOLT_BENCH",
            capture_output=True,
            text=True,
            timeout=60,
            limits=harness_memory_guard.limits_from_env("MOLT_BENCH"),
            errors="strict",
        )
        payload = loads_exact(result.stdout)
        if (
            not isinstance(payload, dict)
            or type(payload.get("schema_version")) is not int
            or payload["schema_version"] != 1
            or payload.get("ok") is not True
            or result.returncode
            or result.stderr.strip()
        ):
            error = (
                payload.get("error")
                if isinstance(payload, dict)
                else "invalid response"
            )
            raise ValueError(
                f"native facts scanner failed ({result.returncode}): {error}; {result.stderr.strip()}"
            )
        if set(payload) != {"schema_version", "ok", "facts"} or not isinstance(
            payload["facts"], dict
        ):
            raise ValueError(
                "native facts scanner returned an invalid success envelope"
            )
        fact = {
            "path": str(tool),
            "sha256": identity.sha256,
            "schema_version": 1,
            "arguments": command[1:],
        }
    return payload["facts"], fact


def _fact_int(value: object) -> int:
    if type(value) is not int or not 0 <= value < 2**64:
        raise ValueError("native facts expected an unsigned 64-bit integer")
    return value


def _fact_name(value: object) -> tuple[bytes, str | None]:
    if not isinstance(value, dict):
        raise ValueError("native facts name is not a tagged representation")
    if set(value) == {"utf8"} and isinstance(value["utf8"], str):
        return value["utf8"].encode("utf-8", errors="strict"), value["utf8"]
    if (
        set(value) == {"bytes"}
        and isinstance(value["bytes"], list)
        and all(type(v) is int and 0 <= v <= 255 for v in value["bytes"])
    ):
        raw = bytes(value["bytes"])
        try:
            raw.decode("utf-8", errors="strict")
        except UnicodeDecodeError:
            return raw, None
    raise ValueError("native facts name has invalid or noncanonical encoding")


def _native_decoded_facts(
    facts: dict, artifact: NativeArtifact, identity
) -> tuple[list[dict], list[dict]]:
    if (
        set(facts) != {"size", "sha256", "slices"}
        or _fact_int(facts["size"]) != identity.size
        or facts["sha256"] != identity.sha256
    ):
        raise ValueError("native facts are not bound to the inspected input bytes")
    slices = facts["slices"]
    if not isinstance(slices, list) or len(slices) != len(artifact.headers):
        raise ValueError("native facts slice census disagrees with admitted artifact")
    sections: list[dict] = []
    symbols: list[dict] = []
    for ordinal, (image, header) in enumerate(
        zip(slices, artifact.headers, strict=True)
    ):
        if not isinstance(image, dict) or set(image) != {
            "offset",
            "size",
            "format",
            "kind",
            "bits",
            "little_endian",
            "machine",
            "subtype",
            "sections",
            "symbols",
        }:
            raise ValueError("native facts image fields are invalid")
        expected_kind = {
            NativeFileKind.OBJECT: "object",
            NativeFileKind.EXECUTABLE: "executable",
            NativeFileKind.DYNAMIC_IMAGE: "dynamic",
            NativeFileKind.SHARED_LIBRARY: "dynamic",
        }.get(header.kind)
        if expected_kind is None:
            raise ValueError("native facts image kind is unsupported")
        if (
            _fact_int(image["offset"]) != header.offset
            or _fact_int(image["size"]) != header.size
            or image["format"] != header.object_format.value
            or image["kind"] != expected_kind
            or _fact_int(image["bits"]) != header.bits
            or type(image["little_endian"]) is not bool
            or image["little_endian"] != (header.byte_order == "little")
        ):
            raise ValueError(
                "native facts image disagrees with admitted artifact slice"
            )
        expected_subtype = (
            header.metadata.subtype
            if isinstance(header.metadata, MachOHeader)
            else None
        )
        subtype = None if image["subtype"] is None else _fact_int(image["subtype"])
        if _fact_int(image["machine"]) != header.machine or subtype != expected_subtype:
            raise ValueError("native facts target disagrees with admitted slice")
        if not isinstance(image["sections"], list) or not isinstance(
            image["symbols"], list
        ):
            raise ValueError("native facts tables must be arrays")
        index: dict[int, dict] = {}
        for row in image["sections"]:
            if not isinstance(row, dict) or set(row) != {
                "index",
                "name",
                "address",
                "declared_size",
                "file_range",
                "compression",
                "uncompressed_size",
            }:
                raise ValueError("native facts section fields are invalid")
            section_id = _fact_int(row["index"])
            if section_id in index:
                raise ValueError("native facts duplicated a section index")
            _raw, name = _fact_name(row["name"])
            physical = row["file_range"]
            if physical is not None:
                if not isinstance(physical, dict) or set(physical) != {
                    "offset",
                    "size",
                }:
                    raise ValueError("native section has invalid file range")
                offset, size = (
                    _fact_int(physical["offset"]),
                    _fact_int(physical["size"]),
                )
                if offset > header.size or size > header.size - offset:
                    raise ValueError("native section exceeds its admitted slice")
            else:
                offset, size = None, 0
            if row["compression"] not in {"None", "Unknown", "Zlib", "Zstandard"}:
                raise ValueError(
                    "native section has an unsupported compression representation"
                )
            section = {
                "slice": ordinal,
                "index": section_id,
                "name": row["name"],
                "display_name": name,
                "address": _fact_int(row["address"]),
                "declared_size": _fact_int(row["declared_size"]),
                "size": size,
                "file_backed": physical is not None,
                "offset": header.offset + offset if offset is not None else None,
                "compression": row["compression"],
                "compressed": row["compression"] != "None",
                "uncompressed_size": _fact_int(row["uncompressed_size"]),
            }
            sections.append(section)
            index[section_id] = section
        seen: set[tuple] = set()
        for row in image["symbols"]:
            required = {
                "name",
                "section_index",
                "address",
                "arm_thumb",
                "declared_size",
                "kind",
                "is_definition",
            }
            if (
                not isinstance(row, dict)
                or not required <= row.keys()
                or row.keys() - required - {"demangled_name"}
                or type(row["is_definition"]) is not bool
                or not isinstance(row["kind"], str)
            ):
                raise ValueError("native facts symbol fields are invalid")
            raw, name = _fact_name(row["name"])
            normalized = (
                name[1:]
                if image["format"] == "macho"
                and name is not None
                and name.startswith("_")
                else name
            )
            demangled = row.get("demangled_name", normalized)
            if demangled is not None and not isinstance(demangled, str):
                raise ValueError("native facts demangle must be text or absent")
            if name is None and demangled is not None:
                raise ValueError("non-UTF8 native name cannot acquire a demangle")
            address = _fact_int(row["address"])
            if type(row["arm_thumb"]) is not bool or (
                row["arm_thumb"]
                and not (
                    image["format"] == "elf"
                    and image["machine"] == 40
                    and row["kind"] == "Text"
                    and address & 1
                )
            ):
                raise ValueError("native Thumb tag disagrees with target/type/value")
            # Raw st_value survives in address. Only the producer's typed
            # EM_ARM/STT_FUNC tag authorizes removing the Thumb state bit.
            byte_address = address & ~1 if row["arm_thumb"] else address
            size = (
                None
                if row["declared_size"] is None
                else _fact_int(row["declared_size"])
            )
            if (image["format"] == "macho") != (size is None):
                raise ValueError(
                    "native symbol extent availability disagrees with object format"
                )
            section_id = (
                None
                if row["section_index"] is None
                else _fact_int(row["section_index"])
            )
            section = index.get(section_id)
            if section_id is not None and section is None:
                raise ValueError("native symbol references an absent section")
            key = (
                raw,
                row["kind"],
                section_id,
                address,
                row["arm_thumb"],
                size,
                row["is_definition"],
            )
            if key in seen:
                continue
            seen.add(key)
            offset = None
            if (
                section is not None
                and row["is_definition"]
                and row["kind"] in {"Text", "Data"}
                and size
                and not section["compressed"]
            ):
                relative = (
                    byte_address
                    if header.kind is NativeFileKind.OBJECT
                    else byte_address - section["address"]
                )
                if (
                    relative < 0
                    or relative > section["declared_size"]
                    or size > section["declared_size"] - relative
                ):
                    raise ValueError(
                        "native symbol exceeds its declared virtual section"
                    )
                if section["file_backed"]:
                    if relative > section["size"] or size > section["size"] - relative:
                        raise ValueError(
                            "native symbol exceeds its backed section extent"
                        )
                    offset = section["offset"] + relative
            symbols.append(
                {
                    "name": row["name"],
                    "display_name": name,
                    "normalized_name": normalized,
                    "demangled_name": demangled,
                    "kind": row["kind"],
                    "slice": ordinal,
                    "section_index": section_id,
                    "address": address,
                    "arm_thumb": row["arm_thumb"],
                    "declared_size": size,
                    "file_offset": offset,
                    "is_definition": row["is_definition"],
                }
            )
    return sections, symbols


def _native_name_authorities() -> tuple[set[str], set[str], list[dict]]:
    # Reuse Cargo's existing source-membership owner, including explicit lib
    # names, and the canonical ABI declarations. No prefix registry is copied.
    root_identity = stable_regular_file_identity(
        REPO_ROOT / "Cargo.toml", label="native naming workspace"
    )
    identities = [root_identity]
    crates: set[str] = set()
    for manifest in workspace_member_manifests(REPO_ROOT):
        identity, data = capture_stable_regular_file(
            manifest, label="native naming crate"
        )
        identities.append(identity)
        value = tomllib.loads(data.decode("utf-8"))
        library = value.get("lib", {})
        crates.add(library.get("name", value["package"]["name"].replace("-", "_")))
    declarations = REPO_ROOT / "runtime/molt-runtime/src/intrinsics/manifest.pyi"
    identity, data = capture_stable_regular_file(
        declarations, label="native naming ABI"
    )
    identities.append(identity)
    tree = ast.parse(data, filename=str(declarations))
    abi = {node.name for node in tree.body if isinstance(node, ast.FunctionDef)}
    for identity in identities:
        verify_stable_regular_file_identity(identity, label="native naming authority")
    return (
        crates,
        abi,
        [
            {"path": str(item.path.relative_to(REPO_ROOT)), "sha256": item.sha256}
            for item in identities
        ],
    )


def _categorise_symbol(
    name: str | None, demangled: str | None, *, crates: set[str], abi: set[str]
) -> str:
    """Name-based attribution, not proof of source ownership or reachability."""
    if name is None or demangled is None:
        return "unknown"
    if name in abi:
        return "runtime_abi"
    root, separator, _rest = demangled.partition("::")
    if separator and root in crates:
        return "crate:" + root
    return "unknown"


def _native_file_accounting(
    total: int, sections: list[dict], symbols: list[dict]
) -> dict:
    # A sorted endpoint sweep uses O(n) space, O(n log n) time, and never
    # adds alias extents or allocates storage proportional to file bytes.
    events: dict[int, list[tuple[str, str, int]]] = {0: [], total: []}
    for i, section in enumerate(sections):
        if section["file_backed"] and section["size"]:
            start, end = section["offset"], section["offset"] + section["size"]
            if start < 0 or end > total:
                raise ValueError("section outside file extent")
            events.setdefault(start, []).append(("section", str(i), 1))
            events.setdefault(end, []).append(("section", str(i), -1))
    for symbol in symbols:
        start, size = symbol["file_offset"], symbol["declared_size"]
        if start is not None and size:
            end = start + size
            if start < 0 or end > total:
                raise ValueError("symbol outside file extent")
            events.setdefault(start, []).append(("symbol", symbol["category"], 1))
            events.setdefault(end, []).append(("symbol", symbol["category"], -1))
    active = {"section": Counter(), "symbol": Counter()}
    totals: Counter[str] = Counter()
    covered = overlapping = conflicts = 0
    previous = 0
    for point, changes in sorted(events.items()):
        length = point - previous
        sections_active, categories = active["section"], active["symbol"]
        if not sections_active:
            category = "outside_sections"
        else:
            covered += length
            if len(sections_active) > 1:
                overlapping += length
            if len(categories) > 1:
                conflicts += length
            category = (
                next(iter(categories))
                if len(sections_active) == len(categories) == 1
                else "unknown"
            )
        totals[category] += length
        for lane, key, delta in changes:
            active[lane][key] += delta
            if active[lane][key] == 0:
                del active[lane][key]
        previous = point
    return {
        "schema_version": NATIVE_ATTRIBUTION_SCHEMA,
        "method": "disjoint-file-intervals-v1",
        "denominator": "file_bytes",
        "denominator_bytes": total,
        "file_backed_section_bytes": covered,
        "outside_sections_bytes": total - covered,
        "overlapping_section_bytes": overlapping,
        "conflicting_symbol_bytes": conflicts,
        "category_bytes": dict(
            sorted((key, value) for key, value in totals.items() if value)
        ),
    }


def validate_native_attribution(value: object, total_bytes: object) -> dict:
    """Reject retired or contradictory JSON before comparison/capsule use."""
    if (
        not isinstance(value, dict)
        or type(value.get("schema_version")) is not int
        or value["schema_version"] != NATIVE_ATTRIBUTION_SCHEMA
    ):
        raise ValueError(
            "native size input requires the current attribution schema; regenerate it"
        )
    expected_fields = {
        "schema_version",
        "method",
        "denominator",
        "denominator_bytes",
        "file_backed_section_bytes",
        "outside_sections_bytes",
        "overlapping_section_bytes",
        "conflicting_symbol_bytes",
        "category_bytes",
    }
    if set(value) != expected_fields:
        raise ValueError("native size attribution has an invalid schema shape")
    if (
        type(value.get("denominator_bytes")) is not int
        or type(total_bytes) is not int
        or total_bytes < 0
        or value.get("denominator_bytes") != total_bytes
        or value.get("denominator") != "file_bytes"
        or value.get("method") != "disjoint-file-intervals-v1"
    ):
        raise ValueError("native size attribution has an invalid denominator/method")
    counts = value.get("category_bytes")
    if (
        not isinstance(counts, dict)
        or any(
            not isinstance(k, str) or type(v) is not int or v < 0
            for k, v in counts.items()
        )
        or sum(counts.values()) != total_bytes
    ):
        raise ValueError("native size category intervals do not partition the file")
    for key in (
        "file_backed_section_bytes",
        "outside_sections_bytes",
        "overlapping_section_bytes",
        "conflicting_symbol_bytes",
    ):
        if type(value.get(key)) is not int or not 0 <= value[key] <= total_bytes:
            raise ValueError(f"native size attribution has invalid {key}")
    if (
        value["file_backed_section_bytes"] + value["outside_sections_bytes"]
        != total_bytes
        or counts.get("outside_sections", 0) != value["outside_sections_bytes"]
    ):
        raise ValueError("native size section coverage contradicts file accounting")
    if (
        max(value["overlapping_section_bytes"], value["conflicting_symbol_bytes"])
        > value["file_backed_section_bytes"]
    ):
        raise ValueError("native size overlap exceeds section coverage")
    if max(
        value["overlapping_section_bytes"], value["conflicting_symbol_bytes"]
    ) > counts.get("unknown", 0):
        raise ValueError("native size conflicting intervals must remain unknown")
    return dict(value)


def analyse_native(path: Path, *, scanner: Path | None = None) -> dict:
    """Inspect native images through the existing compiler facts command."""
    path = path.absolute()
    identity = stable_regular_file_identity(path, label="native size artifact")
    artifact = read_native_artifact(path)
    if any(
        header.object_format.value not in {"elf", "macho"}
        for header in artifact.headers
    ):
        raise ValueError("native size attribution supports ELF and Mach-O only")
    facts, inspector = _native_facts_output(path, scanner)
    sections, symbols = _native_decoded_facts(facts, artifact, identity)
    crates, abi, name_authorities = _native_name_authorities()
    counts: Counter[str] = Counter()
    for symbol in symbols:
        symbol["category"] = _categorise_symbol(
            symbol["normalized_name"], symbol["demangled_name"], crates=crates, abi=abi
        )
        counts[symbol["category"]] += 1
    accounting = _native_file_accounting(identity.size, sections, symbols)
    validate_native_attribution(accounting, identity.size)
    # No observation is published if the file changed during inspection.
    verify_stable_regular_file_identity(
        identity, label="native size artifact", hash_content=True
    )
    symbols.sort(
        key=lambda s: (
            -(s["declared_size"] or 0),
            s["slice"],
            s["section_index"] if s["section_index"] is not None else -1,
            s["address"],
            json.dumps(s["name"], sort_keys=True),
        )
    )
    return {
        "format": "native",
        "path": str(path),
        "total_bytes": identity.size,
        "sha256": identity.sha256,
        "object_format": artifact.headers[0].object_format.value,
        "universal": artifact.universal,
        "attribution": accounting,
        "native_sections": sections,
        "symbols": symbols,
        "native_slices": [
            {
                key: value
                for key, value in image.items()
                if key not in {"sections", "symbols"}
            }
            for image in facts["slices"]
        ],
        "symbol_extent_status": "unavailable-macho-nlist"
        if artifact.headers[0].object_format.value == "macho"
        else "declared-elf-extents",
        "symbol_category_counts": dict(sorted(counts.items())),
        "tools": [inspector],
        "name_authorities": name_authorities,
    }


def print_native_report(analysis: dict) -> None:
    total = analysis["total_bytes"]
    accounting = validate_native_attribution(analysis["attribution"], total)
    print(f"Binary Size Analysis -- {analysis['path']}")
    print(f"Total file size: {_fmt_bytes(total)} ({total:,} bytes)")
    print("Disjoint file-byte attribution (name-based; unknown is explicit):")
    for category, size in sorted(
        accounting["category_bytes"].items(), key=lambda item: (-item[1], item[0])
    ):
        print(f"  {category:<32} {_fmt_bytes(size):>12} {_pct(size, total):>8}")
    print(f"Symbol extents: {analysis['symbol_extent_status']}")
    print("Top 50 declared symbol sizes (aliases overlap; these are not additive):")
    for symbol in analysis["symbols"][:50]:
        size = symbol["declared_size"]
        label = "unavailable" if size is None else _fmt_bytes(size)
        name = (
            symbol["demangled_name"]
            if symbol["demangled_name"] is not None
            else symbol["name"]
        )
        print(
            f"  {label:>12} {symbol['category']:<30} {json.dumps(name, ensure_ascii=False)}"
        )


# ---------------------------------------------------------------------------
# WASM binary analysis
# ---------------------------------------------------------------------------


def analyse_wasm(path: Path) -> dict:
    total_bytes = path.stat().st_size
    sections: list[WasmSectionSpan] = read_wasm_section_spans(path)
    metrics = read_wasm_code_metrics(path)
    func_count = metrics.defined_function_count
    code_size = metrics.code_section_size
    avg_func_size = code_size // func_count if func_count > 0 else 0

    by_type: dict[str, int] = {}
    for sec in sections:
        key = sec.name
        if sec.custom_name:
            key = f"custom:{sec.custom_name}"
        by_type[key] = by_type.get(key, 0) + sec.size

    return {
        "format": "wasm",
        "path": str(path),
        "total_bytes": total_bytes,
        "sections": sections,
        "by_type": by_type,
        "function_count": func_count,
        "code_size": code_size,
        "avg_function_size": avg_func_size,
    }


def print_wasm_report(analysis: dict) -> None:
    total = analysis["total_bytes"]
    by_type = analysis["by_type"]
    func_count = analysis["function_count"]
    avg_func = analysis["avg_function_size"]

    print("=" * 76)
    print(f"WASM Binary Size Analysis — {analysis['path']}")
    print(f"Total file size: {_fmt_bytes(total)} ({total:,} bytes)")
    print("=" * 76)

    print(f"\n{'Section':<35s} {'Size':>12s}  {'%':>6s}")
    print("-" * 57)
    for name, size in sorted(by_type.items(), key=lambda kv: -kv[1]):
        pct = size / total * 100 if total > 0 else 0
        bar = "#" * int(pct / 2)
        print(f"  {name:<33s} {_fmt_bytes(size):>12s}  {pct:>5.1f}%  {bar}")

    accounted = sum(s.size for s in analysis["sections"])
    overhead = total - accounted
    if overhead > 0:
        print(
            f"  {'<headers/padding>':<33s} {_fmt_bytes(overhead):>12s}  {overhead / total * 100:>5.1f}%"
        )

    print("-" * 57)
    print(f"  {'TOTAL':<33s} {_fmt_bytes(total):>12s}")

    print("\n--- Code Section Details ---")
    print(f"  Function count:        {func_count:,}")
    print(f"  Code section size:     {_fmt_bytes(analysis['code_size'])}")
    print(f"  Avg function size:     {_fmt_bytes(avg_func)}")

    # Runtime vs user code estimate based on code vs data ratio
    code_total = by_type.get("code", 0)
    data_total = by_type.get("data", 0)
    custom_total = sum(v for k, v in by_type.items() if k.startswith("custom:"))
    structural = total - code_total - data_total - custom_total

    print("\n--- Estimated Contribution ---")
    print(
        f"  Code (functions):      {_fmt_bytes(code_total):>12s}  {_pct(code_total, total)}"
    )
    print(
        f"  Data (constants/heap): {_fmt_bytes(data_total):>12s}  {_pct(data_total, total)}"
    )
    print(
        f"  Custom sections:       {_fmt_bytes(custom_total):>12s}  {_pct(custom_total, total)}"
    )
    print(
        f"  Structural/metadata:   {_fmt_bytes(structural):>12s}  {_pct(structural, total)}"
    )
    print()


# ---------------------------------------------------------------------------
# Comparison mode
# ---------------------------------------------------------------------------


def compare_binaries(
    path_a: Path, path_b: Path, *, scanner: Path | None = None
) -> dict:
    """Compare two binaries and compute deltas."""
    fmt_a = detect_format(path_a)
    fmt_b = detect_format(path_b)

    size_a = path_a.stat().st_size
    size_b = path_b.stat().st_size
    delta = size_b - size_a

    result: dict = {
        "before": str(path_a),
        "after": str(path_b),
        "before_bytes": size_a,
        "after_bytes": size_b,
        "delta_bytes": delta,
        "delta_pct": (delta / size_a * 100) if size_a > 0 else 0,
    }

    # If both are same format, do deeper comparison
    if fmt_a == fmt_b == "wasm":
        a = analyse_wasm(path_a)
        b = analyse_wasm(path_b)

        section_delta: dict[str, dict] = {}
        all_keys = set(a["by_type"]) | set(b["by_type"])
        for key in sorted(all_keys):
            sa = a["by_type"].get(key, 0)
            sb = b["by_type"].get(key, 0)
            section_delta[key] = {"before": sa, "after": sb, "delta": sb - sa}

        result["section_deltas"] = section_delta
        result["function_count_before"] = a["function_count"]
        result["function_count_after"] = b["function_count"]

    elif fmt_a in ("macho", "elf") and fmt_b in ("macho", "elf"):
        a = analyse_native(path_a, scanner=scanner)
        b = analyse_native(path_b, scanner=scanner)

        # Use the sizes bound to each completed inspection, not the earlier
        # exploratory stat if the path changed before admission.
        result.update(
            before_bytes=a["total_bytes"],
            after_bytes=b["total_bytes"],
            delta_bytes=b["total_bytes"] - a["total_bytes"],
            delta_pct=((b["total_bytes"] - a["total_bytes"]) / a["total_bytes"] * 100)
            if a["total_bytes"]
            else 0,
        )
        result["attribution_before"] = a["attribution"]
        result["attribution_after"] = b["attribution"]
        result["symbol_extent_status_before"] = a["symbol_extent_status"]
        result["symbol_extent_status_after"] = b["symbol_extent_status"]
        context_fields = (
            "sha256",
            "object_format",
            "universal",
            "native_slices",
            "symbol_extent_status",
            "tools",
            "name_authorities",
        )
        result["native_context_before"] = {key: a[key] for key in context_fields}
        result["native_context_after"] = {key: b[key] for key in context_fields}
        reasons = []
        if a["symbol_extent_status"] != b["symbol_extent_status"]:
            reasons.append("symbol extent capabilities differ")
        if any(
            a["attribution"][key] != b["attribution"][key]
            for key in ("schema_version", "method", "denominator")
        ):
            reasons.append("attribution methods differ")
        if sorted(
            (item["path"], item["sha256"]) for item in a["name_authorities"]
        ) != sorted((item["path"], item["sha256"]) for item in b["name_authorities"]):
            reasons.append("name authorities differ")
        if [(item["sha256"], item["schema_version"]) for item in a["tools"]] != [
            (item["sha256"], item["schema_version"]) for item in b["tools"]
        ]:
            reasons.append("inspector identities differ")
        result["category_comparison"] = {"available": not reasons, "reasons": reasons}
        if not reasons:
            before = a["attribution"]["category_bytes"]
            after = b["attribution"]["category_bytes"]
            result["category_deltas"] = {
                category: {
                    "display": category,
                    "before": before.get(category, 0),
                    "after": after.get(category, 0),
                    "delta": after.get(category, 0) - before.get(category, 0),
                }
                for category in sorted(before.keys() | after.keys())
            }

    return result


def print_comparison(comp: dict) -> None:
    delta = comp["delta_bytes"]
    sign = "+" if delta >= 0 else ""

    print("=" * 76)
    print("Binary Size Comparison")
    print("=" * 76)
    print(f"  Before: {comp['before']:<50s} {_fmt_bytes(comp['before_bytes'])}")
    print(f"  After:  {comp['after']:<50s} {_fmt_bytes(comp['after_bytes'])}")
    print(f"  Delta:  {sign}{_fmt_bytes(delta)} ({sign}{comp['delta_pct']:.1f}%)")
    print()

    if "section_deltas" in comp:
        print(f"{'Section':<30s} {'Before':>10s}  {'After':>10s}  {'Delta':>12s}")
        print("-" * 66)
        for key, d in sorted(
            comp["section_deltas"].items(), key=lambda kv: -abs(kv[1]["delta"])
        ):
            dd = d["delta"]
            s = "+" if dd >= 0 else ""
            print(
                f"  {key:<28s} {_fmt_bytes(d['before']):>10s}  {_fmt_bytes(d['after']):>10s}  {s}{_fmt_bytes(dd):>10s}"
            )
        print()
        print(
            f"  Functions: {comp.get('function_count_before', '?')} -> {comp.get('function_count_after', '?')}"
        )

    if "category_comparison" in comp:
        for label in ("before", "after"):
            context = comp[f"native_context_{label}"]
            print(
                f"  {label.title()}: {context['object_format']}, {context['symbol_extent_status']}"
            )
            print(f"    SHA256: {context['sha256']}")
            print(f"    Slices: {json.dumps(context['native_slices'], sort_keys=True)}")
        comparison = comp["category_comparison"]
        if not comparison["available"]:
            print("  Category deltas unavailable: " + "; ".join(comparison["reasons"]))
        print()

    if "category_deltas" in comp:
        print(f"{'Category':<30s} {'Before':>10s}  {'After':>10s}  {'Delta':>12s}")
        print("-" * 66)
        for key, d in sorted(
            comp["category_deltas"].items(), key=lambda kv: -abs(kv[1]["delta"])
        ):
            dd = d["delta"]
            s = "+" if dd >= 0 else ""
            print(
                f"  {d['display']:<28s} {_fmt_bytes(d['before']):>10s}  {_fmt_bytes(d['after']):>10s}  {s}{_fmt_bytes(dd):>10s}"
            )

    print()


# ---------------------------------------------------------------------------
# JSON output
# ---------------------------------------------------------------------------


def to_json(analysis: dict) -> dict:
    """Convert an analysis result to JSON-serialisable dict."""
    out = dict(analysis)
    # Remove non-serialisable objects
    if "symbols" in out:
        out["top_50_symbols"] = out.pop("symbols")[:50]
    if "sections" in out:
        out["sections"] = [
            {"id": s.id, "name": s.name, "size": s.size, "custom_name": s.custom_name}
            for s in out.pop("sections")
        ]
    # segments are already JSON-serialisable dicts
    return out


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Analyse binary size breakdown for Molt native and WASM binaries",
    )
    parser.add_argument("binary", type=Path, nargs="?", help="Path to binary file")
    parser.add_argument(
        "--compare",
        nargs=2,
        metavar=("BEFORE", "AFTER"),
        help="Compare two binaries and show deltas",
    )
    parser.add_argument(
        "--json", action="store_true", dest="json_output", help="Output as JSON"
    )
    parser.add_argument(
        "--budget",
        type=str,
        default=None,
        help=f"Size budget (e.g. '30MB'). Default: {DEFAULT_BUDGET_NATIVE_MB:.0f}MB native, {DEFAULT_BUDGET_WASM_MB:.0f}MB WASM",
    )
    parser.add_argument(
        "--native-facts-scanner",
        type=Path,
        help="Existing molt-backend inspector; never auto-built",
    )
    args = parser.parse_args()

    # Compare mode
    if args.compare:
        path_a, path_b = Path(args.compare[0]), Path(args.compare[1])
        for p in (path_a, path_b):
            if not p.is_file():
                print(f"ERROR: {p} not found", file=sys.stderr)
                sys.exit(1)
        comp = compare_binaries(path_a, path_b, scanner=args.native_facts_scanner)
        if args.json_output:
            print(json.dumps(comp, indent=2))
        else:
            print_comparison(comp)
        return

    # Single binary mode
    if args.binary is None:
        parser.error("binary path is required (unless using --compare)")

    if not args.binary.is_file():
        print(f"ERROR: {args.binary} not found", file=sys.stderr)
        sys.exit(1)

    fmt = detect_format(args.binary)

    if fmt == "wasm":
        analysis = analyse_wasm(args.binary)
        if args.json_output:
            print(json.dumps(to_json(analysis), indent=2))
        else:
            print_wasm_report(analysis)
    elif fmt in ("macho", "elf"):
        analysis = analyse_native(args.binary, scanner=args.native_facts_scanner)
        if args.json_output:
            print(json.dumps(to_json(analysis), indent=2))
        else:
            print_native_report(analysis)
    else:
        print(f"ERROR: Unrecognised binary format for {args.binary}", file=sys.stderr)
        sys.exit(1)

    # Budget check
    total = analysis["total_bytes"]
    default_mb = DEFAULT_BUDGET_WASM_MB if fmt == "wasm" else DEFAULT_BUDGET_NATIVE_MB
    budget_bytes = int(default_mb * 1024 * 1024)
    if args.budget:
        budget_bytes = _parse_size_spec(args.budget)

    if total > budget_bytes:
        over = total - budget_bytes
        print(
            f"BUDGET EXCEEDED: {_fmt_bytes(total)} > {_fmt_bytes(budget_bytes)} "
            f"(over by {_fmt_bytes(over)})",
            file=sys.stderr if args.json_output else sys.stdout,
        )
        sys.exit(1)
    else:
        remaining = budget_bytes - total
        print(
            f"Budget OK: {_fmt_bytes(total)} / {_fmt_bytes(budget_bytes)} "
            f"({_fmt_bytes(remaining)} remaining)",
            file=sys.stderr if args.json_output else sys.stdout,
        )


if __name__ == "__main__":
    main()
