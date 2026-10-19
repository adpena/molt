from __future__ import annotations

import json
from pathlib import Path
import subprocess
from types import SimpleNamespace

import pytest

from tools import build_browser_simd as builder


# Assemble the browser contract independently; no builder or WASM parser helpers.
_FUNCTIONS = (
    ("matmul_f32_tiled", b"\x7f" * 6),
    ("softmax_f32_fused", b"\x7f" * 3),
    ("rms_norm_f32", b"\x7f" * 4 + b"\x7d"),
    ("add_f32", b"\x7f" * 4),
    ("mul_f32", b"\x7f" * 4),
)


def _uint(value: int) -> bytes:
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def _vector(entries) -> bytes:
    return _uint(len(entries)) + b"".join(entries)


def _name(value: str) -> bytes:
    encoded = value.encode("utf-8")
    return _uint(len(encoded)) + encoded


def _exports(*, omit=None, kinds=None, indices=None, function_offset=0) -> bytes:
    entries = [
        (name, 0, index + function_offset)
        for index, (name, _parameters) in enumerate(_FUNCTIONS)
    ] + [("memory", 2, 0), ("__heap_base", 3, 0)]
    return _vector(
        [
            _name(name)
            + bytes([(kinds or {}).get(name, kind)])
            + _uint((indices or {}).get(name, index))
            for name, kind, index in entries
            if name != omit
        ]
    )


def _abi_sections() -> dict[int, bytes]:
    return {
        1: _vector(
            [
                b"\x60" + _uint(len(parameters)) + parameters + b"\0"
                for _name_, parameters in _FUNCTIONS
            ]
        ),
        3: _vector([_uint(index) for index in range(5)]),
        5: b"\1\0\1",  # One unshared memory with a one-page minimum.
        6: b"\1\x7f\0\x41\x10\x0b",  # Immutable i32.const 16.
        7: _exports(),
        10: _vector([b"\2\0\x0b"] * 5),  # Five void no-op bodies.
    }


def _module(sections: dict[int, bytes], *, marker=b"sim") -> bytes:
    return (
        b"\0asm\1\0\0\0"
        + b"".join(
            bytes([section]) + _uint(len(payload)) + payload
            for section, payload in sorted(sections.items())
        )
        + b"\0"
        + _uint(len(marker) + 1)
        + _uint(len(marker))
        + marker
    )


_MODULE = _module(_abi_sections())
_OLD = b"previous published bytes"


def _publication_generation(path: Path) -> tuple[int, int, int]:
    stat = path.stat()
    return stat.st_ino, stat.st_size, stat.st_mtime_ns


@pytest.fixture
def cargo_boundary(tmp_path: Path, monkeypatch: pytest.MonkeyPatch):
    root = tmp_path / "project"
    crate = root / "deploy/browser/simd-ops-rs"
    crate.mkdir(parents=True)
    (crate / "Cargo.toml").write_text(
        '[package]\nname="simd-ops"\nversion="0.1.0"\n', encoding="utf-8"
    )
    reported = tmp_path / "actual Cargo output" / "simd_ops.wasm"
    reported.parent.mkdir()
    reported.write_bytes(_MODULE)
    decoy = crate / "target/wasm32-unknown-unknown/release/simd_ops.wasm"
    decoy.parent.mkdir(parents=True)
    decoy.write_bytes(_module(_abi_sections(), marker=b"old"))
    output = tmp_path / "public" / "simd-ops.wasm"
    output.parent.mkdir()
    output.write_bytes(_OLD)
    artifact = {
        "reason": "compiler-artifact",
        "package_id": "path+" + crate.as_uri() + "#simd-ops@0.1.0",
        "manifest_path": str(crate / "Cargo.toml"),
        "target": {"name": "simd_ops", "kind": ["cdylib"], "crate_types": ["cdylib"]},
        "filenames": [str(reported)],
        "fresh": False,
    }
    terminal = {"reason": "build-finished", "success": True}
    environment = {
        "CARGO": str(tmp_path / "custom-cargo"),
        "CARGO_TARGET_DIR": str(tmp_path / "redirected-target"),
        "HF350_EFFECTIVE_ENVIRONMENT": "selected",
    }
    calls = []
    events = []
    state = {
        "messages": [artifact, terminal],
        "stdout": None,
        "returncode": 0,
        "error": None,
    }

    def context(selected_root, *, session_prefix):
        assert selected_root == root
        assert session_prefix == "browser-simd"

        def canonical_env(_base, *, create_dirs, admit_toolchain_root):
            assert create_dirs is False
            assert admit_toolchain_root is False
            return environment.copy()

        return SimpleNamespace(canonical_env=canonical_env)

    def capacity(command, *, cwd, env):
        assert command[0] == environment["CARGO"]
        assert cwd == crate
        assert env["CARGO_TARGET_DIR"] == environment["CARGO_TARGET_DIR"]
        events.append("capacity")

    def run(command, **kwargs):
        events.append("cargo")
        calls.append((command, kwargs))
        if state["error"] is not None:
            raise state["error"]
        stdout = state["stdout"]
        if stdout is None:
            stdout = "tool progress\n" + "\n".join(
                json.dumps(message) for message in state["messages"]
            )
        return subprocess.CompletedProcess(
            command, state["returncode"], stdout, "cargo diagnostic"
        )

    monkeypatch.setattr(builder, "RunContext", context)
    monkeypatch.setattr(builder, "require_cargo_build_capacity", capacity)
    monkeypatch.setattr(builder, "_COMMANDS", SimpleNamespace(run=run))
    monkeypatch.setattr(builder, "ROOT", root)
    return SimpleNamespace(
        root=root,
        crate=crate,
        reported=reported,
        decoy=decoy,
        output=output,
        artifact=artifact,
        terminal=terminal,
        environment=environment,
        state=state,
        calls=calls,
        events=events,
    )


def test_builder_publishes_only_the_actual_cargo_reported_artifact(cargo_boundary):
    fixture = cargo_boundary
    result = builder.build_browser_simd(fixture.output, project_root=fixture.root)
    assert result == fixture.output
    assert result.read_bytes() == _MODULE
    assert result.read_bytes() != fixture.decoy.read_bytes()
    assert fixture.events == ["capacity", "cargo"]
    [(command, kwargs)] = fixture.calls
    assert command == [
        fixture.environment["CARGO"],
        "build",
        "--locked",
        "--release",
        "--target",
        "wasm32-unknown-unknown",
        "--message-format=json-render-diagnostics",
    ]
    assert kwargs["cwd"] == fixture.crate
    assert kwargs["env"]["CARGO_TARGET_DIR"] == fixture.environment["CARGO_TARGET_DIR"]
    assert kwargs["env"]["HF350_EFFECTIVE_ENVIRONMENT"] == "selected"
    assert kwargs["timeout"] == 120
    assert kwargs["capture_output"] is True
    assert kwargs["text"] is True and kwargs["encoding"] == "utf-8"


@pytest.mark.parametrize("name,parameters", _FUNCTIONS)
@pytest.mark.parametrize("failure", ["missing", "signature"])
def test_browser_function_contract_is_required(
    cargo_boundary, name, parameters, failure
):
    sections = _abi_sections()
    if failure == "missing":
        sections[7] = _exports(omit=name)
    else:
        # Replace the last parameter, keeping a valid void no-op body. In
        # particular rms_norm requires f32 in its last position, not i32.
        types = []
        for selected, actual in _FUNCTIONS:
            if selected == name:
                actual = parameters[:-1] + (
                    b"\x7f" if parameters[-1:] == b"\x7d" else b"\x7d"
                )
            types.append(b"\x60" + _uint(len(actual)) + actual + b"\0")
        sections[1] = _vector(types)
    cargo_boundary.reported.write_bytes(_module(sections))
    before = _publication_generation(cargo_boundary.output)
    with pytest.raises(builder.BrowserSimdBuildError, match=name):
        builder.build_browser_simd(
            cargo_boundary.output, project_root=cargo_boundary.root
        )
    assert cargo_boundary.output.read_bytes() == _OLD
    assert _publication_generation(cargo_boundary.output) == before


def test_browser_function_result_must_be_void(cargo_boundary):
    sections = _abi_sections()
    sections[1] = _vector(
        [
            b"\x60"
            + _uint(len(parameters))
            + parameters
            + (b"\1\x7f" if name == "add_f32" else b"\0")
            for name, parameters in _FUNCTIONS
        ]
    )
    sections[10] = _vector(
        [b"\4\0\x41\0\x0b" if index == 3 else b"\2\0\x0b" for index in range(5)]
    )
    cargo_boundary.reported.write_bytes(_module(sections))
    before = _publication_generation(cargo_boundary.output)
    with pytest.raises(builder.BrowserSimdBuildError, match="add_f32.*void"):
        builder.build_browser_simd(
            cargo_boundary.output, project_root=cargo_boundary.root
        )
    assert cargo_boundary.output.read_bytes() == _OLD
    assert _publication_generation(cargo_boundary.output) == before


@pytest.mark.parametrize(
    "section,payload,diagnostic",
    [
        (7, _exports(kinds={"add_f32": 3}, indices={"add_f32": 0}), "add_f32"),
        (7, _exports(omit="memory"), "memory"),
        (7, _exports(kinds={"memory": 3}), "memory"),
        (7, _exports(indices={"memory": 1}), "memory"),
        (5, b"\0", "memory"),
        (5, b"\1\0\0", "memory"),
        (5, b"\1\3\1\1", "memory"),  # Shared memory has a maximum.
        (5, b"\2\0\1\0\1", "memory"),
        (7, _exports(omit="__heap_base"), "__heap_base"),
        (7, _exports(kinds={"__heap_base": 0}), "__heap_base"),
        (6, b"\1\x7e\0\x42\x10\x0b", "__heap_base"),  # i64.
        (6, b"\1\x7f\1\x41\x10\x0b", "__heap_base"),  # Mutable.
        (6, b"\1\x7f\0\x41\0\x0b", "__heap_base"),  # Zero.
        (6, b"\1\x7f\0\x41\x70\x0b", "__heap_base"),  # Negative.
        (6, b"\1\x7f\0\x41\x11\x0b", "__heap_base"),  # Unaligned.
        (6, b"\1\x7f\0\x41\x90\x80\4\x0b", "__heap_base"),  # 65552.
        (6, b"\1\x7f\0\x41\x90\0\x0b", "__heap_base"),  # Noncanonical.
    ],
)
def test_browser_memory_global_and_export_refusals_preserve_publication(
    cargo_boundary, section, payload, diagnostic
):
    sections = _abi_sections()
    sections[section] = payload
    cargo_boundary.reported.write_bytes(_module(sections))
    before = _publication_generation(cargo_boundary.output)
    with pytest.raises(builder.BrowserSimdBuildError, match=diagnostic):
        builder.build_browser_simd(
            cargo_boundary.output, project_root=cargo_boundary.root
        )
    assert cargo_boundary.output.read_bytes() == _OLD
    assert _publication_generation(cargo_boundary.output) == before


@pytest.mark.parametrize("kind", ["function", "memory", "global"])
def test_browser_module_must_own_its_abi(cargo_boundary, kind):
    sections = _abi_sections()
    if kind == "function":
        descriptor = b"\0\0"  # Function with type index 0.
        sections[7] = _exports(function_offset=1)
    elif kind == "memory":
        descriptor = b"\2\0\1"
        del sections[5]
    else:
        descriptor = b"\3\x7f\0"
        # Export the owned global at index1, dynamically copying the imported
        # immutable global, so it still cannot offer a constant heap base.
        sections[7] = _exports(indices={"__heap_base": 1})
        sections[6] = b"\1\x7f\0\x23\0\x0b"
    sections[2] = _vector([_name("env") + _name("external") + descriptor])
    cargo_boundary.reported.write_bytes(_module(sections))
    before = _publication_generation(cargo_boundary.output)
    with pytest.raises(builder.BrowserSimdBuildError, match="imported ABI"):
        builder.build_browser_simd(
            cargo_boundary.output, project_root=cargo_boundary.root
        )
    assert cargo_boundary.output.read_bytes() == _OLD
    assert _publication_generation(cargo_boundary.output) == before


@pytest.mark.parametrize("failure", ["missing", "mismatched"])
def test_browser_abi_reports_all_failed_capabilities(cargo_boundary, failure):
    module = b"\0asm\1\0\0\0"
    if failure == "mismatched":
        sections = _abi_sections()
        sections[1] = _vector([b"\x60\0\0"] * 5)
        sections[5] = b"\1\0\0"
        sections[6] = b"\1\x7f\1\x41\x10\x0b"
        module = _module(sections)
    cargo_boundary.reported.write_bytes(module)
    with pytest.raises(builder.BrowserSimdBuildError) as caught:
        builder.build_browser_simd(
            cargo_boundary.output, project_root=cargo_boundary.root
        )
    for capability in [name for name, _parameters in _FUNCTIONS] + [
        "memory",
        "__heap_base",
    ]:
        assert capability in str(caught.value)
    assert cargo_boundary.output.read_bytes() == _OLD


def test_browser_heap_may_start_at_initial_memory_boundary(cargo_boundary):
    sections = _abi_sections()
    sections[6] = b"\1\x7f\0\x41\x80\x80\4\x0b"  # i32.const 65536.
    # An explicit unshared maximum is also a valid owned memory.
    sections[5] = b"\1\1\1\2"
    module = _module(sections)
    cargo_boundary.reported.write_bytes(module)
    result = builder.build_browser_simd(
        cargo_boundary.output, project_root=cargo_boundary.root
    )
    assert result.read_bytes() == module


@pytest.mark.parametrize(
    "failure",
    [
        "cargo-failed",
        "missing-terminal",
        "failed-terminal",
        "duplicate-terminal",
        "artifact-after-terminal",
        "malformed-json",
        "duplicate-json-key",
        "missing-artifact",
        "wrong-target",
        "wrong-crate-type",
        "foreign-manifest",
        "foreign-package",
        "two-artifacts",
        "two-wasm-files",
        "missing-file",
        "not-regular",
        "invalid-header",
        "oversized",
    ],
)
def test_builder_refuses_unproven_outputs_and_preserves_old_publication(
    cargo_boundary, monkeypatch: pytest.MonkeyPatch, failure: str
):
    fixture = cargo_boundary
    state = fixture.state
    if failure == "cargo-failed":
        state["returncode"] = 1
    elif failure == "missing-terminal":
        state["messages"] = [fixture.artifact]
    elif failure == "failed-terminal":
        fixture.terminal["success"] = False
    elif failure == "duplicate-terminal":
        state["messages"].append(fixture.terminal)
    elif failure == "artifact-after-terminal":
        state["messages"] = [fixture.terminal, fixture.artifact]
    elif failure == "malformed-json":
        state["stdout"] = (
            json.dumps(fixture.artifact)
            + '\n{"reason":\n'
            + json.dumps(fixture.terminal)
        )
    elif failure == "duplicate-json-key":
        state["stdout"] = (
            json.dumps(fixture.artifact)
            + '\n{"reason":"build-finished","success":false,"success":true}'
        )
    elif failure == "missing-artifact":
        state["messages"] = [fixture.terminal]
    elif failure == "wrong-target":
        fixture.artifact["target"]["name"] = "other"
    elif failure == "wrong-crate-type":
        fixture.artifact["target"]["crate_types"] = ["rlib"]
    elif failure == "foreign-manifest":
        fixture.artifact["manifest_path"] = str(fixture.root / "Cargo.toml")
    elif failure == "foreign-package":
        fixture.artifact["package_id"] = (
            "path+" + fixture.root.as_uri() + "#simd-ops@0.1.0"
        )
    elif failure == "two-artifacts":
        state["messages"].insert(0, fixture.artifact)
    elif failure == "two-wasm-files":
        fixture.artifact["filenames"].append(str(fixture.decoy))
    elif failure == "missing-file":
        fixture.reported.unlink()
    elif failure == "not-regular":
        fixture.reported.unlink()
        fixture.reported.mkdir()
    elif failure == "invalid-header":
        fixture.reported.write_bytes(b"not a WASM module")
    else:
        monkeypatch.setattr(builder, "_MAX_ARTIFACT_BYTES", len(_MODULE) - 1)
    before = fixture.output.stat()
    with pytest.raises((OSError, ValueError)):
        builder.build_browser_simd(fixture.output, project_root=fixture.root)
    assert fixture.output.read_bytes() == _OLD
    after = fixture.output.stat()
    assert (after.st_ino, after.st_size, after.st_mtime_ns) == (
        before.st_ino,
        before.st_size,
        before.st_mtime_ns,
    )
    assert len(fixture.calls) == 1


def test_capacity_refusal_does_not_run_cargo_or_publish(cargo_boundary, monkeypatch):
    def refuse(*_args, **_kwargs):
        raise ValueError("insufficient build capacity")

    monkeypatch.setattr(builder, "require_cargo_build_capacity", refuse)
    with pytest.raises(ValueError, match="insufficient build capacity"):
        builder.build_browser_simd(
            cargo_boundary.output, project_root=cargo_boundary.root
        )
    assert cargo_boundary.calls == []
    assert cargo_boundary.output.read_bytes() == _OLD


@pytest.mark.parametrize("failure", ["timeout", "spawn"])
def test_command_failure_preserves_existing_publication(cargo_boundary, failure):
    cargo_boundary.state["error"] = (
        subprocess.TimeoutExpired(["cargo"], 120)
        if failure == "timeout"
        else OSError("cannot spawn Cargo")
    )
    with pytest.raises((OSError, subprocess.TimeoutExpired)):
        builder.build_browser_simd(
            cargo_boundary.output, project_root=cargo_boundary.root
        )
    assert cargo_boundary.output.read_bytes() == _OLD


@pytest.mark.parametrize(
    "case",
    [
        "equal",
        "different",
        "bad-transcript",
        "bad-artifact",
        "bad-abi",
        "missing-output",
    ],
)
def test_cli_check_compares_a_fresh_build_without_publishing(
    cargo_boundary, monkeypatch, case, capsys
):
    fixture = cargo_boundary
    if case == "equal":
        fixture.output.write_bytes(_MODULE)
    elif case == "bad-transcript":
        fixture.state["messages"] = [fixture.artifact]
    elif case == "bad-artifact":
        fixture.reported.write_bytes(b"invalid WASM")
    elif case == "bad-abi":
        fixture.reported.write_bytes(b"\0asm\1\0\0\0")
    elif case == "missing-output":
        fixture.output.unlink()
    old = fixture.output.read_bytes() if fixture.output.exists() else None

    def publication_forbidden(*_args, **_kwargs):
        pytest.fail("--check must not publish an artifact")

    monkeypatch.setattr(builder, "atomic_write_bytes", publication_forbidden)
    result = builder.main(["--check", "--output", str(fixture.output)])
    assert result == (0 if case == "equal" else 1)
    assert (fixture.output.read_bytes() if fixture.output.exists() else None) == old
    assert len(fixture.calls) == 1
    if case != "equal":
        assert "browser SIMD:" in capsys.readouterr().err
