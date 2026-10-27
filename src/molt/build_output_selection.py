"""Side-effect-free output selection, shared by builds and their input records."""

from __future__ import annotations

from dataclasses import dataclass
import os
from pathlib import Path


def select_output_path(
    output: str | None,
    default: Path,
    *,
    out_dir: Path | None,
    project_root: Path,
) -> Path:
    """Select a coordinate without creating directories or entering build custody."""
    if not output:
        return default
    path = Path(output).expanduser()
    if not path.is_absolute():
        path = (out_dir if out_dir is not None else project_root) / path
    if output.endswith(os.sep) or (os.altsep and output.endswith(os.altsep)):
        return path / default.name
    try:
        if path.exists() and path.is_dir():
            return path / default.name
    except OSError:
        pass
    return path


@dataclass(frozen=True, slots=True)
class BuildOutputSelection:
    """The actual layout decision of a successful native binary build.

    The request key and input ledger bind the entry, project and configuration
    that produced this context. Replaying the same selector also checks the
    directory/file decision and resolved destination, without resolving or
    parsing that entry again. Absence and a regular output file select the same
    coordinate: publishing the first binary must not invalidate its own record.
    """

    output: str | None
    default: Path
    out_dir: Path | None
    project_root: Path
    path: Path
    resolved_path: Path

    @classmethod
    def capture(
        cls,
        output: str | None,
        default: Path,
        *,
        out_dir: Path | None,
        project_root: Path,
    ) -> BuildOutputSelection:
        selected = select_output_path(
            output, default, out_dir=out_dir, project_root=project_root
        )
        return cls(
            output,
            default.absolute(),
            None if out_dir is None else out_dir.absolute(),
            project_root.absolute(),
            selected.absolute(),
            Path(os.path.realpath(selected)),
        )

    def verify(self) -> None:
        current = select_output_path(
            self.output,
            self.default,
            out_dir=self.out_dir,
            project_root=self.project_root,
        )
        if (
            current != self.path
            or Path(os.path.realpath(current)) != self.resolved_path
        ):
            raise ValueError(f"output selection changed: {self.path}")

    def to_dict(self) -> dict[str, str | None]:
        return {
            "output": self.output,
            "default": os.fspath(self.default),
            "out_dir": None if self.out_dir is None else os.fspath(self.out_dir),
            "project_root": os.fspath(self.project_root),
            "path": os.fspath(self.path),
            "resolved_path": os.fspath(self.resolved_path),
        }

    @classmethod
    def from_dict(cls, payload: object) -> BuildOutputSelection:
        if not isinstance(payload, dict) or set(payload) != {
            "output",
            "default",
            "out_dir",
            "project_root",
            "path",
            "resolved_path",
        }:
            raise ValueError("invalid build output selection")
        output = payload["output"]
        if output is not None and not isinstance(output, str):
            raise ValueError("invalid build output selector")

        def path(name: str) -> Path:
            value = payload[name]
            if not isinstance(value, str) or not value or "\0" in value:
                raise ValueError(f"invalid build output {name}")
            result = Path(value)
            if not result.is_absolute():
                raise ValueError(f"build output {name} is not absolute")
            return result

        return cls(
            output,
            path("default"),
            None if payload["out_dir"] is None else path("out_dir"),
            path("project_root"),
            path("path"),
            path("resolved_path"),
        )
