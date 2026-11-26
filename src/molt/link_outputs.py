"""Final-link publication paths: one family and no destructive input aliases."""

from __future__ import annotations

from pathlib import Path
from collections.abc import Iterable, Mapping


def link_selection_path(output: Path) -> Path:
    return output.with_name(output.name + ".molt-link-selection.json")


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


def wasm_artifact_paths(outputs: Mapping[str, Path]) -> dict[str, str]:
    """Public artifact labels for the final-link publication's roles."""
    roles = {
        "wasm": "wasm",
        "linked": "linked_wasm",
        "optimizer": "wasm_optimizer_attestation",
        "app": "app_wasm",
        "runtime": "runtime_wasm",
        "manifest": "manifest",
        "worker_js": "worker_js",
        "wrangler_config": "wrangler_config",
        "bundle_tar": "bundle_tar",
        "cwasm": "cwasm",
        "runtime_cwasm": "runtime_cwasm",
    }
    return {roles[role]: str(path) for role, path in outputs.items() if role in roles}


def wasm_link_output_paths(
    linked: Path | None,
    *,
    split_output_dir: Path | None = None,
    inputs: Iterable[Path] = (),
    external_selection: bool = False,
    optimize: bool = False,
) -> dict[str, Path]:
    outputs: dict[str, Path] = {}
    if linked is not None:
        outputs["linked"] = linked
    if optimize:
        from molt.wasm_optimizer_identity import wasm_optimizer_attestation_path

        if linked is None:
            raise ValueError("WASM optimization requires a monolithic linked output")
        outputs["optimizer"] = wasm_optimizer_attestation_path(linked)
    if split_output_dir is not None:
        outputs.update(
            app=split_output_dir / "app.wasm",
            runtime=split_output_dir / "molt_runtime.wasm",
        )
    if external_selection and outputs:
        outputs["selection"] = link_selection_path(linked or outputs["app"])
    validate_link_output_paths(outputs, inputs=inputs)
    return outputs
