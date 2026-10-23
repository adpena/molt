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
    input_sha256: str
    input_size: int
    output_sha256: str
    output_size: int

    @classmethod
    def from_payload(cls, value: object) -> BackendNativeWorkerOrigin:
        if (
            not isinstance(value, dict)
            or set(value) != {"schema", "process_id", "input", "output"}
            or value["schema"] != "molt.native-worker-origin.v1"
        ):
            raise ValueError("native worker origin schema is invalid")
        return cls(
            _process_id(value["process_id"]),
            *_content(value["input"], "worker input"),
            *_content(value["output"], "worker output"),
        )

    def payload(self) -> dict[str, object]:
        return {
            "schema": "molt.native-worker-origin.v1",
            "process_id": self.process_id,
            "input": {"sha256": self.input_sha256, "size": self.input_size},
            "output": {"sha256": self.output_sha256, "size": self.output_size},
        }


@dataclass(frozen=True, slots=True)
class BackendCodegenOrigin:
    process_id: int | None
    workers: tuple[BackendNativeWorkerOrigin, ...]

    @classmethod
    def from_payload(cls, value: object, *, daemon_pid: int) -> BackendCodegenOrigin:
        if (
            isinstance(value, dict)
            and set(value) == {"kind", "process_id"}
            and value["kind"] == "in-process"
        ):
            process_id = _process_id(value["process_id"])
            if process_id != daemon_pid:
                raise ValueError(
                    "in-process codegen origin differs from serving daemon"
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


@dataclass(frozen=True, slots=True)
class BackendDaemonJobOrigin:
    """Decoded input, producing processes and emitted output retained by the daemon.

    This is not loaded-image or compiler/runtime release admission. Kernel
    process identity, persisted-cache origin and runtime closure remain separate.
    """

    daemon_pid: int
    input: BackendAddressedProgramInput
    output_sha256: str
    output_size: int
    codegen: BackendCodegenOrigin

    @classmethod
    def from_payload(cls, value: object) -> BackendDaemonJobOrigin:
        if (
            not isinstance(value, dict)
            or set(value) != {"schema", "daemon_pid", "input", "output", "codegen"}
            or value["schema"] != "molt.daemon-job-origin.v2"
        ):
            raise ValueError("backend daemon origin schema is invalid")
        pid = _process_id(value["daemon_pid"])
        return cls(
            pid,
            BackendAddressedProgramInput.from_payload(value["input"]),
            *_content(value["output"], "output"),
            BackendCodegenOrigin.from_payload(value["codegen"], daemon_pid=pid),
        )

    def payload(self) -> dict[str, object]:
        return {
            "schema": "molt.daemon-job-origin.v2",
            "daemon_pid": self.daemon_pid,
            "input": self.input.payload(),
            "output": {"sha256": self.output_sha256, "size": self.output_size},
            "codegen": self.codegen.payload(),
        }

    def verify_output(self, path: Path) -> None:
        from molt.toolchain_identity import (
            open_stable_regular_file,
            stable_regular_file_handle_identity,
        )
        from molt.cli.static_archive_identity import static_archive_member_identities

        with open_stable_regular_file(
            path, label="backend daemon emitted output"
        ) as opened:
            observed = stable_regular_file_handle_identity(
                opened, label="backend daemon emitted output"
            )
            if (observed.sha256, observed.size) != (
                self.output_sha256,
                self.output_size,
            ):
                raise ValueError("backend daemon origin names different output bytes")
            if self.codegen.workers:
                members = static_archive_member_identities(opened.path, opened=opened)
                if len(members) != len(self.codegen.workers):
                    raise ValueError(
                        "backend archive origin omits or repeats a worker output"
                    )
                for index, (member, worker) in enumerate(
                    zip(members, self.codegen.workers, strict=True)
                ):
                    if member.member.name != f"molt_{index:08}.o" or (
                        member.sha256,
                        member.member.size,
                    ) != (worker.output_sha256, worker.output_size):
                        raise ValueError(
                            "backend archive member differs from native worker origin"
                        )
