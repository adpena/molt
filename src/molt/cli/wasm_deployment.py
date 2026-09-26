"""Private WASM deployment generations; publication belongs to final-link custody."""

from __future__ import annotations

import contextlib
import os
import shutil
import warnings
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Iterator, Mapping

from molt.artifact_publication import (
    discard_staged_output,
    publication_payload_snapshot,
)
from molt.cli.link_fingerprints import FinalLinkReceiptRequest, publish_link_outputs
from molt.cli.models import _PreparedNonNativeResult
from molt.file_deletion import delete_path
from molt.file_publication import staged_file_path
from molt.link_outputs import validate_link_output_paths
from molt.toolchain_identity import (
    StableRegularFileVersion,
    stable_regular_file_version,
    verify_stable_regular_file_identity,
)


PRECOMPILE_ENV = {
    "cwasm": "MOLT_WASM_PRECOMPILED_PATH",
    "runtime_cwasm": "MOLT_WASM_PRECOMPILED_RUNTIME_PATH",
}


@dataclass(frozen=True)
class WasmDeploymentSources:
    """Mutation custody across key derivation and private deployment rendering."""

    files: tuple[StableRegularFileVersion, ...]
    packages: Mapping[Path, tuple[Path, ...]]
    include: Callable[[Path, Path], bool]

    @classmethod
    def capture(
        cls,
        paths: tuple[Path, ...],
        packages: Mapping[Path, tuple[Path, ...]],
        include: Callable[[Path, Path], bool],
    ) -> WasmDeploymentSources:
        return cls(
            tuple(
                stable_regular_file_version(path, label="WASM deployment source")
                for path in dict.fromkeys(paths)
            ),
            dict(packages),
            include,
        )

    def verify_files(self) -> None:
        for identity in self.files:
            verify_stable_regular_file_identity(
                identity, label="WASM deployment source"
            )

    def verify(self) -> None:
        # Source namespace locks are short reads, never held across a linker or
        # host subprocess. Publication starts only after this snapshot exits.
        with publication_payload_snapshot(
            self.packages, include=self.include
        ) as current:
            if current != self.packages:
                raise ValueError(
                    "WASM deployment package membership changed after input capture"
                )
            self.verify_files()


@dataclass(frozen=True)
class WasmDeploymentPlan:
    outputs: Mapping[str, Path]
    root: Path
    split: bool
    removals: tuple[Path, ...]

    @classmethod
    def create(
        cls,
        core: Mapping[str, Path],
        *,
        output_root: Path,
        split: bool,
        loader_assets: tuple[str, ...],
        target_feature_asset: str,
        bundle: bool,
        precompile: bool,
    ) -> WasmDeploymentPlan:
        root = output_root if split else core["linked"].parent
        outputs = {**core, "manifest": root / "manifest.json"}
        obsolete = {root / "bundle.tar", root / "wrangler.toml"} if split else set()
        if split:
            outputs.update(
                worker_js=root / "worker.js",
                wrangler_config=root / "wrangler.jsonc",
            )
            outputs.update({f"asset:{name}": root / name for name in loader_assets})
            outputs["target_features"] = root / target_feature_asset
            if bundle:
                outputs["bundle_tar"] = root / "bundle.tar"
        sources = {"cwasm": core["app"] if split else core["linked"]}
        if split:
            sources["runtime_cwasm"] = core["runtime"]
        for role, source in sources.items():
            default = source.with_suffix(".molt.cwasm")
            obsolete.add(default)
            if precompile:
                override = os.environ.get(PRECOMPILE_ENV[role])
                if override == "":
                    raise ValueError(f"{PRECOMPILE_ENV[role]} must not be empty")
                outputs[role] = Path(override).absolute() if override else default
        validate_link_output_paths(outputs)
        return cls(
            outputs, root, split, tuple(sorted(obsolete - set(outputs.values())))
        )

    def artifacts(self) -> dict[str, str]:
        roles = {
            "linked": "linked_wasm",
            "app": "app_wasm",
            "runtime": "runtime_wasm",
            "manifest": "manifest",
            "worker_js": "worker_js",
            "wrangler_config": "wrangler_config",
            "bundle_tar": "bundle_tar",
            "cwasm": "cwasm",
            "runtime_cwasm": "runtime_cwasm",
        }
        return {
            roles[role]: str(path)
            for role, path in self.outputs.items()
            if role in roles
        }


@dataclass(frozen=True)
class WasmDeploymentGeneration:
    plan: WasmDeploymentPlan
    root: Path
    outputs: Mapping[str, Path]

    @classmethod
    @contextlib.contextmanager
    def prepare(cls, plan: WasmDeploymentPlan) -> Iterator[WasmDeploymentGeneration]:
        plan.root.mkdir(parents=True, exist_ok=True)
        root = staged_file_path(plan.root / "manifest.json", purpose="wasm-generation")
        root.mkdir()
        try:
            outputs: dict[str, Path] = {}
            for role, final in plan.outputs.items():
                # Keep the execution manifest's modules adjacent, preserving names.
                # Other destinations (custom linked/precompile paths) remain private.
                if final.parent == plan.root or final.is_relative_to(plan.root):
                    path = root / final.relative_to(plan.root)
                else:
                    path = root / role / final.name
                path.parent.mkdir(parents=True, exist_ok=True)
                outputs[role] = path
            validate_link_output_paths(outputs)
            yield cls(plan, root, outputs)
        finally:
            removed, error = delete_path(root)
            if not removed:
                warnings.warn(
                    f"Private WASM generation cleanup failed at {root}: {error}",
                    RuntimeWarning,
                    stacklevel=2,
                )

    def precompile_environment(self) -> dict[str, str]:
        env = os.environ.copy()
        for role, name in PRECOMPILE_ENV.items():
            if role in self.outputs:
                env[name] = str(self.outputs[role].resolve())
            else:
                env.pop(name, None)
        return env

    def publish(self, receipt: FinalLinkReceiptRequest | None) -> None:
        candidates: dict[str, tuple[Path, Path]] = {}
        try:
            for role, private in self.outputs.items():
                final = self.plan.outputs[role]
                final.parent.mkdir(parents=True, exist_ok=True)
                stage = staged_file_path(final, purpose="wasm-deployment")
                candidates[role] = (stage, final)
                shutil.copyfile(private, stage)
            publish_link_outputs(
                candidates,
                receipt=receipt,
                removals=self.plan.removals,
                retire_previous_outputs_under=self.plan.root,
            )
        finally:
            for stage, _ in candidates.values():
                discard_staged_output(stage)

    def public_result(
        self, result: _PreparedNonNativeResult
    ) -> _PreparedNonNativeResult:
        reverse = {path: self.plan.outputs[role] for role, path in self.outputs.items()}
        reverse[self.root] = self.plan.root

        def path(value: Path | None) -> Path | None:
            return reverse.get(value, value) if value is not None else None

        def text(value: str) -> str:
            for private, public in sorted(
                reverse.items(), key=lambda pair: -len(str(pair[0]))
            ):
                value = value.replace(str(private), str(public))
            return value

        return _PreparedNonNativeResult(
            primary_output=reverse.get(result.primary_output, result.primary_output),
            consumer_output=reverse.get(result.consumer_output, result.consumer_output),
            bundle_root=path(result.bundle_root),
            linked_output_path=path(result.linked_output_path),
            success_messages=[text(message) for message in result.success_messages],
            extra_fields={
                key: text(value) if isinstance(value, str) else value
                for key, value in result.extra_fields.items()
            },
            artifacts={
                key: text(value) for key, value in (result.artifacts or {}).items()
            },
        )
