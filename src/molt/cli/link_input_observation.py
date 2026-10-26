"""LLD input-buffer observations for explicitly requested admission captures.

The linker reproducer owns the input bytes; final-link publication owns the
observation and output together. This module neither launches tools nor grants
execution or release authority. A sealed input namespace and independently
verified producing image are required at the admitting consumer.
"""

from __future__ import annotations

from dataclasses import dataclass
from contextlib import nullcontext
from typing import BinaryIO, Callable, ContextManager
import hashlib
from pathlib import Path, PurePath, PurePosixPath, PureWindowsPath
import tarfile
from typing import Mapping, Literal, TypedDict

from molt.python_identity_common import canonical_absolute_path
from molt.toolchain_identity import (
    open_stable_regular_file,
    stable_regular_file_handle_identity,
)

SCHEMA = "molt.link-input-observation.v2"
_MAX_MEMBERS = 16384
_MAX_BYTES = 8 * 1024 * 1024 * 1024


def _canonical_archive_path(value: object) -> str:
    """Validate an LLD member coordinate without interpreting it as a host path."""
    if not isinstance(value, str) or not value or "\x00" in value:
        raise ValueError("link input archive coordinate is noncanonical")
    path = PurePosixPath(value)
    if (
        not path.parts
        or path.is_absolute()
        or path.as_posix() != value
        or ".." in path.parts
    ):
        raise ValueError("link input archive coordinate is noncanonical")
    return value


def lld_archive_path(path: str | PurePath) -> str:
    """Project a canonical host path using LLVM lld::relativeToRoot.

    Drive letters lose their colon; UNC server/share components remain. This
    one-way coordinate names retained archive bytes, never an inferred host file.
    """
    if not isinstance(path, (str, PurePath)):
        raise ValueError("LLD input requires a canonical host path")
    host = canonical_absolute_path(str(path))
    prefix = ""
    if isinstance(host, PureWindowsPath):
        prefix = (
            host.drive[:-1]
            if host.drive.endswith(":")
            else host.drive.removeprefix("\\\\").replace("\\", "/")
        )
    coordinate = "/".join((*((prefix,) if prefix else ()), *host.parts[1:]))
    return _canonical_archive_path(coordinate)


def _content(value: object) -> tuple[int, str]:
    if not isinstance(value, Mapping) or set(value) != {"size", "sha256"}:
        raise ValueError("link input content identity is not exact")
    size, digest = value["size"], value["sha256"]
    if (
        type(size) is not int
        or size < 0
        or not isinstance(digest, str)
        or len(digest) != 64
        or any(c not in "0123456789abcdef" for c in digest)
    ):
        raise ValueError("link input content identity is invalid")
    return size, digest


class ContentRecord(TypedDict):
    size: int
    sha256: str


class LinkInputFileRecord(ContentRecord):
    archive_path: str


class LinkInputRecord(TypedDict):
    schema: Literal["molt.link-input-observation.v2"]
    dialect: Literal["elf-gnu", "wasm"]
    linker_path: str
    inputs: list[LinkInputFileRecord]
    roles: dict[str, str]
    reproducer: ContentRecord


class WasmSourceInputRecords(TypedDict):
    compiler_output: ContentRecord
    runtime: ContentRecord
    deployment_runtime: ContentRecord | None


class WasmLinkInputRecord(TypedDict):
    schema: Literal["molt.wasm-link-inputs.v2"]
    source_inputs: WasmSourceInputRecords
    links: dict[str, LinkInputRecord]


class CompilationCacheRecord(TypedDict):
    compiler: str | None
    stdlib: str | None


class CompilationInputRecord(TypedDict):
    schema: Literal["molt.compilation-inputs.v3"]
    compiler: dict[str, object]
    stdlib: dict[str, object] | None
    cache: CompilationCacheRecord
    link: LinkInputRecord | WasmLinkInputRecord


def _content_record(value: object) -> ContentRecord:
    size, digest = _content(value)
    return {"size": size, "sha256": digest}


@dataclass(frozen=True, slots=True)
class LinkInputObservation:
    """One actual LLD read inventory and the semantic inputs it must contain."""

    dialect: Literal["elf-gnu", "wasm"]
    linker_path: str
    inputs: tuple[tuple[str, int, str], ...]
    roles: tuple[tuple[str, str], ...]
    reproducer_size: int
    reproducer_sha256: str

    def as_record(self) -> LinkInputRecord:
        return {
            "schema": "molt.link-input-observation.v2",
            "dialect": self.dialect,
            "linker_path": self.linker_path,
            "inputs": [
                {"archive_path": p, "size": n, "sha256": h} for p, n, h in self.inputs
            ],
            "roles": dict(self.roles),
            "reproducer": {
                "size": self.reproducer_size,
                "sha256": self.reproducer_sha256,
            },
        }

    @classmethod
    def from_record(cls, value: object) -> LinkInputObservation:
        if (
            not isinstance(value, Mapping)
            or set(value)
            != {"schema", "dialect", "linker_path", "inputs", "roles", "reproducer"}
            or value["schema"] != SCHEMA
            or value["dialect"] not in {"elf-gnu", "wasm"}
        ):
            raise ValueError("link input observation contract is invalid")
        linker = str(canonical_absolute_path(value["linker_path"]))
        rows, roles = value["inputs"], value["roles"]
        if not isinstance(rows, list) or not 1 <= len(rows) <= _MAX_MEMBERS:
            raise ValueError("link input observation has no bounded input inventory")
        inputs = []
        for row in rows:
            if not isinstance(row, Mapping) or set(row) != {
                "archive_path",
                "size",
                "sha256",
            }:
                raise ValueError("link input observation row is invalid")
            p = _canonical_archive_path(row["archive_path"])
            size, digest = _content({k: row[k] for k in ("size", "sha256")})
            inputs.append((p, size, digest))
        if [p for p, _, _ in inputs] != sorted({p for p, _, _ in inputs}):
            raise ValueError("link input observations are not sorted and unique")
        if sum(n for _, n, _ in inputs) > _MAX_BYTES:
            raise ValueError("link input inventory exceeds its byte bound")
        paths = {p for p, _, _ in inputs}
        if (
            not isinstance(roles, Mapping)
            or not roles
            or any(
                not isinstance(k, str)
                or not k
                or not isinstance(v, str)
                or v not in paths
                for k, v in roles.items()
            )
        ):
            raise ValueError("link input semantic role has no observed producer")
        size, digest = _content(value["reproducer"])
        if not 0 < size <= _MAX_BYTES:
            raise ValueError("link reproducer is outside its capture bound")
        return cls(
            "elf-gnu" if value["dialect"] == "elf-gnu" else "wasm",
            linker,
            tuple(inputs),
            tuple(sorted(roles.items())),
            size,
            digest,
        )


def capture_link_inputs(
    reproducer: Path,
    *,
    dialect: str,
    linker_path: str,
    expected: Mapping[str, tuple[str, Mapping[str, object]]],
    input_sink: Callable[[str, int], ContextManager[BinaryIO | None]] | None = None,
) -> LinkInputObservation:
    """Read LLD's retained buffers without extracting or rehashing selected paths.

    Expected roles and sink keys use relative archive coordinates; linker_path
    is an exact host coordinate. Neither is inferred from the reviewing host.
    Every semantic role is compared to independent upstream content. Unknown
    inputs remain in the complete inventory. Repeated paths and nonregular
    entries reject. LLD itself omits repeated opens, so this is intentionally
    not an immutable-input or process-image assertion. An admission-only sink
    may retain these same buffers in a private, unpublished root; it must not
    publish that root unless this complete parser and its mutation fence pass.
    """
    linker_path = str(canonical_absolute_path(linker_path))
    for coordinate, content in expected.values():
        _canonical_archive_path(coordinate)
        _content(content)
    prefix: str | None = None
    observed: dict[str, tuple[int, str]] = {}
    administrative: set[str] = set()
    names: set[str] = set()
    total = 0
    with open_stable_regular_file(reproducer, label="LLD input reproducer") as opened:
        identity = stable_regular_file_handle_identity(
            opened, label="LLD input reproducer", max_bytes=_MAX_BYTES
        )
        with tarfile.open(fileobj=opened.stream, mode="r:") as archive:
            for member in archive:
                if len(names) >= _MAX_MEMBERS or member.name in names:
                    raise ValueError(
                        "link reproducer repeats or exceeds its input inventory"
                    )
                names.add(member.name)
                p = PurePosixPath(_canonical_archive_path(member.name))
                if prefix is None and p.parts:
                    prefix = p.parts[0]
                    administrative = {prefix + "/response.txt", prefix + "/version.txt"}
                if (
                    not member.isfile()
                    or len(p.parts) < 2
                    or p.parts[0] != prefix
                    or member.size < 0
                ):
                    raise ValueError("link reproducer contains a noncanonical input")
                total += member.size
                if total > _MAX_BYTES:
                    raise ValueError(
                        "link reproducer expanded bytes exceed the capture bound"
                    )
                input_path = _canonical_archive_path("/".join(p.parts[1:]))
                stream = archive.extractfile(member)
                if stream is None:
                    raise ValueError("link reproducer input bytes are unavailable")
                digest = hashlib.sha256()
                remaining = member.size
                sink = (
                    input_sink(input_path, member.size)
                    if input_sink is not None and member.name not in administrative
                    else nullcontext(None)
                )
                with stream, sink as destination:
                    while remaining:
                        block = stream.read(min(remaining, 64 * 1024))
                        if not block:
                            raise ValueError("link reproducer input is truncated")
                        remaining -= len(block)
                        digest.update(block)
                        if destination is not None:
                            destination.write(block)
                if member.name not in administrative:
                    observed[input_path] = (
                        member.size,
                        digest.hexdigest(),
                    )
        if not administrative <= names:
            raise ValueError("link reproducer omitted its command or version")
        # The same retained handle is fenced and rehashed after parsing.
        closed = stable_regular_file_handle_identity(
            opened, label="LLD input reproducer after parse", max_bytes=_MAX_BYTES
        )
        if (closed.size, closed.sha256) != (identity.size, identity.sha256):
            raise ValueError("link reproducer changed during admission")
    roles = {}
    for role, (name, content) in expected.items():
        if observed.get(name) != _content(content):
            raise ValueError(f"linker did not consume the admitted {role} bytes")
        roles[role] = name
    return LinkInputObservation.from_record(
        {
            "schema": SCHEMA,
            "dialect": dialect,
            "linker_path": linker_path,
            "inputs": [
                {"archive_path": p, "size": n, "sha256": h}
                for p, (n, h) in sorted(observed.items())
            ],
            "roles": roles,
            "reproducer": {"size": identity.size, "sha256": identity.sha256},
        }
    )


def validate_wasm_link_inputs(value: object) -> WasmLinkInputRecord:
    """Validate distinct source-read and linker-buffer observations, not admission."""
    if (
        not isinstance(value, dict)
        or set(value) != {"schema", "source_inputs", "links"}
        or value["schema"] != "molt.wasm-link-inputs.v2"
    ):
        raise ValueError("WASM link input observation contract is invalid")
    sources, links = value["source_inputs"], value["links"]
    if (
        not isinstance(sources, dict)
        or set(sources) != {"compiler_output", "runtime", "deployment_runtime"}
        or not isinstance(links, dict)
        or set(links) not in ({"linked"}, {"linked", "app"})
    ):
        raise ValueError("WASM link input observation family is incomplete")
    compiler_output = _content_record(sources["compiler_output"])
    runtime = _content_record(sources["runtime"])
    deployment_runtime = None
    if "app" in links:
        deployment_runtime = _content_record(sources["deployment_runtime"])
    elif sources["deployment_runtime"] is not None:
        raise ValueError("monolithic WASM observation has a deployment runtime")
    admitted_links: dict[str, LinkInputRecord] = {}
    for role, raw in links.items():
        observation = LinkInputObservation.from_record(raw)
        if observation.dialect != "wasm" or set(dict(observation.roles)) != (
            {"compiler_rewritten", "runtime"}
            if role == "linked"
            else {"compiler_rewritten"}
        ):
            raise ValueError("WASM linker observation has different semantic inputs")
        if role == "linked":
            observed = {p: (n, h) for p, n, h in observation.inputs}
            if observed[dict(observation.roles)["runtime"]] != (
                runtime["size"],
                runtime["sha256"],
            ):
                raise ValueError("WASM linker consumed a different runtime buffer")
        admitted_links[role] = observation.as_record()
    return {
        "schema": "molt.wasm-link-inputs.v2",
        "source_inputs": {
            "compiler_output": compiler_output,
            "runtime": runtime,
            "deployment_runtime": deployment_runtime,
        },
        "links": admitted_links,
    }


def compilation_input_observation(
    origin: object, link: object, *, stdlib: object, cache: object
) -> CompilationInputRecord:
    """Join consumed linker inputs to the retained emitter; no image assertion."""
    from molt.backend_protocol import BackendEmissionOrigin, BackendStdlibOrigin

    if not isinstance(cache, dict) or set(cache) != {"compiler", "stdlib"}:
        raise ValueError("compilation cache coordinates are not exact")
    for path in cache.values():
        if path is not None:
            canonical_absolute_path(path)
    cache_record: CompilationCacheRecord = {
        "compiler": cache["compiler"],
        "stdlib": cache["stdlib"],
    }
    producer = BackendEmissionOrigin.from_payload(origin)
    constituent_origin = (
        BackendStdlibOrigin.from_payload(stdlib) if stdlib is not None else None
    )
    expected = (producer.output_size, producer.output_sha256)
    admitted: LinkInputRecord | WasmLinkInputRecord
    if isinstance(link, dict) and link.get("schema") == "molt.wasm-link-inputs.v2":
        if constituent_origin is not None:
            raise ValueError("WASM compiler cannot name a native stdlib archive")
        wasm = validate_wasm_link_inputs(link)
        admitted = wasm
        if _content(wasm["source_inputs"]["compiler_output"]) != expected:
            raise ValueError("WASM source buffer differs from compiler emission")
    else:
        observed = LinkInputObservation.from_record(link)
        admitted = observed.as_record()
        roles = {"compiler_output", "runtime"}
        if constituent_origin is not None:
            roles.add("stdlib")
        if observed.dialect != "elf-gnu" or set(dict(observed.roles)) != roles:
            raise ValueError("native capture omitted compiler or runtime input")
        inputs = {p: (n, h) for p, n, h in observed.inputs}
        if inputs[dict(observed.roles)["compiler_output"]] != expected:
            raise ValueError("native linker buffer differs from compiler emission")
        if constituent_origin is not None and inputs[
            dict(observed.roles)["stdlib"]
        ] != (constituent_origin.output_size, constituent_origin.output_sha256):
            raise ValueError(
                "native linker buffer differs from original stdlib emissions"
            )
    return {
        "schema": "molt.compilation-inputs.v3",
        "compiler": producer.payload(),
        "stdlib": constituent_origin.payload()
        if constituent_origin is not None
        else None,
        "link": admitted,
        "cache": cache_record,
    }


def validate_compilation_input_observation(value: object) -> CompilationInputRecord:
    if (
        not isinstance(value, dict)
        or set(value) != {"schema", "compiler", "stdlib", "cache", "link"}
        or value["schema"] != "molt.compilation-inputs.v3"
    ):
        raise ValueError("compilation input observation contract is invalid")
    return compilation_input_observation(
        value["compiler"], value["link"], stdlib=value["stdlib"], cache=value["cache"]
    )


def capture_native_link_preparation(
    dependencies: Path,
    *,
    source: Path,
    driver: Path,
    cwd: Path,
) -> dict[str, object]:
    """Snapshot driver-declared C inputs for a later sealed compilation.

    A dependency path is a selection hint, not an observation of consumed
    bytes. These snapshots only prepare immutable inputs for the fresh build;
    they cannot replace its compiler image, linker buffers or output receipt.
    """
    from molt.cli.dependency_files import parse_make_depfile
    from molt.toolchain_identity import (
        stable_regular_file_identity,
        verify_stable_regular_file_identity,
    )

    report = stable_regular_file_identity(
        dependencies, label="native preparation dependencies"
    )
    if report.size > 1024 * 1024:
        raise ValueError("native preparation dependency report exceeds its bound")
    paths, error = parse_make_depfile(
        dependencies, cwd=cwd, producer="admission native driver"
    )
    if (
        error is not None
        or paths is None
        or len(paths) > 16384
        or source.resolve() not in {row.resolved for row in paths}
    ):
        raise ValueError(error or "native preparation report omitted its source unit")

    def record(lexical: Path, resolved: Path) -> dict[str, object]:
        identity = stable_regular_file_identity(
            resolved, label="native preparation selected file"
        )
        return {
            "path": str(lexical),
            "content_path": str(identity.path),
            "size": identity.size,
            "sha256": identity.sha256,
        }

    files = [
        record(row.lexical, row.resolved)
        for row in sorted(paths, key=lambda row: str(row.lexical))
    ]
    verify_stable_regular_file_identity(
        report, label="native preparation dependencies", hash_content=True
    )
    return validate_native_link_preparation(
        {
            "schema": "molt.native-link-preparation.v2",
            "driver": record(driver.absolute(), driver.resolve()),
            "source": str(source.absolute()),
            "files": files,
        }
    )


def validate_native_link_preparation(value: object) -> dict[str, object]:
    if (
        not isinstance(value, dict)
        or set(value) != {"schema", "driver", "source", "files"}
        or value["schema"] != "molt.native-link-preparation.v2"
        or not isinstance(value["files"], list)
        or not 1 <= len(value["files"]) <= 16384
    ):
        raise ValueError("native link preparation record is not exact")

    def path(raw: object) -> str:
        return str(canonical_absolute_path(raw))

    names = []
    for row in [value["driver"], *value["files"]]:
        if not isinstance(row, dict) or set(row) != {
            "path",
            "content_path",
            "size",
            "sha256",
        }:
            raise ValueError("native preparation file is not exact")
        path(row["path"])
        path(row["content_path"])
        _content({name: row[name] for name in ("size", "sha256")})
    names = [row["path"] for row in value["files"]]
    if names != sorted(set(names)) or path(value["source"]) not in names:
        raise ValueError("native preparation repeats inputs or omits its source")
    return value
