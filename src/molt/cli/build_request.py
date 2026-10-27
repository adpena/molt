"""One typed compilation request, shared by callers and process transport."""

from __future__ import annotations

from collections.abc import Mapping
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Literal

from molt.backend_executable_names import CodegenBackend, DEFAULT_CODEGEN_BACKEND
from molt.capability_policy import CapabilityInput
from molt.cli.config_resolution import StdlibProfile
from molt.type_facts import TypeHintPolicy
from molt.compilation_target import (
    Target,
    TargetKind,
    admit_target,
    target_name,
    validate_target,
)

ParseCodec = Literal["msgpack", "cbor", "json"]
BuildProfile = Literal["dev", "release"]
# "jit" publishes a JIT image, one executable file the development JIT runner
# (molt-jit) starts, instead of an AOT executable; `molt run --tier jit`
# selects it.
EmitMode = Literal["bin", "obj", "wasm", "jit"]


@dataclass(frozen=True, slots=True)
class FactGraphRequest:
    output_path: Path
    function_name: str
    requested_target: str


@dataclass(frozen=True, slots=True, kw_only=True)
class BuildRequest:
    file_path: str | None
    target: Target = TargetKind.NATIVE
    parse_codec: ParseCodec = "msgpack"
    type_hint_policy: TypeHintPolicy = "check"
    type_facts_path: str | None = None
    pgo_profile: str | None = None
    runtime_feedback: str | None = None
    output: str | None = None
    json_output: bool = False
    verbose: bool = False
    deterministic: bool = True
    deterministic_warn: bool = False
    trusted: bool = False
    capabilities: CapabilityInput | None = None
    cache: bool = True
    cache_dir: str | None = None
    cache_report: bool = False
    sysroot: str | None = None
    emit_ir: str | None = None
    emit: EmitMode | None = None
    out_dir: str | None = None
    profile: BuildProfile = "release"
    linked: bool = False
    linked_output: str | None = None
    require_linked: bool = False
    respect_pythonpath: bool = False
    module: str | None = None
    diagnostics: bool | None = None
    capture_link_inputs: bool = False
    diagnostics_file: str | None = None
    diagnostics_verbosity: str | None = None
    portable: bool = False
    wasm_opt_level: str | None = "Oz"
    precompile: bool = False
    wasm_profile: str = "auto"
    snapshot: bool = False
    stdlib_profile: StdlibProfile | None = None
    tree_shake: bool = True
    lib_paths: list[str] | None = None
    split_runtime: bool = False
    capability_manifest: str | None = None
    require_signed_manifest: bool = False
    audit_log: str | None = None
    io_mode: str | None = None
    python_version: str | None = None
    build_config: Mapping[str, Any] | None = None
    bolt: bool = False
    bolt_training_cmd: str | None = None
    fact_graph_request: FactGraphRequest | None = None
    codegen_backend: CodegenBackend = DEFAULT_CODEGEN_BACKEND

    def __post_init__(self) -> None:
        validate_target(self.target)


def _require_json_value(value: object, location: str = "$") -> None:
    """Keep arbitrary configuration/capability values lossless on the JSON wire."""
    import math

    if value is None or type(value) in (str, bool, int):
        return
    if type(value) is float and math.isfinite(value):
        return
    if type(value) is list:
        for index, item in enumerate(value):
            _require_json_value(item, f"{location}[{index}]")
        return
    if type(value) is dict:
        for key, item in value.items():
            if type(key) is not str:
                raise ValueError(f"{location}: JSON object keys must be strings")
            _require_json_value(item, f"{location}.{key}")
        return
    raise ValueError(
        f"{location}: expected a finite JSON value, got {type(value).__name__}"
    )


def _reject_unknown_fields(
    payload: Mapping[str, Any], schema: type[BuildRequest] | type[FactGraphRequest]
) -> None:
    from dataclasses import fields

    unknown = payload.keys() - {field.name for field in fields(schema)}
    if unknown:
        raise ValueError(
            f"{schema.__name__}: unknown fields: {', '.join(sorted(unknown))}"
        )


def _decode_path(expected: type, value: object) -> Path:
    if expected is Path and type(value) is str:
        return Path(value)
    raise ValueError("fact graph output_path must be a string")


def build_request_from_payload(payload: object) -> BuildRequest:
    """Admit canonical batch build params; aliases and scalar coercions reject."""
    import msgspec

    _require_json_value(payload)
    if not isinstance(payload, dict):
        raise ValueError("BuildRequest must be a JSON object")
    _reject_unknown_fields(payload, BuildRequest)
    graph = payload.get("fact_graph_request")
    if isinstance(graph, dict):
        _reject_unknown_fields(graph, FactGraphRequest)
    admitted = dict(payload)
    if "target" in admitted:
        admitted["target"] = admit_target(admitted["target"])
    return msgspec.convert(admitted, BuildRequest, strict=True, dec_hook=_decode_path)


def parse_codec_from_input(value: str | None) -> ParseCodec:
    """Admit a codec input using the request's strict decoder and default."""
    payload: dict[str, object] = {"file_path": None}
    if value is not None:
        payload["parse_codec"] = value
    return build_request_from_payload(payload).parse_codec


def build_request_values(request: BuildRequest) -> dict[str, Any]:
    """Project the complete request into scalar cache/transport field values."""
    from dataclasses import fields

    payload = {
        field.name: getattr(request, field.name) for field in fields(BuildRequest)
    }
    payload["target"] = target_name(request.target)
    graph = request.fact_graph_request
    if graph is not None:
        if not isinstance(graph, FactGraphRequest) or not isinstance(
            graph.output_path, Path
        ):
            raise ValueError("fact_graph_request must contain a Path output_path")
        payload["fact_graph_request"] = {
            field.name: str(graph.output_path)
            if field.name == "output_path"
            else getattr(graph, field.name)
            for field in fields(FactGraphRequest)
        }
    return payload


def build_request_to_payload(request: BuildRequest) -> dict[str, Any]:
    """Validate the canonical field projection at the process boundary."""
    payload = build_request_values(request)
    build_request_from_payload(payload)
    return payload
