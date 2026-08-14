from __future__ import annotations

from dataclasses import dataclass
import hashlib
import os
from pathlib import Path
import shutil
import tempfile
from typing import Any, Mapping, Sequence


_FORBIDDEN_OUTPUT_ARGUMENTS = frozenset({"-o", "--output", "/out"})
_FORBIDDEN_LINK_MODES = frozenset({"-shared", "--shared", "/dll"})
_STATIC_INPUT_SUFFIXES = frozenset({".a", ".lib", ".o", ".obj", ".molt.wasm"})
_SEALED_PATH_PREFIXES = (
    "-Wl,-force_load,",
    "-Wl,/WHOLEARCHIVE:",
    "/WHOLEARCHIVE:",
)


def _argument_basename(argument: str) -> str:
    return Path(argument.replace("\\", "/")).name


def _is_static_input_path(path: str) -> bool:
    lowered = path.lower()
    return any(lowered.endswith(suffix) for suffix in _STATIC_INPUT_SUFFIXES)


def _sealed_path_argument(argument: str) -> tuple[str, str] | None:
    if argument.upper().startswith("/DEFAULTLIB:"):
        return None
    for prefix in _SEALED_PATH_PREFIXES:
        if argument.startswith(prefix):
            path = argument[len(prefix) :]
            return (prefix, path) if _is_static_input_path(path) else None
    return ("", argument) if _is_static_input_path(argument) else None


def _is_bare_library_name(path: str) -> bool:
    value = Path(path)
    return (
        not value.is_absolute()
        and value.name == path
        and "/" not in path
        and "\\" not in path
        and value.suffix.lower() in {".a", ".lib"}
    )


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _atomic_copy_file(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        dir=destination.parent,
        prefix=f".{destination.name}.",
        suffix=".tmp",
        delete=False,
    ) as stream:
        temporary = Path(stream.name)
    try:
        shutil.copyfile(source, temporary)
        os.replace(temporary, destination)
    finally:
        temporary.unlink(missing_ok=True)


def _validate_argument_modes(arguments: Sequence[str]) -> None:
    lowered = tuple(argument.lower() for argument in arguments)
    for index, (raw_argument, argument) in enumerate(
        zip(arguments, lowered, strict=True)
    ):
        option = argument.split(":", 1)[0]
        if option in _FORBIDDEN_OUTPUT_ARGUMENTS or any(
            argument.startswith(f"{item}=") for item in _FORBIDDEN_OUTPUT_ARGUMENTS
        ):
            raise ValueError(
                "source-extension final link requirements cannot select an output path"
            )
        if argument in _FORBIDDEN_LINK_MODES:
            raise ValueError(
                "source-extension final link requirements cannot select shared linkage"
            )
        if index and lowered[index - 1] in _FORBIDDEN_OUTPUT_ARGUMENTS:
            raise ValueError(
                "source-extension final link requirements cannot contain an output operand"
            )
        if argument.startswith("@"):
            raise ValueError(
                "source-extension final link requirements cannot contain response files"
            )
        if (
            raw_argument.startswith("-L")
            or raw_argument.startswith("-Wl,-L")
            or raw_argument.upper().startswith("/LIBPATH:")
        ):
            raise ValueError(
                "source-extension final link requirements cannot contain library search "
                "paths; publish exact checksummed static inputs instead"
            )
        if (
            raw_argument == "-T"
            or raw_argument.startswith("-T")
            or raw_argument.startswith("--script=")
            or raw_argument.startswith("-Wl,-T,")
            or raw_argument.startswith("-Wl,--script=")
            or raw_argument.startswith("-Wl,--version-script=")
            or raw_argument.startswith("-Wl,-exported_symbols_list,")
            or raw_argument.upper().startswith("/DEF:")
        ):
            raise ValueError(
                "source-extension final link requirements cannot contain unsealed linker "
                "script/export paths"
            )


def _resolve_source_path(path: str, roots: Sequence[Path]) -> Path | None:
    candidate = Path(path).expanduser()
    candidates = (
        (candidate,)
        if candidate.is_absolute()
        else tuple(root / candidate for root in roots)
    )
    return next(
        (
            resolved
            for item in candidates
            if item.is_file()
            for resolved in (item.resolve(),)
        ),
        None,
    )


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkInput:
    argument_index: int
    path: str
    sha256: str
    prefix: str = ""

    def manifest_payload(self) -> dict[str, object]:
        return {
            "argument_index": self.argument_index,
            "path": self.path,
            "sha256": self.sha256,
            "prefix": self.prefix,
        }


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkRequirements:
    target_triple: str
    arguments: tuple[str, ...]
    inputs: tuple[SourceExtensionLinkInput, ...] = ()

    def manifest_payload(self) -> dict[str, object]:
        return {
            "target_triple": self.target_triple,
            "arguments": list(self.arguments),
            "inputs": [item.manifest_payload() for item in self.inputs],
        }


def source_extension_link_requirements(
    link_args: Sequence[str],
    *,
    target_triple: str,
    folded_static_archives: Sequence[str] = (),
    path_roots: Sequence[Path] = (),
    publish_root: Path | None = None,
) -> SourceExtensionLinkRequirements:
    folded = {name.lower() for name in folded_static_archives}
    arguments: list[str] = []
    inputs: list[SourceExtensionLinkInput] = []
    for raw_argument in link_args:
        argument = str(raw_argument).strip()
        if not argument:
            continue
        sealed_path = _sealed_path_argument(argument)
        if sealed_path is not None:
            prefix, raw_path = sealed_path
            if _argument_basename(raw_path).lower() in folded:
                continue
            source = _resolve_source_path(raw_path, path_roots)
            if source is None:
                if prefix == "" and _is_bare_library_name(raw_path):
                    arguments.append(argument)
                    continue
                raise ValueError(
                    f"source-extension final link input does not exist: {raw_path}"
                )
            digest = _sha256_file(source)
            if publish_root is None:
                manifest_path = str(source)
            else:
                destination = publish_root / "__molt_link__" / digest / source.name
                _atomic_copy_file(source, destination)
                manifest_path = destination.relative_to(publish_root).as_posix()
            argument_index = len(arguments)
            arguments.append(prefix + manifest_path)
            inputs.append(
                SourceExtensionLinkInput(
                    argument_index=argument_index,
                    path=manifest_path,
                    sha256=digest,
                    prefix=prefix,
                )
            )
            continue
        arguments.append(argument)
    _validate_argument_modes(arguments)
    return SourceExtensionLinkRequirements(
        target_triple=target_triple,
        arguments=tuple(arguments),
        inputs=tuple(inputs),
    )


def parse_source_extension_link_requirements(
    manifest: Mapping[str, Any],
    *,
    expected_target_triple: str,
) -> tuple[SourceExtensionLinkRequirements | None, list[str]]:
    raw = manifest.get("link_requirements")
    if raw is None:
        return SourceExtensionLinkRequirements(expected_target_triple, ()), []
    if not isinstance(raw, Mapping):
        return None, ["link_requirements must be an object"]
    errors: list[str] = []
    target = raw.get("target_triple")
    if not isinstance(target, str) or not target.strip():
        errors.append("link_requirements.target_triple must be a non-empty string")
        normalized_target = ""
    else:
        normalized_target = target.strip().lower()
        if normalized_target != expected_target_triple.lower():
            errors.append(
                "link_requirements.target_triple must match target_triple "
                f"({normalized_target!r} != {expected_target_triple.lower()!r})"
            )
    raw_arguments = raw.get("arguments")
    if not isinstance(raw_arguments, list) or not all(
        isinstance(argument, str) and argument.strip() for argument in raw_arguments
    ):
        errors.append("link_requirements.arguments must be a list of non-empty strings")
        arguments: tuple[str, ...] = ()
    else:
        arguments = tuple(argument.strip() for argument in raw_arguments)
        try:
            _validate_argument_modes(arguments)
        except ValueError as exc:
            errors.append(str(exc))
    raw_inputs = raw.get("inputs", [])
    inputs: list[SourceExtensionLinkInput] = []
    if not isinstance(raw_inputs, list):
        errors.append("link_requirements.inputs must be a list")
        raw_inputs = []
    for index, item in enumerate(raw_inputs):
        if not isinstance(item, Mapping):
            errors.append(f"link_requirements.inputs[{index}] must be an object")
            continue
        argument_index = item.get("argument_index")
        path = item.get("path")
        sha256 = item.get("sha256")
        prefix = item.get("prefix", "")
        if not isinstance(argument_index, int) or isinstance(argument_index, bool):
            errors.append(
                f"link_requirements.inputs[{index}].argument_index must be an integer"
            )
            continue
        if not isinstance(path, str) or not path.strip():
            errors.append(
                f"link_requirements.inputs[{index}].path must be a non-empty string"
            )
            continue
        normalized_path = path.strip().replace("\\", "/")
        path_value = Path(normalized_path)
        if path_value.is_absolute() or ".." in path_value.parts:
            errors.append(
                f"link_requirements.inputs[{index}].path must be package-relative"
            )
            continue
        if not _is_static_input_path(normalized_path):
            errors.append(
                f"link_requirements.inputs[{index}].path must name a static input"
            )
            continue
        if (
            not isinstance(sha256, str)
            or len(sha256) != 64
            or any(character not in "0123456789abcdefABCDEF" for character in sha256)
        ):
            errors.append(
                f"link_requirements.inputs[{index}].sha256 must be a SHA-256 digest"
            )
            continue
        if not isinstance(prefix, str) or prefix not in ("", *_SEALED_PATH_PREFIXES):
            errors.append(f"link_requirements.inputs[{index}].prefix is invalid")
            continue
        inputs.append(
            SourceExtensionLinkInput(
                argument_index=argument_index,
                path=normalized_path,
                sha256=sha256.lower(),
                prefix=prefix,
            )
        )
    input_indexes = [item.argument_index for item in inputs]
    if input_indexes != sorted(set(input_indexes)):
        errors.append(
            "link_requirements input argument indexes must be unique and sorted"
        )
    declared_indexes = set(input_indexes)
    for item in inputs:
        if not 0 <= item.argument_index < len(arguments):
            errors.append(
                "link_requirements input argument index is outside arguments: "
                f"{item.argument_index}"
            )
        elif arguments[item.argument_index] != item.prefix + item.path:
            errors.append(
                "link_requirements input does not own its argument value at index "
                f"{item.argument_index}"
            )
    for index, argument in enumerate(arguments):
        sealed_path = _sealed_path_argument(argument)
        if (
            sealed_path is not None
            and not (sealed_path[0] == "" and _is_bare_library_name(sealed_path[1]))
            and index not in declared_indexes
        ):
            errors.append(
                "link_requirements contains an unchecksummed static path operand at "
                f"arguments[{index}]"
            )
    if errors:
        return None, errors
    return (
        SourceExtensionLinkRequirements(
            normalized_target,
            arguments,
            tuple(inputs),
        ),
        [],
    )


def _resolved_link_input(
    item: SourceExtensionLinkInput,
    *,
    package_root: Path,
    manifest_dir: Path,
) -> tuple[Path | None, str | None]:
    package_root = package_root.resolve()
    relative = Path(item.path)
    candidates = (manifest_dir / relative, package_root / relative)
    selected = next(
        (candidate.resolve() for candidate in candidates if candidate.is_file()),
        None,
    )
    if selected is None:
        return None, f"link requirement path does not exist: {item.path}"
    if not (selected == package_root or selected.is_relative_to(package_root)):
        return (
            None,
            "link requirement path escapes the sealed package root: " + str(selected),
        )
    actual = _sha256_file(selected)
    if actual.lower() != item.sha256.lower():
        return (
            None,
            f"link requirement checksum mismatch for {item.path}: "
            f"expected {item.sha256}, got {actual}",
        )
    return selected, None


def resolve_source_extension_link_arguments(
    requirements: SourceExtensionLinkRequirements,
    *,
    package_root: Path,
    manifest_dir: Path,
) -> tuple[tuple[str, ...] | None, list[str]]:
    resolved = list(requirements.arguments)
    errors: list[str] = []
    for item in requirements.inputs:
        selected, error = _resolved_link_input(
            item,
            package_root=package_root,
            manifest_dir=manifest_dir,
        )
        if error is not None:
            errors.append(error)
            continue
        assert selected is not None
        resolved[item.argument_index] = item.prefix + str(selected)
    if errors:
        return None, errors
    return tuple(resolved), []


def materialize_source_extension_link_requirements(
    requirements: SourceExtensionLinkRequirements,
    *,
    package_root: Path,
    manifest_dir: Path,
    publish_root: Path,
) -> tuple[SourceExtensionLinkRequirements | None, list[str]]:
    arguments = list(requirements.arguments)
    published_inputs: list[SourceExtensionLinkInput] = []
    errors: list[str] = []
    for item in requirements.inputs:
        source, error = _resolved_link_input(
            item,
            package_root=package_root,
            manifest_dir=manifest_dir,
        )
        if error is not None:
            errors.append(error)
            continue
        assert source is not None
        destination = publish_root / "__molt_link__" / item.sha256 / source.name
        _atomic_copy_file(source, destination)
        relative = destination.relative_to(publish_root).as_posix()
        arguments[item.argument_index] = item.prefix + relative
        published_inputs.append(
            SourceExtensionLinkInput(
                argument_index=item.argument_index,
                path=relative,
                sha256=item.sha256,
                prefix=item.prefix,
            )
        )
    if errors:
        return None, errors
    return (
        SourceExtensionLinkRequirements(
            target_triple=requirements.target_triple,
            arguments=tuple(arguments),
            inputs=tuple(published_inputs),
        ),
        [],
    )
