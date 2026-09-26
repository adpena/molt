"""One ordered producer-link authority for Meson-owned source partitions.

The producer's original spans are retained for exact metadata attestation.
Only derived external arguments are handed to the canonical typed link parser;
this module does not interpret external libraries or render linker commands.
"""

from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path, PurePosixPath, PureWindowsPath
from typing import Literal, Mapping

from molt.cli.source_extension_link_arguments import (
    SourceExtensionLinkArgument,
    SourceExtensionLinkScope,
    source_extension_link_arguments,
)
from molt.cli.source_extension_link_requirements import (
    SourceExtensionLinkLoadingPolicy,
)
from molt.cli.source_extension_target import source_extension_link_dialect


_SPAN_DISPOSITIONS = frozenset({"external", "product", "python-provider"})
_SOURCE_DISPOSITIONS = frozenset({"source", "excluded"})


def _canonical_relative_path(value: object, *, field: str) -> Path:
    if (
        not isinstance(value, str)
        or not value
        or "\\" in value
        or ":" in value
        or any(ord(character) < 32 or ord(character) == 127 for character in value)
    ):
        raise ValueError(f"{field} must be a canonical relative path")
    path = PurePosixPath(value)
    if (
        path.is_absolute()
        or PureWindowsPath(value).drive
        or PureWindowsPath(value).root
        or value in {".", ".."}
        or value != path.as_posix()
        or any(part in {".", ".."} for part in path.parts)
    ):
        raise ValueError(f"{field} must be a canonical relative path")
    return Path(*path.parts)


def _canonical_target_id(value: object) -> str:
    if (
        not isinstance(value, str)
        or not value.strip()
        or value != value.strip()
        or any(ord(character) < 32 or ord(character) == 127 for character in value)
    ):
        raise ValueError("source-link target id is invalid")
    return value


def _manifest_path(path: Path, *, build_root: Path) -> str:
    if path.is_absolute():
        try:
            path = path.resolve(strict=False).relative_to(build_root.resolve())
        except ValueError as exc:
            raise ValueError(
                f"source-link member escapes Meson build root: {path}"
            ) from exc
    return _canonical_relative_path(
        path.as_posix(), field="source-link path"
    ).as_posix()


def _canonical_span(value: object) -> SourceExtensionLinkArgument:
    if (
        not isinstance(value, list)
        or not value
        or not all(isinstance(argument, str) for argument in value)
    ):
        raise ValueError("source-link item arguments must be a non-empty string array")
    spans = source_extension_link_arguments(value)
    if len(spans) != 1 or spans[0].arguments != tuple(value):
        raise ValueError("source-link item must contain exactly one canonical span")
    return spans[0]


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkSpan:
    span: SourceExtensionLinkArgument
    disposition: Literal["external", "product", "python-provider"] = "external"

    def __post_init__(self) -> None:
        if self.disposition not in _SPAN_DISPOSITIONS:
            raise ValueError("source-link span disposition is invalid")
        if (self.span.kind == "product") != (self.disposition == "product"):
            raise ValueError("source-link product span has incorrect disposition")


@dataclass(frozen=True, slots=True)
class SourceExtensionSourceArchiveOperand:
    target_id: str
    archive_output: Path
    member_object_paths: tuple[Path, ...]
    span: SourceExtensionLinkArgument | None
    disposition: Literal["source", "excluded"] = "source"
    loading: SourceExtensionLinkLoadingPolicy | None = None

    def __post_init__(self) -> None:
        _canonical_target_id(self.target_id)
        if self.disposition not in _SOURCE_DISPOSITIONS:
            raise ValueError("source-link archive operand identity is invalid")
        if self.disposition == "source":
            recorded_input = self.span is not None and self.span.arguments == (
                f"@build/{self.archive_output.as_posix()}",
            )
            if self.span is None or (
                self.span.kind not in {"input", "forced"} and not recorded_input
            ):
                raise ValueError(
                    "source-link source operand needs an input/forced span"
                )
            if not self.member_object_paths:
                raise ValueError("source-link source archive has no exact members")
            if self.loading not in {
                SourceExtensionLinkLoadingPolicy.DEFAULT,
                SourceExtensionLinkLoadingPolicy.ALL_MEMBERS,
            }:
                raise ValueError("source-link archive loading is invalid")
        elif self.member_object_paths or self.loading is not None:
            raise ValueError("excluded source archive cannot carry selected members")


SourceExtensionLinkPlanItem = (
    SourceExtensionLinkSpan | SourceExtensionSourceArchiveOperand
)


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkProjection:
    primary_target_id: str
    primary_member_objects: tuple[Path, ...]
    items: tuple[SourceExtensionLinkPlanItem, ...]

    def __post_init__(self) -> None:
        _canonical_target_id(self.primary_target_id)
        if not self.primary_member_objects:
            raise ValueError("source-link primary target needs exact eager members")
        if len(set(self.primary_member_objects)) != len(self.primary_member_objects):
            raise ValueError("source-link primary members are duplicated")
        archives: dict[Path, tuple[str, tuple[Path, ...], str]] = {}
        for item in self.items:
            if not isinstance(item, SourceExtensionSourceArchiveOperand):
                continue
            identity = (item.target_id, item.member_object_paths, item.disposition)
            previous = archives.setdefault(item.archive_output, identity)
            if previous != identity:
                raise ValueError(
                    "source-link repeated archive has conflicting member custody: "
                    f"{item.archive_output}"
                )

    @property
    def eager_member_objects(self) -> tuple[Path, ...]:
        return tuple(
            dict.fromkeys(
                (
                    *self.primary_member_objects,
                    *(
                        member
                        for item in self.items
                        if isinstance(item, SourceExtensionSourceArchiveOperand)
                        and item.disposition == "source"
                        and item.loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
                        for member in item.member_object_paths
                    ),
                )
            )
        )

    @property
    def lazy_source_operands(self) -> tuple[SourceExtensionSourceArchiveOperand, ...]:
        return tuple(
            item
            for item in self.items
            if isinstance(item, SourceExtensionSourceArchiveOperand)
            and item.disposition == "source"
            and item.loading is SourceExtensionLinkLoadingPolicy.DEFAULT
        )

    def producer_arguments(self) -> tuple[str, ...]:
        return tuple(
            argument
            for item in self.items
            if (span := item.span) is not None
            for argument in span.arguments
        )

    def external_arguments(self) -> tuple[str, ...]:
        return tuple(
            argument
            for item in self.items
            if isinstance(item, SourceExtensionLinkSpan)
            and item.disposition == "external"
            for argument in item.span.arguments
        )

    def validate_dialect(self, target_triple: str) -> None:
        dialect = source_extension_link_dialect(target_triple)
        scope = SourceExtensionLinkScope()
        for item in self.items:
            span = item.span
            if span is None:
                continue
            span.validate_dialect(dialect)
            scope.advance(span)
            if isinstance(item, SourceExtensionSourceArchiveOperand) and (
                item.disposition == "source"
            ):
                expected = (
                    SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
                    if scope.whole_archive or span.kind == "forced"
                    else SourceExtensionLinkLoadingPolicy.DEFAULT
                )
                if item.loading is not expected:
                    raise ValueError(
                        f"source-link loading differs from producer span: {item.target_id}"
                    )
        scope.finish()

    def manifest_payload(self, *, build_root: Path) -> dict[str, object]:
        items: list[dict[str, object]] = []
        for item in self.items:
            span = item.span
            row: dict[str, object] = {
                "disposition": item.disposition,
                "arguments": [] if span is None else list(span.arguments),
            }
            if isinstance(item, SourceExtensionSourceArchiveOperand):
                row.update(
                    {
                        "target_id": item.target_id,
                        "archive_output": _manifest_path(
                            item.archive_output, build_root=build_root
                        ),
                        "member_objects": [
                            _manifest_path(path, build_root=build_root)
                            for path in item.member_object_paths
                        ],
                    }
                )
                if item.loading is not None:
                    row["loading"] = item.loading.value
            items.append(row)
        return {
            "schema_version": 1,
            "primary_target_id": self.primary_target_id,
            "primary_member_objects": [
                _manifest_path(path, build_root=build_root)
                for path in self.primary_member_objects
            ],
            "items": items,
        }

    @classmethod
    def from_manifest(cls, payload: object) -> SourceExtensionLinkProjection:
        if not isinstance(payload, Mapping) or set(payload) != {
            "schema_version",
            "primary_target_id",
            "primary_member_objects",
            "items",
        }:
            raise ValueError("source-link projection has invalid shape")
        if type(payload["schema_version"]) is not int or payload["schema_version"] != 1:
            raise ValueError("source-link projection schema version is unsupported")
        target_id = payload["primary_target_id"]
        raw_primary = payload["primary_member_objects"]
        raw_items = payload["items"]
        if not isinstance(target_id, str) or not target_id:
            raise ValueError("source-link primary target id is invalid")
        if not isinstance(raw_primary, list) or not raw_primary:
            raise ValueError("source-link primary members must be a nonempty array")
        if not isinstance(raw_items, list):
            raise ValueError("source-link items must be an array")
        primary = tuple(
            _canonical_relative_path(value, field="primary member")
            for value in raw_primary
        )
        items: list[SourceExtensionLinkPlanItem] = []
        for index, raw in enumerate(raw_items):
            if not isinstance(raw, Mapping):
                raise ValueError(f"source-link item {index} must be an object")
            disposition = raw.get("disposition")
            if not isinstance(disposition, str):
                raise ValueError(f"source-link item {index} has invalid disposition")
            if disposition in _SPAN_DISPOSITIONS:
                if set(raw) != {"disposition", "arguments"}:
                    raise ValueError(f"source-link item {index} has invalid shape")
                span = _canonical_span(raw["arguments"])
                items.append(SourceExtensionLinkSpan(span, disposition))
                continue
            if disposition not in _SOURCE_DISPOSITIONS:
                raise ValueError(f"source-link item {index} has invalid disposition")
            expected = {
                "disposition",
                "arguments",
                "target_id",
                "archive_output",
                "member_objects",
            }
            if disposition == "source":
                expected.add("loading")
            if set(raw) != expected:
                raise ValueError(f"source-link item {index} has invalid shape")
            source_id = raw["target_id"]
            if not isinstance(source_id, str) or not source_id:
                raise ValueError(f"source-link item {index} has invalid target id")
            raw_members = raw["member_objects"]
            if not isinstance(raw_members, list):
                raise ValueError(f"source-link item {index} members must be an array")
            raw_arguments = raw["arguments"]
            span = (
                None
                if disposition == "excluded" and raw_arguments == []
                else _canonical_span(raw_arguments)
            )
            try:
                loading = (
                    SourceExtensionLinkLoadingPolicy(raw["loading"])
                    if disposition == "source"
                    else None
                )
            except (TypeError, ValueError) as exc:
                raise ValueError(
                    f"source-link item {index} loading is invalid"
                ) from exc
            items.append(
                SourceExtensionSourceArchiveOperand(
                    target_id=source_id,
                    archive_output=_canonical_relative_path(
                        raw["archive_output"], field="archive output"
                    ),
                    member_object_paths=tuple(
                        _canonical_relative_path(value, field="archive member")
                        for value in raw_members
                    ),
                    span=span,
                    disposition=disposition,
                    loading=loading,
                )
            )
        return cls(target_id, primary, tuple(items))
