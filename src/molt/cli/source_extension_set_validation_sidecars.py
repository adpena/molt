"""One sidecar validation pass retaining current language, input and binary custody."""

from __future__ import annotations
from pathlib import Path
from typing import Any, Mapping, cast
from molt.cli.source_extension_manifest_codec import (
    _manifest_sequence,
    _object_unit_sha256,
)
from molt.cli.source_extension_object_closure import (
    SourceExtensionObjectClosureError,
    validate_source_extension_object_closure,
    validate_source_extension_object_closure_sources,
)
from molt.cli.source_extensions import validate_source_extension_artifact_object_closure
from molt.cli.source_extension_object_closure_schema import (
    SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY,
    SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
)
from molt.cli.source_extension_reproducibility import _require_location_neutral
from molt.cli.source_extension_set_identity import (
    ValidatedSourceExtensionIdentitySidecar,
    _extension_content_projection,
    validate_source_extension_execution_metadata,
)
from molt.cli.source_extension_set_registry import SourceExtensionVariant
from molt.cli.source_extension_set_validation_schema import (
    RecordedSourceExtensionSet,
    SourceExtensionSetValidationError,
)
from molt.cli.source_extension_set_validation_target import (
    ValidatedSourceExtensionTarget,
)
from molt.cli.source_extension_target import source_extension_artifact_suffix
from molt.cli.source_extension_language import require_source_extension_language
from molt.exact_json import canonical_json_bytes, loads_exact
from molt.file_hashing import _sha256_bytes, _sha256_file
from molt.toolchain_identity import (
    stable_regular_file_identity,
    verify_stable_regular_file_identity,
)


def validate_source_extension_sidecars(
    *,
    publish_root: Path,
    extension_set: RecordedSourceExtensionSet,
    variant: SourceExtensionVariant,
    set_manifest: Mapping[str, Any],
    target: ValidatedSourceExtensionTarget,
    inventory_sha256: Mapping[str, str],
) -> tuple[ValidatedSourceExtensionIdentitySidecar, ...]:
    target_plan = target.plan
    target_commands = {role: list(command) for role, command in target.commands}
    raw_extensions = set_manifest["extensions"]
    target_triple = set_manifest.get("target_triple")
    if not isinstance(target_triple, str) or not target_triple:
        raise SourceExtensionSetValidationError(
            "extension-set manifest has no target-triple authority"
        )
    artifact_suffix = source_extension_artifact_suffix(target_triple)
    expected_artifacts = {
        publish_root.joinpath(
            *spec.module.split(".")[:-1],
            f"{spec.target}{artifact_suffix}",
        ).resolve()
        for spec in extension_set.extensions
    }
    expected_sidecars = {
        artifact.with_name(f"{artifact.name}.extension_manifest.json")
        for artifact in expected_artifacts
    }
    actual_artifacts = {
        path.resolve()
        for suffix in (".molt.a", ".molt.wasm")
        for path in publish_root.glob(f"**/*{suffix}")
        if path.is_file()
    }
    actual_sidecars = {
        path.resolve()
        for suffix in (".molt.a", ".molt.wasm")
        for path in publish_root.glob(f"**/*{suffix}.extension_manifest.json")
        if path.is_file()
    }
    if actual_artifacts != expected_artifacts:
        missing = sorted(str(path) for path in expected_artifacts - actual_artifacts)
        unexpected = sorted(str(path) for path in actual_artifacts - expected_artifacts)
        raise SourceExtensionSetValidationError(
            "published extension artifacts differ from configured complete set; "
            f"missing={missing}, unexpected={unexpected}"
        )
    if actual_sidecars != expected_sidecars:
        missing = sorted(str(path) for path in expected_sidecars - actual_sidecars)
        unexpected = sorted(str(path) for path in actual_sidecars - expected_sidecars)
        raise SourceExtensionSetValidationError(
            "published extension sidecars differ from configured complete set; "
            f"missing={missing}, unexpected={unexpected}"
        )
    entries_by_module = {
        str(item["module"]): item
        for item in raw_extensions
        if isinstance(item, Mapping)
    }
    validated_sidecars: list[ValidatedSourceExtensionIdentitySidecar] = []
    for spec in extension_set.extensions:
        sidecar_path = publish_root.joinpath(
            *spec.module.split(".")[:-1],
            f"{spec.target}{artifact_suffix}.extension_manifest.json",
        ).resolve()
        try:
            sidecar_bytes = sidecar_path.read_bytes()
            sidecar = loads_exact(sidecar_bytes.decode("utf-8"))
        except (OSError, UnicodeError, ValueError) as exc:
            raise SourceExtensionSetValidationError(
                f"failed to read published extension sidecar {sidecar_path}: {exc}"
            ) from exc
        if not isinstance(sidecar, Mapping) or sidecar.get("module") != spec.module:
            raise SourceExtensionSetValidationError(
                f"published extension sidecar has wrong module: {sidecar_path}"
            )
        expected_sidecar_contract = {
            "name": extension_set.package,
            "version": extension_set.package_version,
            "module": spec.module,
            "abi_tier": variant.abi_tier,
            "target_python": variant.target_python.tag,
            "python_tag": f"py{variant.target_python.major}",
            "target_triple": variant.target_triple,
            "artifact_kind": target_plan.artifact_kind,
            "python_exports": list(spec.python_exports),
            "capabilities": list(spec.capabilities),
            "provided_capsules": list(spec.provided_capsules),
        }
        sidecar_mismatches = [
            f"{field}: expected {expected!r}, got {sidecar.get(field)!r}"
            for field, expected in expected_sidecar_contract.items()
            if sidecar.get(field) != expected
        ]
        source_plan = sidecar.get("source_plan")
        if (
            not isinstance(source_plan, Mapping)
            or source_plan.get("target_selector") != spec.target
        ):
            sidecar_mismatches.append("source_plan.target_selector differs from set")
        if sidecar_mismatches:
            raise SourceExtensionSetValidationError(
                "extension sidecar differs from set variant contract: "
                + "; ".join(sidecar_mismatches)
            )
        if sidecar.get("deterministic") is not True:
            raise SourceExtensionSetValidationError(
                "extension sidecar requires deterministic=true"
            )
        raw_wheel = sidecar.get("wheel")
        if (
            not isinstance(raw_wheel, str)
            or not raw_wheel
            or Path(raw_wheel).is_absolute()
            or "\\" in raw_wheel
        ):
            raise SourceExtensionSetValidationError(
                f"extension sidecar wheel path is invalid for {spec.module}"
            )
        wheel_path = (sidecar_path.parent / raw_wheel).resolve()
        if (
            not wheel_path.is_relative_to(publish_root.resolve())
            or not wheel_path.is_file()
            or sidecar.get("wheel_sha256") != _sha256_file(wheel_path)
        ):
            raise SourceExtensionSetValidationError(
                f"extension sidecar wheel is not sealed or checksummed for {spec.module}"
            )
        try:
            _require_location_neutral(
                sidecar,
                authority=f"published extension sidecar {sidecar_path}",
            )
        except ValueError as exc:
            raise SourceExtensionSetValidationError(str(exc)) from exc
        entry = entries_by_module[spec.module]
        closure = sidecar.get("object_closure")
        closure_sha256 = (
            closure.get("closure_sha256") if isinstance(closure, Mapping) else None
        )
        try:
            if not isinstance(closure, Mapping):
                raise SourceExtensionObjectClosureError(
                    "extension sidecar has no object closure"
                )
            validated_object_closure = validate_source_extension_object_closure(sidecar)
            validate_source_extension_object_closure_sources(
                closure,
                manifest_dir=sidecar_path.parent,
                manifest=sidecar,
                allowed_root=publish_root,
            )
        except SourceExtensionObjectClosureError as exc:
            raise SourceExtensionSetValidationError(
                f"extension sidecar object closure is invalid for {spec.module}: {exc}"
            ) from exc
        checksums = {
            "artifact_sha256": sidecar.get("extension_sha256"),
            "wheel_sha256": sidecar.get("wheel_sha256"),
            "object_closure_sha256": closure_sha256,
        }
        for field_name, sidecar_value in checksums.items():
            if entry.get(field_name) != sidecar_value:
                raise SourceExtensionSetValidationError(
                    f"extension-set manifest {field_name} differs from sidecar for "
                    f"{spec.module}"
                )
        closure_objects = (
            closure.get("objects") if isinstance(closure, Mapping) else None
        )
        if not isinstance(closure_objects, list):
            raise SourceExtensionSetValidationError(
                f"extension sidecar object closure is invalid for {spec.module}"
            )
        for object_index, closure_object in enumerate(closure_objects):
            if not isinstance(closure_object, Mapping):
                raise SourceExtensionSetValidationError(
                    f"extension sidecar object[{object_index}] is invalid"
                )
            closure_object = cast(Mapping[str, Any], closure_object)
            try:
                language = require_source_extension_language(
                    closure_object.get("language")
                )
                compile_command = _manifest_sequence(
                    sidecar, closure_object, "compile_command"
                )
                symbol_command = _manifest_sequence(
                    sidecar, closure_object, "symbol_command"
                )
            except ValueError as exc:
                raise SourceExtensionSetValidationError(str(exc)) from exc
            expected_compiler = target_commands.get(language.compiler_role)
            if not expected_compiler:
                raise SourceExtensionSetValidationError(
                    f"extension object language {language.value} has no canonical compiler"
                )
            expected_nm = target_commands["nm"]
            symbol_authority = closure_object.get("symbol_authority")
            symbol_custody_matches = (
                symbol_authority == SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY
                and symbol_command is None
                if target_plan.artifact_kind == "wasm_relocatable_object"
                else symbol_authority == SOURCE_EXTENSION_NATIVE_SYMBOL_AUTHORITY
                and symbol_command == expected_nm
            )
            if not (
                isinstance(compile_command, list)
                and compile_command[: len(expected_compiler)] == expected_compiler
                and symbol_custody_matches
            ):
                raise SourceExtensionSetValidationError(
                    f"extension sidecar object[{object_index}] for {spec.module} "
                    "did not consume the canonical compiler/symbol authority"
                )
            if closure_object.get("unit_sha256") != _object_unit_sha256(
                sidecar, closure_object
            ):
                raise SourceExtensionSetValidationError(
                    f"extension sidecar object[{object_index}] for {spec.module} "
                    "has false content-addressed unit identity"
                )
        artifact_path = Path(str(sidecar_path).removesuffix(".extension_manifest.json"))
        artifact_identity = stable_regular_file_identity(
            artifact_path, label="extension-set artifact"
        )
        artifact_sha256 = artifact_identity.sha256
        if artifact_sha256 != entry.get("artifact_sha256"):
            raise SourceExtensionSetValidationError(
                f"extension-set manifest artifact checksum differs from bytes for "
                f"{spec.module}"
            )
        execution_metadata = validate_source_extension_execution_metadata(
            sidecar, inventory_sha256=inventory_sha256
        )
        artifact_closure_errors = validate_source_extension_artifact_object_closure(
            artifact_path=artifact_path,
            manifest=sidecar,
            validated_closure=validated_object_closure,
            required_function_exports=execution_metadata.direct_function_exports,
        )
        verify_stable_regular_file_identity(
            artifact_identity, label="extension-set artifact"
        )
        if artifact_closure_errors:
            raise SourceExtensionSetValidationError(
                f"extension sidecar artifact/object closure mismatch for {spec.module}: "
                + "; ".join(artifact_closure_errors)
            )

        content = _extension_content_projection(
            sidecar,
            validated_closure=validated_object_closure,
            execution_metadata=execution_metadata,
        )
        validated_sidecars.append(
            ValidatedSourceExtensionIdentitySidecar(
                module=spec.module,
                target=spec.target,
                artifact_relative_path=artifact_path.relative_to(
                    publish_root
                ).as_posix(),
                sidecar_relative_path=sidecar_path.relative_to(publish_root).as_posix(),
                artifact_sha256=artifact_sha256,
                sidecar_sha256=_sha256_bytes(sidecar_bytes),
                manifest_json=canonical_json_bytes(sidecar),
                content_json=canonical_json_bytes(content),
            )
        )
    return tuple(validated_sidecars)
