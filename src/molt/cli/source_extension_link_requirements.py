from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
from typing import Any, Mapping, Sequence


_FORBIDDEN_OUTPUT_ARGUMENTS = frozenset({"-o", "--output", "/out"})
_FORBIDDEN_LINK_MODES = frozenset({"-shared", "--shared", "/dll"})
_STATIC_INPUT_SUFFIXES = frozenset({".a", ".lib", ".o", ".obj", ".molt.wasm"})


def _argument_basename(argument: str) -> str:
    return Path(argument.replace("\\", "/")).name


def _is_static_input_argument(argument: str) -> bool:
    lowered = argument.lower()
    return any(lowered.endswith(suffix) for suffix in _STATIC_INPUT_SUFFIXES)


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkRequirements:
    target_triple: str
    arguments: tuple[str, ...]

    def manifest_payload(self) -> dict[str, object]:
        return {
            "target_triple": self.target_triple,
            "arguments": list(self.arguments),
        }


def source_extension_link_requirements(
    link_args: Sequence[str],
    *,
    target_triple: str,
    folded_static_archives: Sequence[str] = (),
) -> SourceExtensionLinkRequirements:
    folded = {name.lower() for name in folded_static_archives}
    arguments = tuple(
        argument
        for raw_argument in link_args
        if (argument := str(raw_argument).strip())
        and _argument_basename(argument).lower() not in folded
    )
    lowered = tuple(argument.lower() for argument in arguments)
    for index, argument in enumerate(lowered):
        option = argument.split(":", 1)[0]
        if option in _FORBIDDEN_OUTPUT_ARGUMENTS:
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
    return SourceExtensionLinkRequirements(
        target_triple=target_triple,
        arguments=arguments,
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
            source_extension_link_requirements(
                arguments,
                target_triple=normalized_target or expected_target_triple,
            )
        except ValueError as exc:
            errors.append(str(exc))
    if errors:
        return None, errors
    return SourceExtensionLinkRequirements(normalized_target, arguments), []


def resolve_source_extension_link_arguments(
    requirements: SourceExtensionLinkRequirements,
    *,
    package_root: Path,
    manifest_dir: Path,
) -> tuple[tuple[str, ...] | None, list[str]]:
    resolved: list[str] = []
    errors: list[str] = []
    package_root = package_root.resolve()
    for argument in requirements.arguments:
        prefix = ""
        raw_path = argument
        if argument.startswith("-L") and len(argument) > 2:
            prefix = "-L"
            raw_path = argument[2:]
        elif not _is_static_input_argument(argument):
            resolved.append(argument)
            continue
        path = Path(raw_path).expanduser()
        candidates = (
            (path,)
            if path.is_absolute()
            else (manifest_dir / path, package_root / path)
        )
        selected = next((candidate.resolve() for candidate in candidates if candidate.exists()), None)
        if selected is None:
            errors.append(f"link requirement path does not exist: {raw_path}")
            continue
        if not (selected == package_root or selected.is_relative_to(package_root)):
            errors.append(
                "link requirement path escapes the sealed package root: "
                f"{selected}"
            )
            continue
        resolved.append(prefix + str(selected))
    if errors:
        return None, errors
    return tuple(resolved), []
