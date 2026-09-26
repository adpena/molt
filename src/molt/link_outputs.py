"""Final-link publication paths: one family and no destructive input aliases."""

from __future__ import annotations

from pathlib import Path
from collections.abc import Iterable, Mapping


def validate_link_output_paths(
    outputs: Mapping[str, Path], *, inputs: Iterable[Path] = ()
) -> None:
    if not outputs:
        raise ValueError("final link output family is empty")
    owners: dict[Path, str] = {}
    for role, path in outputs.items():
        if not isinstance(role, str) or not role:
            raise ValueError("final link output role must be a nonempty string")
        resolved = path.resolve()
        if resolved in owners:
            raise ValueError(
                f"Link output roles {owners[resolved]!r} and {role!r} "
                f"alias the same path: {path}"
            )
        owners[resolved] = role
    for path in inputs:
        role = owners.get(path.resolve())
        if role is not None:
            raise ValueError(f"Link output role {role!r} aliases input: {path}")


def wasm_link_output_paths(
    linked: Path, *, split_output_dir: Path | None = None, inputs: Iterable[Path] = ()
) -> dict[str, Path]:
    outputs = {"linked": linked}
    if split_output_dir is not None:
        outputs.update(
            app=split_output_dir / "app.wasm",
            runtime=split_output_dir / "molt_runtime.wasm",
            size_attestation=split_output_dir / "wasm_size_attestation.json",
        )
    validate_link_output_paths(outputs, inputs=inputs)
    return outputs
