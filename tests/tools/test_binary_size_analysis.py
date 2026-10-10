"""Independent interval/layout controls; actual LLVM bytes exercise the Rust CLI
in runtime/molt-backend/tests/native_artifact_facts.rs. These synthetic fact
records are explicitly authored receiver negatives, not claimed tool captures.
"""

from __future__ import annotations
from copy import deepcopy
from dataclasses import replace
import json
from pathlib import Path
import sys
from types import SimpleNamespace
import pytest

from molt.native_artifact_header import (
    ElfHeader,
    MachOHeader,
    NativeArtifact,
    NativeFileKind,
    NativeHeader,
)
from molt.native_target_shape import NativeObjectFormat
from tools import binary_size_analysis as size

ROOT = Path(__file__).resolve().parents[2]


def _case(macho=False):
    identity = SimpleNamespace(size=256, sha256="a" * 64)
    header = NativeHeader(
        NativeObjectFormat.MACHO if macho else NativeObjectFormat.ELF,
        0x100000C if macho else 62,
        64,
        "little",
        NativeFileKind.EXECUTABLE,
        0,
        256,
        MachOHeader(0, 32, 0, 0) if macho else ElfHeader(0, 0, 0, 0, 0, 0, 4),
    )
    sections = [
        {
            "index": 1,
            "name": {"utf8": ".text"},
            "address": 4096,
            "declared_size": 16,
            "file_range": {"offset": 64, "size": 16},
            "compression": "None",
            "uncompressed_size": 16,
        },
        {
            "index": 2,
            "name": {"utf8": ".data"},
            "address": 8192,
            "declared_size": 8,
            "file_range": {"offset": 96, "size": 8},
            "compression": "None",
            "uncompressed_size": 8,
        },
        {
            "index": 3,
            "name": {"utf8": ".bss"},
            "address": 36864,
            "declared_size": 4096,
            "file_range": None,
            "compression": "None",
            "uncompressed_size": 0,
        },
    ]
    symbols = [
        {
            "name": {"utf8": "_RNvCsgrakSpcflzr_12molt_runtime19molt_call_indirect0"},
            "demangled_name": "molt_runtime::molt_call_indirect0",
            "section_index": 1,
            "address": 4096,
            "declared_size": 8,
            "kind": "Text",
            "arm_thumb": False,
            "is_definition": True,
        },
        {
            "name": {"utf8": "runtime_alias"},
            "section_index": 1,
            "address": 4096,
            "declared_size": 8,
            "kind": "Text",
            "arm_thumb": False,
            "is_definition": True,
        },
        {
            "name": {"utf8": "user function"},
            "section_index": 1,
            "address": 4100,
            "declared_size": 8,
            "kind": "Text",
            "arm_thumb": False,
            "is_definition": True,
        },
    ]
    if macho:
        for symbol in symbols:
            symbol["declared_size"] = None
            symbol["name"]["utf8"] = "_" + symbol["name"]["utf8"]
    facts = {
        "size": 256,
        "sha256": identity.sha256,
        "slices": [
            {
                "offset": 0,
                "size": 256,
                "format": header.object_format.value,
                "kind": "executable",
                "bits": 64,
                "little_endian": True,
                "machine": header.machine,
                "subtype": 0 if macho else None,
                "sections": sections,
                "symbols": symbols,
            }
        ],
    }
    return facts, NativeArtifact((header,)), identity


def test_independent_alias_conflict_and_bss_denominator():
    facts, artifact, identity = _case()
    sections, symbols = size._native_decoded_facts(facts, artifact, identity)
    assert symbols[2]["display_name"] == "user function"
    for symbol, category in zip(
        symbols, ["crate:molt_runtime", "crate:molt_runtime", "unknown"], strict=True
    ):
        symbol["category"] = category
    result = size._native_file_accounting(256, sections, symbols)
    assert result["category_bytes"] == {
        "crate:molt_runtime": 4,
        "unknown": 20,
        "outside_sections": 232,
    }
    assert result["file_backed_section_bytes"] == 24
    assert result["conflicting_symbol_bytes"] == 4
    assert result["overlapping_section_bytes"] == 0
    assert size._native_file_accounting(256, sections[::-1], symbols[::-1]) == result
    assert size.validate_native_attribution(result, 256) == result


def test_macho_keeps_name_and_section_facts_without_invented_lengths():
    facts, artifact, identity = _case(True)
    sections, symbols = size._native_decoded_facts(facts, artifact, identity)
    assert symbols[0]["normalized_name"].startswith("_R")
    assert symbols[0]["name"]["utf8"].startswith("__R")
    assert all(s["declared_size"] is None and s["file_offset"] is None for s in symbols)
    for symbol in symbols:
        symbol["category"] = "crate:molt_runtime"
    assert size._native_file_accounting(256, sections, symbols)["category_bytes"] == {
        "outside_sections": 232,
        "unknown": 24,
    }


def test_compressed_storage_smaller_than_symbol_is_not_virtual_mapping():
    facts, artifact, identity = _case()
    row = facts["slices"][0]["sections"][0]
    row.update(
        compression="Zlib",
        declared_size=4,
        uncompressed_size=1024,
        file_range={"offset": 64, "size": 4},
    )
    facts["slices"][0]["symbols"][0]["declared_size"] = 512
    sections, symbols = size._native_decoded_facts(facts, artifact, identity)
    assert sections[0]["size"] == 4
    assert all(s["file_offset"] is None for s in symbols)


def test_fat_slice_order_and_disjoint_offsets():
    facts, artifact, identity = _case(True)
    facts["size"] = 768
    identity.size = 768
    one = deepcopy(facts["slices"][0])
    one["offset"] = 256
    two = deepcopy(one)
    two["offset"] = 512
    facts["slices"] = [one, two]
    header = artifact.headers[0]
    artifact = NativeArtifact(
        (replace(header, offset=256), replace(header, offset=512)), universal=True
    )
    sections, symbols = size._native_decoded_facts(facts, artifact, identity)
    assert [s["offset"] for s in sections if s["file_backed"]] == [320, 352, 576, 608]
    for s in symbols:
        s["category"] = "unknown"
    assert size._native_file_accounting(768, sections, symbols)["category_bytes"] == {
        "outside_sections": 720,
        "unknown": 48,
    }
    facts["slices"].reverse()
    with pytest.raises(ValueError, match="slice"):
        size._native_decoded_facts(facts, artifact, identity)


@pytest.mark.parametrize(
    "mutation",
    [
        lambda f: f.update(sha256="b" * 64),
        lambda f: f["slices"][0].update(bits=True),
        lambda f: f["slices"][0].update(machine=183),
        lambda f: f["slices"][0].update(subtype=0),
        lambda f: f["slices"][0].update(size=257),
        lambda f: f["slices"][0]["sections"][0].update(
            file_range={"offset": 256, "size": 1}
        ),
        lambda f: f["slices"][0]["symbols"][0].update(declared_size=17),
        lambda f: f["slices"][0]["symbols"][0].update(section_index=99),
        lambda f: f["slices"][0]["symbols"][0].update(declared_size=None),
        lambda f: f["slices"][0]["symbols"][0].update(name={"bytes": [65]}),
        lambda f: f["slices"][0]["symbols"][0].update(name={"bytes": [256]}),
        lambda f: f["slices"][0]["symbols"][0].update(name={"bytes": [255]}),
    ],
)
def test_receiver_rejects_unbound_or_incoherent_facts(mutation):
    facts, artifact, identity = _case()
    mutation(facts)
    with pytest.raises(ValueError):
        size._native_decoded_facts(facts, artifact, identity)


def test_lossless_names_and_dynamic_duplicates():
    facts, artifact, identity = _case()
    rows = facts["slices"][0]["symbols"]
    rows[1]["name"] = {"utf8": 'name\n  }\nSymbol {\n braces "'}
    rows[2]["name"] = {"bytes": [255, 10, 123, 125]}
    rows.append(deepcopy(rows[0]))
    sections, symbols = size._native_decoded_facts(facts, artifact, identity)
    assert len(symbols) == 3
    assert symbols[1]["name"] == rows[1]["name"]
    assert symbols[2]["normalized_name"] is None
    assert symbols[2]["name"] == rows[2]["name"]


def test_overlapping_sections_and_many_aliases_never_multiply_file_bytes():
    sections = [
        {"offset": 10, "size": 20, "file_backed": True},
        {"offset": 20, "size": 20, "file_backed": True},
    ]
    symbols = [
        {"file_offset": 10, "declared_size": 30, "category": "runtime_abi"}
    ] * 100
    result = size._native_file_accounting(10**12, sections, symbols)
    assert result["overlapping_section_bytes"] == 10
    assert result["category_bytes"] == {
        "outside_sections": 10**12 - 30,
        "runtime_abi": 20,
        "unknown": 10,
    }


@pytest.mark.parametrize(
    "raw,demangled,expected",
    [
        (
            "_RNvCsgrakSpcflzr_12molt_runtime19molt_call_indirect0",
            "molt_runtime::molt_call_indirect0",
            "crate:molt_runtime",
        ),
        (
            "_ZN12molt_runtime3foo17h0123456789abcdefE",
            "molt_runtime::foo",
            "crate:molt_runtime",
        ),
        ("molt_main", "molt_main", "unknown"),
        ("molt_user_function", "molt_user_function", "unknown"),
        ("molt_len", "molt_len", "runtime_abi"),
        ("x", "serde::de::read", "unknown"),
        ("x", "molt_runtime_fake::f", "unknown"),
        (None, None, "unknown"),
    ],
)
def test_classification_uses_exact_authorities(raw, demangled, expected):
    assert (
        size._categorise_symbol(
            raw, demangled, crates={"molt_runtime"}, abi={"molt_len"}
        )
        == expected
    )


def test_retired_or_contradictory_native_json_refused():
    with pytest.raises(ValueError, match="regenerate"):
        size.validate_native_attribution(None, 256)
    good = size._native_file_accounting(256, [], [])
    for field, value in [
        ("denominator", "symbol_sum"),
        ("denominator_bytes", 257),
        ("category_bytes", {"unknown": 999}),
        ("outside_sections_bytes", 0),
        ("overlapping_section_bytes", 1),
    ]:
        bad = deepcopy(good)
        bad[field] = value
        with pytest.raises(ValueError):
            size.validate_native_attribution(bad, 256)


def test_captured_fixture_integrity_and_independent_format_headers():
    import hashlib
    from molt.native_artifact_header import native_artifact_from_bytes

    payload = json.loads(
        (Path(__file__).parent / "fixtures/native_size_facts.json").read_text(
            encoding="utf-8"
        )
    )
    for case in payload["cases"]:
        raw = bytes(case["artifact_bytes"])
        assert hashlib.sha256(raw).hexdigest() == case["artifact_sha256"]
        assert (
            hashlib.sha256(case["readobj_stdout"].encode()).hexdigest()
            == case["readobj_sha256"]
        )
        artifact = native_artifact_from_bytes(raw)
        assert len(artifact.headers) == 1
        assert artifact.headers[0].bits == 64
        assert artifact.headers[0].kind is NativeFileKind.OBJECT


def test_explicit_scanner_selection_never_discovers_or_builds(monkeypatch, tmp_path):
    from molt.cli import compiler_identity

    def forbidden(*args, **kwargs):
        pytest.fail("explicit inspection must not discover or build a compiler")

    monkeypatch.setattr(compiler_identity, "installed_compiler_admission", forbidden)
    selected = tmp_path / "existing-inspector"
    assert size._native_scanner(selected) == (selected, None)


def test_installed_scanner_preserves_admitted_image_fact(monkeypatch, tmp_path):
    from molt.cli import compiler_identity

    selected = tmp_path / "installed-backend"
    admission = SimpleNamespace(
        compiler=SimpleNamespace(
            binary=selected, record={"sha256": "b" * 64, "size": 37}
        )
    )
    monkeypatch.setattr(
        compiler_identity, "installed_compiler_admission", lambda root: admission
    )
    assert size._native_scanner(None) == (selected, ("b" * 64, 37))

    def damaged(root):
        raise ValueError("installed compiler source damaged")

    monkeypatch.setattr(compiler_identity, "installed_compiler_admission", damaged)
    with pytest.raises(ValueError, match="source damaged"):
        size._native_scanner(None)


def test_development_scanner_uses_published_byte_receipt(monkeypatch, tmp_path):
    from molt.cli import (
        compiler_identity,
        backend_execution,
        backend_binary,
        runtime_fingerprints,
    )

    selected = tmp_path / "published-backend"
    receipt_path = tmp_path / "published-receipt"
    content = {"schema": "molt.artifact-bytes.v1", "sha256": "c" * 64, "size_bytes": 73}
    receipt = {"artifact_content_identity": content}
    seen = []
    monkeypatch.setattr(
        compiler_identity, "installed_compiler_admission", lambda root: None
    )
    monkeypatch.setattr(
        compiler_identity, "compiler_cargo_profile", lambda environ: "dev"
    )
    monkeypatch.setattr(backend_execution, "_backend_bin_path", lambda *args: selected)
    monkeypatch.setattr(
        backend_binary, "_backend_fingerprint_path", lambda *args: receipt_path
    )

    def read(path):
        assert path == receipt_path
        seen.append(path)
        return receipt

    monkeypatch.setattr(runtime_fingerprints, "_read_runtime_fingerprint", read)
    monkeypatch.setattr(
        runtime_fingerprints,
        "_artifact_needs_rebuild",
        lambda artifact, expected, actual: False,
    )
    assert size._native_scanner(None) == (selected, ("c" * 64, 73))
    assert seen == [receipt_path]
    receipt.clear()
    with pytest.raises(ValueError, match="byte identity"):
        size._native_scanner(None)
    monkeypatch.setattr(
        runtime_fingerprints, "_read_runtime_fingerprint", lambda path: None
    )
    with pytest.raises(ValueError, match="never builds"):
        size._native_scanner(None)


def _copied_scanner(tmp_path):
    # The real stable-executable owner validates this copied host executable.
    # Tests replace only the process boundary, never its image custody.
    import shutil

    selected = tmp_path / ("scanner.exe" if sys.platform == "win32" else "scanner")
    shutil.copy2(sys.executable, selected)
    identity = size.stable_regular_file_identity(selected, label="scanner test")
    return selected, identity


def test_scanner_receipt_mismatch_refuses_before_any_process(monkeypatch, tmp_path):
    selected, identity = _copied_scanner(tmp_path)
    monkeypatch.setattr(
        size, "_native_scanner", lambda explicit: (selected, ("0" * 64, identity.size))
    )

    def forbidden(*args, **kwargs):
        pytest.fail("unadmitted image must never execute")

    monkeypatch.setattr(
        size.harness_memory_guard, "guarded_completed_process", forbidden
    )
    with pytest.raises(ValueError, match="compiler receipt"):
        size._native_facts_output(tmp_path / "artifact", None)


@pytest.mark.parametrize("damage", ["none", "image", "entrypoint"])
def test_scanner_actual_image_fence_and_command(monkeypatch, tmp_path, damage):
    selected, identity = _copied_scanner(tmp_path)
    artifact = tmp_path / "input"
    monkeypatch.setattr(
        size,
        "_native_scanner",
        lambda explicit: (selected, (identity.sha256, identity.size)),
    )

    def inspect(command, **kwargs):
        assert command == [str(selected), "--scan-native-artifact-facts", str(artifact)]
        assert kwargs["capture_output"] and kwargs["text"]
        if damage == "image":
            with selected.open("ab") as out:
                out.write(b"changed")
        if damage == "entrypoint":
            replacement = selected.with_name("replacement")
            replacement.write_bytes(selected.read_bytes())
            replacement.chmod(selected.stat().st_mode)
            replacement.replace(selected)
        return SimpleNamespace(
            returncode=0,
            stderr="",
            stdout=json.dumps({"schema_version": 1, "ok": True, "facts": {}}),
        )

    monkeypatch.setattr(size.harness_memory_guard, "guarded_completed_process", inspect)
    if damage != "none":
        with pytest.raises(ValueError):
            size._native_facts_output(artifact, None)
    else:
        facts, tool = size._native_facts_output(artifact, None)
        assert facts == {} and tool["sha256"] == identity.sha256


@pytest.mark.parametrize(
    "result",
    [
        SimpleNamespace(
            returncode=0,
            stderr="warning",
            stdout='{"schema_version":1,"ok":true,"facts":{}}',
        ),
        SimpleNamespace(
            returncode=2,
            stderr="",
            stdout='{"schema_version":1,"ok":false,"error":"unsupported"}',
        ),
        SimpleNamespace(
            returncode=0,
            stderr="",
            stdout='{"schema_version":1,"ok":true,"facts":{},"facts":{}}',
        ),
        SimpleNamespace(
            returncode=0,
            stderr="",
            stdout='{"schema_version":true,"ok":true,"facts":{}}',
        ),
    ],
)
def test_scanner_failure_or_noncanonical_envelope_is_not_success(
    monkeypatch, tmp_path, result
):
    selected, identity = _copied_scanner(tmp_path)
    monkeypatch.setattr(size, "_native_scanner", lambda explicit: (selected, None))
    monkeypatch.setattr(
        size.harness_memory_guard,
        "guarded_completed_process",
        lambda *args, **kwargs: result,
    )
    with pytest.raises(ValueError):
        size._native_facts_output(tmp_path / "input", selected)


def test_native_comparison_uses_bound_sizes_and_denominators(monkeypatch, tmp_path):
    before = tmp_path / "before"
    after = tmp_path / "after"
    before.write_bytes(b"before")
    after.write_bytes(b"after")
    monkeypatch.setattr(size, "detect_format", lambda path: "elf")
    selected = tmp_path / "scanner"

    def admitted(path, *, scanner):
        assert scanner == selected
        count = 256 if path == before else 512
        return {
            "total_bytes": count,
            "attribution": size._native_file_accounting(count, [], []),
            "symbol_extent_status": "declared-elf-extents",
            "sha256": "a" * 64 if path == before else "b" * 64,
            "object_format": "elf",
            "universal": False,
            "native_slices": [{"offset": 0, "size": count}],
            "tools": [{"sha256": "c" * 64, "schema_version": 1}],
            "name_authorities": [{"path": "Cargo.toml", "sha256": "d" * 64}],
        }

    monkeypatch.setattr(size, "analyse_native", admitted)
    result = size.compare_binaries(before, after, scanner=selected)
    assert (
        result["before_bytes"],
        result["after_bytes"],
        result["delta_bytes"],
        result["delta_pct"],
    ) == (256, 512, 256, 100)
    assert result["category_deltas"] == {
        "outside_sections": {
            "display": "outside_sections",
            "before": 256,
            "after": 512,
            "delta": 256,
        }
    }


@pytest.mark.parametrize(
    "machine,kind,tag,value,expected",
    [
        (40, "Text", True, 4097, 64),
        (40, "Text", False, 4096, 64),
        (40, "Data", False, 4097, 65),
        (183, "Text", False, 4097, 65),
    ],
)
@pytest.mark.parametrize("relocatable", [False, True])
def test_thumb_mapping_preserves_raw_value_and_odd_non_thumb_offsets(
    machine, kind, tag, value, expected, relocatable
):
    facts, artifact, identity = _case()
    image = facts["slices"][0]
    image["machine"] = machine
    image["bits"] = 32 if machine == 40 else 64
    native_kind = NativeFileKind.OBJECT if relocatable else NativeFileKind.EXECUTABLE
    artifact = NativeArtifact(
        (
            replace(
                artifact.headers[0],
                machine=machine,
                bits=image["bits"],
                kind=native_kind,
            ),
        )
    )
    if relocatable:
        image["kind"] = "object"
        image["sections"][0]["address"] = 0
        value -= 4096
    symbol = image["symbols"][0]
    symbol.update(address=value, arm_thumb=tag, kind=kind, declared_size=1)
    image["symbols"] = [symbol]
    if tag:
        # Exactly ends at the section boundary; treating raw value as an
        # address would refuse this independently authored valid interval.
        symbol["declared_size"] = 16
    sections, symbols = size._native_decoded_facts(facts, artifact, identity)
    assert symbols[0]["address"] == value
    assert symbols[0]["file_offset"] == expected
    symbols[0]["category"] = "runtime_abi"
    counts = size._native_file_accounting(256, sections, symbols)["category_bytes"]
    assert counts["runtime_abi"] == (16 if tag else 1)


@pytest.mark.parametrize(
    "mutation",
    [
        lambda image: image.update(machine=183),
        lambda image: image["symbols"][0].update(kind="Data"),
        lambda image: image["symbols"][0].update(address=4096),
        lambda image: image["symbols"][0].update(arm_thumb=1),
    ],
)
def test_thumb_receiver_rejects_contradictory_tag(mutation):
    facts, artifact, identity = _case()
    image = facts["slices"][0]
    image["machine"] = 40
    image["symbols"][0].update(address=4097, arm_thumb=True)
    mutation(image)
    image["bits"] = 32 if image["machine"] == 40 else 64
    artifact = NativeArtifact(
        (replace(artifact.headers[0], machine=image["machine"], bits=image["bits"]),)
    )
    with pytest.raises(ValueError, match="Thumb"):
        size._native_decoded_facts(facts, artifact, identity)


def test_shared_macho_receiver_matches_admitted_header_kind():
    facts, artifact, identity = _case(True)
    facts["slices"][0]["kind"] = "dynamic"
    artifact = NativeArtifact(
        (replace(artifact.headers[0], kind=NativeFileKind.SHARED_LIBRARY),)
    )
    sections, symbols = size._native_decoded_facts(facts, artifact, identity)
    assert sections and all(row["declared_size"] is None for row in symbols)


@pytest.mark.parametrize("difference", ["extent", "authority", "inspector", "none"])
def test_native_comparison_preserves_identity_and_reports_capability(
    difference, monkeypatch, tmp_path, capsys
):
    before = tmp_path / "before"
    after = tmp_path / "after"
    before.write_bytes(b"a")
    after.write_bytes(b"b")
    monkeypatch.setattr(
        size,
        "detect_format",
        lambda path: "elf" if path == before or difference != "extent" else "macho",
    )

    def inspect(path, *, scanner):
        second = path == after
        macho = second and difference == "extent"
        return {
            "total_bytes": 20 if second else 10,
            "sha256": ("b" if second else "a") * 64,
            "object_format": "macho" if macho else "elf",
            "universal": False,
            "native_slices": [
                {"offset": 0, "size": 20 if second else 10, "machine": 183}
            ],
            "attribution": size._native_file_accounting(20 if second else 10, [], []),
            "symbol_extent_status": "unavailable-macho-nlist"
            if macho
            else "declared-elf-extents",
            "tools": [
                {
                    "path": "scanner",
                    "sha256": ("x" if second and difference == "inspector" else "c")
                    * 64,
                    "schema_version": 1,
                }
            ],
            "name_authorities": [
                {
                    "path": "Cargo.toml",
                    "sha256": ("x" if second and difference == "authority" else "d")
                    * 64,
                }
            ],
        }

    monkeypatch.setattr(size, "analyse_native", inspect)
    result = size.compare_binaries(before, after)
    assert result["delta_bytes"] == 10
    assert result["native_context_before"]["sha256"] == "a" * 64
    assert result["native_context_after"]["sha256"] == "b" * 64
    for label, path in (("before", before), ("after", after)):
        expected = inspect(path, scanner=None)
        for key in ("tools", "name_authorities", "native_slices", "object_format"):
            assert result[f"native_context_{label}"][key] == expected[key]
    assert result["category_comparison"]["available"] == (difference == "none")
    assert ("category_deltas" in result) == (difference == "none")
    size.print_comparison(result)
    text = capsys.readouterr().out
    assert "declared-elf-extents" in text and "a" * 64 in text
    if difference == "extent":
        assert "unavailable-macho-nlist" in text
    if difference != "none":
        assert "Category deltas unavailable" in text


@pytest.mark.parametrize(
    "budget,code,label", [("1MB", 0, "Budget OK"), ("1B", 1, "BUDGET EXCEEDED")]
)
def test_actual_json_cli_stdout_feeds_existing_capsule_reader(
    tmp_path, budget, code, label
):
    from tools import analysis_capsule

    artifact = tmp_path / "empty.wasm"
    artifact.write_bytes(b"\0asm\x01\0\0\0")
    result = size.harness_memory_guard.guarded_completed_process(
        [
            sys.executable,
            str(ROOT / "tools/binary_size_analysis.py"),
            "--json",
            "--budget",
            budget,
            str(artifact),
        ],
        prefix="MOLT_TEST_SUITE",
        cwd=ROOT,
        capture_output=True,
        text=True,
        timeout=60,
        limits=size.harness_memory_guard.limits_from_env("MOLT_TEST_SUITE"),
    )
    assert result.returncode == code and result.child_returncode == code
    parsed = json.loads(result.stdout)  # Entire stdout; no last-line workaround.
    assert parsed["format"] == "wasm" and parsed["total_bytes"] == 8
    assert label in result.stderr and label not in result.stdout
    saved = tmp_path / "analysis.json"
    saved.write_text(result.stdout, encoding="utf-8")
    assert analysis_capsule.load_json(saved) == parsed
    assert analysis_capsule._summarize_binary(parsed, None)["size"]["total_bytes"] == 8


@pytest.mark.parametrize(
    "budget,code,label", [("1MB", 0, "Budget OK"), ("1B", 1, "BUDGET EXCEEDED")]
)
def test_native_json_cli_budget_uses_same_transport(
    tmp_path, monkeypatch, capsys, budget, code, label
):
    from tools import analysis_capsule

    artifact = tmp_path / "image"
    artifact.write_bytes(b"native")
    monkeypatch.setattr(size, "detect_format", lambda path: "elf")
    analysis = {
        "format": "native",
        "path": str(artifact),
        "total_bytes": 6,
        "attribution": size._native_file_accounting(6, [], []),
        "symbols": [],
    }
    monkeypatch.setattr(size, "analyse_native", lambda *args, **kwargs: analysis)
    monkeypatch.setattr(
        sys,
        "argv",
        ["binary_size_analysis.py", "--json", "--budget", budget, str(artifact)],
    )
    if code:
        with pytest.raises(SystemExit) as stop:
            size.main()
        assert stop.value.code == code
    else:
        size.main()
    captured = capsys.readouterr()
    parsed = json.loads(captured.out)
    assert parsed["total_bytes"] == 6 and label in captured.err
    saved = tmp_path / "native.json"
    saved.write_text(captured.out, encoding="utf-8")
    assert analysis_capsule.load_json(saved) == parsed
