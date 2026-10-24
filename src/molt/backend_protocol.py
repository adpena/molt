"""Compiler backend transport facts shared with the compiled server."""

from __future__ import annotations

from dataclasses import dataclass
import re
from pathlib import Path
from typing import Literal


BACKEND_DAEMON_PROTOCOL_VERSION = int(
    Path(__file__).with_name("backend_daemon_protocol_version.txt").read_text("ascii")
)
if not 0 < BACKEND_DAEMON_PROTOCOL_VERSION < 1 << 32:
    raise ValueError("backend daemon protocol version must fit a positive u32")

BACKEND_IR_LEASE_FORMAT: Literal["msgpack"] = "msgpack"


def _content(value: object, label: str) -> tuple[str, int]:
    if (
        not isinstance(value, dict)
        or set(value) != {"sha256", "size"}
        or not isinstance(value["sha256"], str)
        or re.fullmatch(r"[0-9a-f]{64}", value["sha256"]) is None
        or type(value["size"]) is not int
        or not 0 < value["size"] < 1 << 64
    ):
        raise ValueError(f"backend origin {label} content identity is invalid")
    return value["sha256"], value["size"]


def _process_id(value: object) -> int:
    if type(value) is not int or not 0 < value < 1 << 32:
        raise ValueError("backend origin process ID is invalid")
    return value


@dataclass(frozen=True, slots=True)
class BackendNativeWorkerOrigin:
    process_id: int
    scope: str | None
    input_sha256: str
    input_size: int
    output_sha256: str
    output_size: int

    @classmethod
    def from_payload(cls, value: object) -> BackendNativeWorkerOrigin:
        if (
            not isinstance(value, dict)
            or set(value) != {"schema", "process_id", "scope", "input", "output"}
            or value["schema"] != "molt.native-worker-origin.v2"
        ):
            raise ValueError("native worker origin schema is invalid")
        return cls(
            _process_id(value["process_id"]),
            validate_emission_scope(value["scope"]),
            *_content(value["input"], "worker input"),
            *_content(value["output"], "worker output"),
        )

    def payload(self) -> dict[str, object]:
        return {
            "schema": "molt.native-worker-origin.v2",
            "process_id": self.process_id,
            "scope": self.scope,
            "input": {"sha256": self.input_sha256, "size": self.input_size},
            "output": {"sha256": self.output_sha256, "size": self.output_size},
        }


@dataclass(frozen=True, slots=True)
class BackendCodegenOrigin:
    process_id: int | None
    workers: tuple[BackendNativeWorkerOrigin, ...]

    @classmethod
    def from_payload(cls, value: object, *, producer_pid: int) -> BackendCodegenOrigin:
        if (
            isinstance(value, dict)
            and set(value) == {"kind", "process_id"}
            and value["kind"] == "in-process"
        ):
            process_id = _process_id(value["process_id"])
            if process_id != producer_pid:
                raise ValueError(
                    "in-process codegen origin differs from original producer"
                )
            return cls(process_id, ())
        if (
            isinstance(value, dict)
            and set(value) == {"kind", "workers"}
            and value["kind"] == "native-workers"
        ):
            if not isinstance(value["workers"], list) or not value["workers"]:
                raise ValueError("native codegen origin has no workers")
            return cls(
                None,
                tuple(
                    BackendNativeWorkerOrigin.from_payload(row)
                    for row in value["workers"]
                ),
            )
        raise ValueError("backend codegen origin schema is invalid")

    def payload(self) -> dict[str, object]:
        if self.process_id is not None:
            return {"kind": "in-process", "process_id": self.process_id}
        return {
            "kind": "native-workers",
            "workers": [worker.payload() for worker in self.workers],
        }


@dataclass(frozen=True, slots=True)
class BackendAddressedProgramInput:
    """Exact partial lease bytes and the ordered, validated assembled bodies.

    The lease binds the program header and shipped bodies. Resident bodies are
    named by digest, not falsely counted as bytes read from this lease.
    """

    lease_sha256: str
    lease_size: int
    function_digests: str

    @classmethod
    def from_payload(cls, value: object) -> BackendAddressedProgramInput:
        if (
            not isinstance(value, dict)
            or set(value) != {"schema", "lease", "function_digests"}
            or value["schema"] != "molt.addressed-program-input.v1"
        ):
            raise ValueError("backend addressed input schema is invalid")
        digests = value["function_digests"]
        if (
            not isinstance(digests, str)
            or re.fullmatch(r"(?:[0-9a-f]{64})*", digests) is None
        ):
            raise ValueError("backend addressed input function digests are invalid")
        ordered = tuple(digests[i : i + 64] for i in range(0, len(digests), 64))
        if len(set(ordered)) != len(ordered):
            raise ValueError("backend addressed input repeats a function digest")
        return cls(*_content(value["lease"], "addressed lease"), digests)

    def payload(self) -> dict[str, object]:
        return {
            "schema": "molt.addressed-program-input.v1",
            "lease": {"sha256": self.lease_sha256, "size": self.lease_size},
            "function_digests": self.function_digests,
        }


def validate_emission_scope(value: object) -> str | None:
    if value is None:
        return None
    if not isinstance(value, str) or re.fullmatch(r"[0-9a-f]{64}", value) is None:
        raise ValueError("emission scope is not a canonical policy nonce")
    return value


@dataclass(frozen=True, slots=True)
class BackendEmissionProducer:
    kind: Literal["one-shot", "daemon"]
    process_id: int
    scope: str | None

    @classmethod
    def from_payload(cls, value: object) -> BackendEmissionProducer:
        if (
            not isinstance(value, dict)
            or set(value) != {"kind", "process_id", "scope"}
            or value["kind"] not in ("one-shot", "daemon")
        ):
            raise ValueError("backend emission producer is invalid")
        kind: Literal["one-shot", "daemon"] = (
            "one-shot" if value["kind"] == "one-shot" else "daemon"
        )
        return cls(
            kind,
            _process_id(value["process_id"]),
            validate_emission_scope(value["scope"]),
        )

    def payload(self) -> dict[str, object]:
        return {"kind": self.kind, "process_id": self.process_id, "scope": self.scope}


@dataclass(frozen=True, slots=True)
class BackendEmissionOrigin:
    """Original input, producing processes and output, without admission claims.

    Scope and PID are observations. Only the original closed native journal can
    bind them to a stable generation and image; current process state cannot.
    """

    producer: BackendEmissionProducer
    input: BackendAddressedProgramInput
    output_sha256: str
    output_size: int
    codegen: BackendCodegenOrigin

    @classmethod
    def from_payload(cls, value: object) -> BackendEmissionOrigin:
        if (
            not isinstance(value, dict)
            or set(value) != {"schema", "producer", "input", "output", "codegen"}
            or value["schema"] != "molt.backend-emission-origin.v1"
        ):
            raise ValueError("backend emission origin schema is invalid")
        producer = BackendEmissionProducer.from_payload(value["producer"])
        input_identity = BackendAddressedProgramInput.from_payload(value["input"])
        codegen = BackendCodegenOrigin.from_payload(
            value["codegen"], producer_pid=producer.process_id
        )
        if any(worker.scope != producer.scope for worker in codegen.workers):
            raise ValueError(
                "application worker origin belongs to another emission scope"
            )
        return cls(
            producer, input_identity, *_content(value["output"], "output"), codegen
        )

    def payload(self) -> dict[str, object]:
        return {
            "schema": "molt.backend-emission-origin.v1",
            "producer": self.producer.payload(),
            "input": self.input.payload(),
            "output": {"sha256": self.output_sha256, "size": self.output_size},
            "codegen": self.codegen.payload(),
        }

    def verify_output(self, path: Path) -> None:
        _verify_emitted_output(
            path, self.output_sha256, self.output_size, self.codegen.workers
        )


@dataclass(frozen=True, slots=True)
class BackendStdlibOrigin:
    """Archive bytes and ordered original constituent emissions, across scopes."""

    output_sha256: str
    output_size: int
    workers: tuple[BackendNativeWorkerOrigin, ...]

    @classmethod
    def from_payload(cls, value: object) -> BackendStdlibOrigin:
        if (
            not isinstance(value, dict)
            or set(value) != {"schema", "output", "workers"}
            or value["schema"] != "molt.stdlib-archive-origin.v1"
            or not isinstance(value["workers"], list)
            or not 0 < len(value["workers"]) <= 16384
        ):
            raise ValueError("stdlib archive origin schema is invalid")
        return cls(
            *_content(value["output"], "stdlib output"),
            tuple(
                BackendNativeWorkerOrigin.from_payload(row) for row in value["workers"]
            ),
        )

    def payload(self) -> dict[str, object]:
        return {
            "schema": "molt.stdlib-archive-origin.v1",
            "output": {"sha256": self.output_sha256, "size": self.output_size},
            "workers": [worker.payload() for worker in self.workers],
        }

    def verify_output(self, path: Path) -> None:
        _verify_emitted_output(path, self.output_sha256, self.output_size, self.workers)


def _verify_emitted_output(
    path: Path, sha256: str, size: int, workers: tuple[BackendNativeWorkerOrigin, ...]
) -> None:
    import hashlib
    from molt.toolchain_identity import (
        open_stable_regular_file,
        stable_regular_file_handle_identity,
    )
    from molt.cli.static_archive_identity import open_static_archive_members
    from molt.native_artifact_header import (
        NativeReader,
        NativeFileKind,
        decode_native_artifact,
    )
    from molt.native_target_shape import NativeObjectFormat

    with open_stable_regular_file(path, label="backend emitted output") as opened:
        observed = stable_regular_file_handle_identity(
            opened, label="backend emitted output"
        )
        if (observed.sha256, observed.size) != (sha256, size):
            raise ValueError("backend emission origin names different output bytes")
        if not workers:
            return
        with open_static_archive_members(opened.path, opened=opened) as (
            members,
            stream,
        ):
            if len(members) != len(workers):
                raise ValueError(
                    "backend archive origin omits or repeats a worker output"
                )
            for index, (member, worker) in enumerate(
                zip(members, workers, strict=True)
            ):
                if (
                    member.name != f"molt_{index:08}.o"
                    or member.size < worker.output_size
                ):
                    raise ValueError(
                        "backend archive member differs from native worker origin"
                    )
                stream.seek(member.content_offset)
                remaining = worker.output_size
                digest = hashlib.sha256()
                while remaining:
                    block = stream.read(min(1024 * 1024, remaining))
                    if not block:
                        raise ValueError("truncated native worker output")
                    digest.update(block)
                    remaining -= len(block)
                if digest.hexdigest() != worker.output_sha256:
                    raise ValueError(
                        "backend archive member differs from native worker origin"
                    )
                padding = member.size - worker.output_size
                if padding:
                    # ar_archive_writer 0.5.3 aligns Darwin member data to eight
                    # with newlines. This is distinct from BSD extended-name
                    # padding, which the canonical archive parser already removed.
                    suffix = stream.read(padding) if padding < 8 else b""

                    def read_at(offset: int, count: int) -> bytes:
                        stream.seek(offset)
                        return stream.read(count)

                    artifact = decode_native_artifact(
                        NativeReader(worker.output_size, read_at, member.content_offset)
                    )
                    if (
                        member.name_format != "bsd-extended"
                        or artifact.universal
                        or padding != (-worker.output_size) % 8
                        or suffix != b"\n" * padding
                    ):
                        raise ValueError(
                            "native archive has undocumented member padding"
                        )
                    artifact.validate_format_and_kind(
                        object_format=NativeObjectFormat.MACHO,
                        kinds=frozenset({NativeFileKind.OBJECT}),
                    )
