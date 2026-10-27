from __future__ import annotations

from dataclasses import fields
import json
from pathlib import Path

import pytest

from molt.cli.build_request import (
    BuildRequest,
    FactGraphRequest,
    build_request_from_payload,
    build_request_to_payload,
    parse_codec_from_input,
)


@pytest.fixture
def full_payload():
    # Independently enumerate every public build option: additions need wire proof.
    return {
        "file_path": "main.py",
        "target": "wasm",
        "parse_codec": "json",
        "type_hint_policy": "trust",
        "type_facts_path": "facts.json",
        "pgo_profile": "pgo.json",
        "runtime_feedback": "feedback.json",
        "output": "out.wasm",
        "json_output": True,
        "capture_link_inputs": True,
        "verbose": True,
        "deterministic": False,
        "deterministic_warn": True,
        "trusted": True,
        "capabilities": {
            "allow": ["fs.read"],
            "nested": {"limit": 3, "enabled": False},
        },
        "cache": False,
        "cache_dir": "cache",
        "cache_report": True,
        "sysroot": "sysroot",
        "emit_ir": "ir.json",
        "emit": "wasm",
        "out_dir": "out",
        "profile": "dev",
        "linked": True,
        "linked_output": "linked.wasm",
        "require_linked": True,
        "respect_pythonpath": True,
        "module": "example.main",
        "diagnostics": True,
        "diagnostics_file": "diagnostics.json",
        "diagnostics_verbosity": "debug",
        "portable": True,
        "wasm_opt_level": "O2",
        "precompile": True,
        "wasm_profile": "wasi",
        "snapshot": True,
        "stdlib_profile": "full",
        "tree_shake": False,
        "lib_paths": ["lib", "other"],
        "split_runtime": True,
        "capability_manifest": "capabilities.json",
        "require_signed_manifest": True,
        "audit_log": "audit.json",
        "io_mode": "async",
        "python_version": "3.14",
        "build_config": {"nested": [None, True, 4, 1.5, {"x": "value"}]},
        "bolt": True,
        "bolt_training_cmd": "training --flag",
        "fact_graph_request": {
            "output_path": str(Path("graphs/f.json")),
            "function_name": "f",
            "requested_target": "llvm",
        },
        "codegen_backend": "llvm",
    }


def test_complete_request_round_trip(full_payload):
    assert set(full_payload) == {field.name for field in fields(BuildRequest)}
    request = build_request_from_payload(json.loads(json.dumps(full_payload)))
    assert request.fact_graph_request == FactGraphRequest(
        Path("graphs/f.json"), "f", "llvm"
    )
    assert request.codegen_backend == "llvm"
    assert build_request_to_payload(request) == full_payload
    assert build_request_from_payload(build_request_to_payload(request)) == request


def test_defaults_and_in_process_construction():
    request = BuildRequest(file_path=None)
    assert request.profile == "release"
    assert build_request_from_payload({"file_path": None}) == request
    # The in-process constructor does not serialize arbitrary caller-owned config.
    config = {"caller_value": object()}
    assert BuildRequest(file_path=None, build_config=config).build_config is config


@pytest.mark.parametrize(
    "capabilities", [None, "fs.read", ["fs.read", "net.read"], {"allow": ["fs.read"]}]
)
def test_capability_forms(capabilities):
    request = BuildRequest(file_path=None, capabilities=capabilities)
    assert build_request_from_payload(build_request_to_payload(request)) == request


@pytest.mark.parametrize(
    "name,value",
    [
        ("cache", 1),
        ("verbose", "false"),
        ("diagnostics", 0),
        ("target", False),
        ("file_path", 1),
        ("lib_paths", [1]),
        ("parse_codec", "pickle"),
        ("type_hint_policy", "coerce"),
        ("profile", "debug"),
        ("emit", "exe"),
        ("codegen_backend", "unknown"),
        ("capabilities", [False]),
        ("build_config", []),
        ("build_config", {"x": float("nan")}),
        ("build_config", {1: "x"}),
        ("build_config", {"x": (1, 2)}),
    ],
)
def test_bad_types_reject(name, value):
    with pytest.raises((ValueError, TypeError)):
        build_request_from_payload({"file_path": None, name: value})
    with pytest.raises((ValueError, TypeError)):
        build_request_to_payload(BuildRequest(**{"file_path": None, name: value}))


@pytest.mark.parametrize(
    "name", ["unknown", "codec", "type_hints", "backend", "type_facts"]
)
def test_unknown_and_replaced_names_reject(name):
    with pytest.raises(ValueError, match="unknown fields"):
        build_request_from_payload({"file_path": None, name: "x"})


@pytest.mark.parametrize(
    "change",
    [
        {"extra": True},
        {"output_path": 1},
        {"function_name": False},
        {"requested_target": 1},
    ],
)
def test_fact_graph_schema_rejects(full_payload, change):
    full_payload["fact_graph_request"].update(change)
    with pytest.raises((ValueError, TypeError)):
        build_request_from_payload(full_payload)


@pytest.mark.parametrize(
    "payload",
    [None, [], {}, {"file_path": None, "fact_graph_request": {"output_path": "x"}}],
)
def test_missing_or_nonobject_request_rejects(payload):
    with pytest.raises((ValueError, TypeError)):
        build_request_from_payload(payload)


def test_full_request_reaches_batch_build_without_option_loss(
    full_payload, monkeypatch, capsys
):
    import io
    import sys
    from molt.cli.quality_commands import _internal_batch_build_server

    received = []

    def compile_request(request: BuildRequest) -> int:
        received.append(request)
        print("compiler stdout")
        print("compiler stderr", file=sys.stderr)
        return 7

    monkeypatch.setattr(
        sys,
        "stdin",
        io.StringIO(
            json.dumps(
                {
                    "id": 42,
                    "op": "build",
                    "params": full_payload,
                }
            )
            + "\n"
            + json.dumps({"id": 43, "op": "shutdown"})
            + "\n"
        ),
    )
    assert _internal_batch_build_server(build_fn=compile_request) == 0
    assert len(received) == 1
    assert build_request_to_payload(received[0]) == full_payload
    responses = [json.loads(line) for line in capsys.readouterr().out.splitlines()]
    assert responses == [
        {
            "id": 42,
            "ok": False,
            "returncode": 7,
            "stdout": "compiler stdout\n",
            "stderr": "compiler stderr\n",
        },
        {"id": 43, "ok": True, "shutdown": True},
    ]


def test_batch_refusal_reports_all_unknown_options_before_environment(
    monkeypatch, capsys
):
    import io
    import sys
    from molt.cli import quality_commands

    def forbidden(*args, **kwargs):
        pytest.fail("invalid request crossed the environment/build boundary")

    monkeypatch.setattr(quality_commands, "_temporary_env_overrides", forbidden)
    monkeypatch.setattr(
        sys,
        "stdin",
        io.StringIO(
            json.dumps(
                {
                    "id": 9,
                    "op": "build",
                    "params": {
                        "file_path": "app.py",
                        "codec": "json",
                        "backend": "llvm",
                    },
                    "env_overrides": {"MOLT_REQUEST_REFUSAL_PROBE": "forbidden"},
                }
            )
            + "\n"
        ),
    )
    assert quality_commands._internal_batch_build_server(build_fn=forbidden) == 0
    assert json.loads(capsys.readouterr().out) == {
        "id": 9,
        "ok": False,
        "error": "BuildRequest: unknown fields: backend, codec",
    }


@pytest.mark.parametrize(
    ("raw", "expected"),
    [(None, "msgpack"), ("msgpack", "msgpack"), ("cbor", "cbor"), ("json", "json")],
)
def test_codec_input_admits_only_the_selected_value(
    raw, expected, monkeypatch, tmp_path
):
    monkeypatch.setenv("MOLT_CODEC", "invalid-ambient")
    (tmp_path / "molt.toml").write_text('[build]\ncodec = "cbor"\n', encoding="utf-8")
    monkeypatch.chdir(tmp_path)
    assert parse_codec_from_input(raw) == expected


@pytest.mark.parametrize("raw", ["", "invalid-codec", "JSON", " msgpack "])
def test_codec_input_rejects_present_invalid_values(raw):
    with pytest.raises(ValueError, match="parse_codec"):
        parse_codec_from_input(raw)
