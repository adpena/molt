from __future__ import annotations

from dataclasses import dataclass
from enum import Enum
import hashlib
import os
from pathlib import Path
import re
import shutil
import tempfile
from typing import Any, Callable, Mapping, Sequence

from molt.cli.source_extension_target import (
    SourceExtensionLinkDialect,
    source_extension_link_dialect,
)

_STATIC_INPUT_SUFFIXES = frozenset({".a", ".lib", ".o", ".obj", ".molt.wasm"})
_BARE_LIBRARY_SUFFIXES = {
    SourceExtensionLinkDialect.ELF_GNU: frozenset({".a"}),
    SourceExtensionLinkDialect.MACHO: frozenset({".a"}),
    SourceExtensionLinkDialect.COFF_GNU: frozenset({".a"}),
    SourceExtensionLinkDialect.COFF_MSVC: frozenset({".lib"}),
    SourceExtensionLinkDialect.WASM: frozenset({".a"}),
}
_GROUP_DIALECTS = frozenset(
    {
        SourceExtensionLinkDialect.ELF_GNU,
        SourceExtensionLinkDialect.COFF_GNU,
        SourceExtensionLinkDialect.WASM,
    }
)
_WHOLE_ARCHIVE_DIALECTS = frozenset(SourceExtensionLinkDialect)
_GNU_LIBRARY_DIALECTS = frozenset(
    {
        SourceExtensionLinkDialect.ELF_GNU,
        SourceExtensionLinkDialect.MACHO,
        SourceExtensionLinkDialect.COFF_GNU,
        SourceExtensionLinkDialect.WASM,
    }
)
_LINK_SYMBOL = re.compile(r"[A-Za-z_.$?@][A-Za-z0-9_.$?@-]*")
_BARE_PROVIDER_NAME = re.compile(r"[A-Za-z0-9_+.@-]+")


class SourceExtensionLinkProviderKind(str, Enum):
    LIBRARY = "library"
    ARCHIVE = "archive"
    FRAMEWORK = "framework"
    THREAD_RUNTIME = "thread-runtime"


class SourceExtensionLinkLoadingPolicy(str, Enum):
    DEFAULT = "default"
    AS_NEEDED = "as-needed"
    ALL_MEMBERS = "all-members"


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkProvider:
    provider_kind: SourceExtensionLinkProviderKind
    name: str
    loading: SourceExtensionLinkLoadingPolicy = SourceExtensionLinkLoadingPolicy.DEFAULT

    def manifest_payload(self) -> dict[str, object]:
        return {
            "kind": "provider",
            "provider_kind": self.provider_kind.value,
            "name": self.name,
            "loading": self.loading.value,
        }


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkInput:
    path: str
    sha256: str
    loading: SourceExtensionLinkLoadingPolicy = SourceExtensionLinkLoadingPolicy.DEFAULT

    def manifest_payload(self) -> dict[str, object]:
        return {
            "kind": "input",
            "path": self.path,
            "sha256": self.sha256,
            "loading": self.loading.value,
        }


SourceExtensionLinkAtom = SourceExtensionLinkProvider | SourceExtensionLinkInput


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkCyclicGroup:
    members: tuple[SourceExtensionLinkAtom, ...]

    def manifest_payload(self) -> dict[str, object]:
        return {
            "kind": "cyclic-group",
            "members": [member.manifest_payload() for member in self.members],
        }


SourceExtensionLinkItem = SourceExtensionLinkAtom | SourceExtensionLinkCyclicGroup


@dataclass(frozen=True, slots=True)
class SourceExtensionLinkRequirements:
    target_triple: str
    items: tuple[SourceExtensionLinkItem, ...] = ()
    retained_symbols: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        if not self.target_triple or self.target_triple != self.target_triple.lower():
            raise ValueError("source-extension link target must be canonical lowercase")
        dialect = source_extension_link_dialect(self.target_triple)
        if self.retained_symbols != tuple(sorted(set(self.retained_symbols))) or any(
            not _is_link_symbol(symbol) for symbol in self.retained_symbols
        ):
            raise ValueError(
                "source-extension retained symbols must be canonical, unique, and sorted"
            )
        for item in self.items:
            _validate_item(item, dialect=dialect, package_relative=False)

    @property
    def inputs(self) -> tuple[SourceExtensionLinkInput, ...]:
        result: list[SourceExtensionLinkInput] = []
        for item in self.items:
            members = (
                item.members
                if isinstance(item, SourceExtensionLinkCyclicGroup)
                else (item,)
            )
            result.extend(
                member
                for member in members
                if isinstance(member, SourceExtensionLinkInput)
            )
        return tuple(result)

    def manifest_payload(self) -> dict[str, object]:
        return {
            "target_triple": self.target_triple,
            "items": [item.manifest_payload() for item in self.items],
            "retained_symbols": list(self.retained_symbols),
        }


def _argument_basename(argument: str) -> str:
    return Path(argument.replace("\\", "/")).name


def _is_static_input_path(path: str) -> bool:
    lowered = path.lower()
    return any(lowered.endswith(suffix) for suffix in _STATIC_INPUT_SUFFIXES)


def _is_bare_provider_name(value: str) -> bool:
    path = Path(value)
    return (
        bool(value)
        and not value.startswith("-")
        and not path.is_absolute()
        and path.name == value
        and "/" not in value
        and "\\" not in value
        and value not in {".", ".."}
        and _BARE_PROVIDER_NAME.fullmatch(value) is not None
    )


def _is_bare_library_name(path: str, *, dialect: SourceExtensionLinkDialect) -> bool:
    return (
        _is_bare_provider_name(path)
        and Path(path).suffix.lower() in _BARE_LIBRARY_SUFFIXES[dialect]
    )


def _is_link_symbol(value: str) -> bool:
    return _LINK_SYMBOL.fullmatch(value) is not None


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


def _canonical_link_arguments(
    link_args: Sequence[str], *, dialect: SourceExtensionLinkDialect
) -> tuple[str, ...]:
    raw = tuple(str(argument).strip() for argument in link_args)
    canonical: list[str] = []
    index = 0
    while index < len(raw):
        argument = raw[index]
        if not argument:
            index += 1
            continue
        if dialect is SourceExtensionLinkDialect.MACHO and argument == "-framework":
            if index + 1 >= len(raw) or not _is_bare_provider_name(raw[index + 1]):
                raise ValueError(
                    "source-extension Mach-O -framework requires a bare name"
                )
            canonical.append(f"-Wl,-framework,{raw[index + 1]}")
            index += 2
            continue
        if (
            dialect is SourceExtensionLinkDialect.COFF_MSVC
            and argument.upper().startswith("/DEFAULTLIB:")
        ):
            provider = argument.split(":", 1)[1]
            if not _is_bare_library_name(provider, dialect=dialect):
                raise ValueError(
                    "source-extension COFF-MSVC /DEFAULTLIB requires a bare .lib name"
                )
            canonical.append(provider)
            index += 1
            continue
        canonical.append(argument)
        index += 1
    return tuple(canonical)


def _resolve_source_path(path: str, roots: Sequence[Path]) -> Path | None:
    resolved_roots = tuple(root.resolve() for root in roots)
    candidate = Path(path).expanduser()
    candidates = (
        (candidate,)
        if candidate.is_absolute()
        else tuple(root / candidate for root in roots)
    )
    for item in candidates:
        if not item.is_file():
            continue
        resolved = item.resolve()
        if not any(
            resolved == root or resolved.is_relative_to(root) for root in resolved_roots
        ):
            raise ValueError(
                "source-extension final link input escapes declared source roots: "
                + str(resolved)
            )
        return resolved
    return None


def _forced_input_operand(
    argument: str, *, dialect: SourceExtensionLinkDialect
) -> str | None:
    prefixes = {
        SourceExtensionLinkDialect.MACHO: ("-Wl,-force_load,",),
        SourceExtensionLinkDialect.COFF_MSVC: (
            "-Wl,/WHOLEARCHIVE:",
            "/WHOLEARCHIVE:",
        ),
    }.get(dialect, ())
    for prefix in prefixes:
        matches = (
            argument[: len(prefix)].casefold() == prefix.casefold()
            if dialect is SourceExtensionLinkDialect.COFF_MSVC
            else argument.startswith(prefix)
        )
        if matches:
            path = argument[len(prefix) :]
            return path if _is_static_input_path(path) else None
    return None


def _retained_symbol(
    argument: str, *, dialect: SourceExtensionLinkDialect
) -> str | None:
    prefixes = {
        SourceExtensionLinkDialect.ELF_GNU: ("-Wl,--undefined=", "-Wl,-u,"),
        SourceExtensionLinkDialect.MACHO: ("-Wl,-u,",),
        SourceExtensionLinkDialect.COFF_GNU: ("-Wl,--undefined=", "-Wl,-u,"),
        SourceExtensionLinkDialect.COFF_MSVC: ("-Wl,/INCLUDE:", "/INCLUDE:"),
        SourceExtensionLinkDialect.WASM: ("--undefined=", "-Wl,--undefined="),
    }[dialect]
    for prefix in prefixes:
        matches = (
            argument[: len(prefix)].casefold() == prefix.casefold()
            if dialect is SourceExtensionLinkDialect.COFF_MSVC
            else argument.startswith(prefix)
        )
        if matches:
            symbol = argument[len(prefix) :]
            return symbol if _is_link_symbol(symbol) else None
    return None


def _provider_from_argument(
    argument: str,
    *,
    dialect: SourceExtensionLinkDialect,
    loading: SourceExtensionLinkLoadingPolicy,
) -> SourceExtensionLinkProvider | None:
    if argument == "-pthread" and dialect not in {
        SourceExtensionLinkDialect.COFF_MSVC,
        SourceExtensionLinkDialect.WASM,
    }:
        return SourceExtensionLinkProvider(
            SourceExtensionLinkProviderKind.THREAD_RUNTIME,
            "pthread",
            loading,
        )
    if (
        dialect in _GNU_LIBRARY_DIALECTS
        and argument.startswith("-l")
        and _is_bare_provider_name(argument[2:])
    ):
        return SourceExtensionLinkProvider(
            SourceExtensionLinkProviderKind.LIBRARY,
            argument[2:],
            loading,
        )
    framework_prefix = "-Wl,-framework,"
    if (
        dialect is SourceExtensionLinkDialect.MACHO
        and argument.startswith(framework_prefix)
        and _is_bare_provider_name(argument[len(framework_prefix) :])
    ):
        return SourceExtensionLinkProvider(
            SourceExtensionLinkProviderKind.FRAMEWORK,
            argument[len(framework_prefix) :],
            loading,
        )
    if _is_bare_library_name(argument, dialect=dialect):
        kind = (
            SourceExtensionLinkProviderKind.ARCHIVE
            if argument.lower().endswith(".a")
            else SourceExtensionLinkProviderKind.LIBRARY
        )
        return SourceExtensionLinkProvider(kind, argument, loading)
    return None


def _validate_provider(
    provider: SourceExtensionLinkProvider,
    *,
    dialect: SourceExtensionLinkDialect,
) -> None:
    if not _is_bare_provider_name(provider.name):
        raise ValueError(f"link provider name is not canonical: {provider.name!r}")
    if provider.provider_kind is SourceExtensionLinkProviderKind.FRAMEWORK:
        if dialect is not SourceExtensionLinkDialect.MACHO:
            raise ValueError("framework link providers require the Mach-O dialect")
    elif provider.provider_kind is SourceExtensionLinkProviderKind.THREAD_RUNTIME:
        if provider.name != "pthread" or dialect in {
            SourceExtensionLinkDialect.COFF_MSVC,
            SourceExtensionLinkDialect.WASM,
        }:
            raise ValueError("thread-runtime provider is unsupported for this target")
    elif provider.provider_kind is SourceExtensionLinkProviderKind.ARCHIVE:
        if not _is_bare_library_name(provider.name, dialect=dialect):
            raise ValueError("archive provider must be a target-dialect archive name")
    elif (
        dialect is SourceExtensionLinkDialect.COFF_MSVC
        and not provider.name.lower().endswith(".lib")
    ):
        raise ValueError("COFF-MSVC library provider must be a bare .lib name")
    if provider.loading is SourceExtensionLinkLoadingPolicy.AS_NEEDED:
        if not (
            dialect is SourceExtensionLinkDialect.ELF_GNU
            and provider.provider_kind is SourceExtensionLinkProviderKind.LIBRARY
        ):
            raise ValueError("as-needed loading requires an ELF library provider")
    if provider.loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS:
        if provider.provider_kind not in {
            SourceExtensionLinkProviderKind.ARCHIVE,
            SourceExtensionLinkProviderKind.LIBRARY,
        }:
            raise ValueError("all-members loading requires a library/archive provider")
        if dialect is SourceExtensionLinkDialect.MACHO:
            raise ValueError("Mach-O all-members loading requires a checksummed input")


def _validate_input(
    item: SourceExtensionLinkInput,
    *,
    dialect: SourceExtensionLinkDialect,
    package_relative: bool,
) -> None:
    if not _is_static_input_path(item.path):
        raise ValueError("link input path must name a static input")
    if package_relative:
        normalized = item.path.replace("\\", "/")
        path = Path(normalized)
        if normalized != item.path or path.is_absolute() or ".." in path.parts:
            raise ValueError("link input path must be canonical and package-relative")
    if (
        len(item.sha256) != 64
        or item.sha256 != item.sha256.lower()
        or any(character not in "0123456789abcdef" for character in item.sha256)
    ):
        raise ValueError("link input sha256 must be a lowercase SHA-256 digest")
    if item.loading is SourceExtensionLinkLoadingPolicy.AS_NEEDED:
        raise ValueError("checksummed link inputs do not support as-needed loading")
    if (
        item.loading is SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
        and dialect not in _WHOLE_ARCHIVE_DIALECTS
    ):
        raise ValueError("all-members loading is unsupported for this target")


def _validate_item(
    item: SourceExtensionLinkItem,
    *,
    dialect: SourceExtensionLinkDialect,
    package_relative: bool,
) -> None:
    if isinstance(item, SourceExtensionLinkProvider):
        _validate_provider(item, dialect=dialect)
        return
    if isinstance(item, SourceExtensionLinkInput):
        _validate_input(item, dialect=dialect, package_relative=package_relative)
        return
    if dialect not in _GROUP_DIALECTS:
        raise ValueError("cyclic archive groups are unsupported for this target")
    if not item.members:
        raise ValueError("cyclic archive groups must contain at least one member")
    for member in item.members:
        _validate_item(
            member,
            dialect=dialect,
            package_relative=package_relative,
        )


def _render_atom(
    item: SourceExtensionLinkAtom,
    *,
    dialect: SourceExtensionLinkDialect,
) -> tuple[str, ...]:
    if isinstance(item, SourceExtensionLinkInput):
        base = item.path
    elif item.provider_kind is SourceExtensionLinkProviderKind.FRAMEWORK:
        base = f"-Wl,-framework,{item.name}"
    elif item.provider_kind is SourceExtensionLinkProviderKind.THREAD_RUNTIME:
        base = "-pthread"
    elif item.provider_kind is SourceExtensionLinkProviderKind.LIBRARY:
        base = (
            item.name
            if dialect is SourceExtensionLinkDialect.COFF_MSVC
            or item.name.lower().endswith(".lib")
            else f"-l{item.name}"
        )
    else:
        base = item.name
    if item.loading is SourceExtensionLinkLoadingPolicy.DEFAULT:
        return (base,)
    if item.loading is SourceExtensionLinkLoadingPolicy.AS_NEEDED:
        return ("-Wl,--as-needed", base, "-Wl,--no-as-needed")
    if dialect is SourceExtensionLinkDialect.MACHO:
        return (f"-Wl,-force_load,{base}",)
    if dialect is SourceExtensionLinkDialect.COFF_MSVC:
        return (f"-Wl,/WHOLEARCHIVE:{base}",)
    return ("-Wl,--whole-archive", base, "-Wl,--no-whole-archive")


def render_source_extension_link_arguments(
    requirements: SourceExtensionLinkRequirements,
) -> tuple[str, ...]:
    """Render typed requirements for their attested target driver dialect."""

    dialect = source_extension_link_dialect(requirements.target_triple)
    arguments: list[str] = []
    retention_prefix = {
        SourceExtensionLinkDialect.ELF_GNU: "-Wl,--undefined=",
        SourceExtensionLinkDialect.MACHO: "-Wl,-u,",
        SourceExtensionLinkDialect.COFF_GNU: "-Wl,--undefined=",
        SourceExtensionLinkDialect.COFF_MSVC: "-Wl,/INCLUDE:",
        SourceExtensionLinkDialect.WASM: "--undefined=",
    }[dialect]
    arguments.extend(
        retention_prefix + symbol for symbol in requirements.retained_symbols
    )
    for item in requirements.items:
        if isinstance(item, SourceExtensionLinkCyclicGroup):
            arguments.append("-Wl,--start-group")
            for member in item.members:
                arguments.extend(_render_atom(member, dialect=dialect))
            arguments.append("-Wl,--end-group")
        else:
            arguments.extend(_render_atom(item, dialect=dialect))
    return tuple(arguments)


def source_extension_link_requirements(
    link_args: Sequence[str],
    *,
    target_triple: str,
    folded_static_archives: Sequence[str] = (),
    path_roots: Sequence[Path] = (),
    publish_root: Path | None = None,
) -> SourceExtensionLinkRequirements:
    dialect = source_extension_link_dialect(target_triple)
    folded = {name.casefold() for name in folded_static_archives}
    items: list[SourceExtensionLinkItem] = []
    group_members: list[SourceExtensionLinkAtom] | None = None
    retained_symbols: set[str] = set()
    whole_archive = False
    as_needed = False

    def append_item(item: SourceExtensionLinkAtom) -> None:
        if group_members is None:
            items.append(item)
        else:
            group_members.append(item)

    def make_input(
        raw_path: str,
        *,
        loading: SourceExtensionLinkLoadingPolicy,
        resolved_source: Path | None = None,
    ) -> SourceExtensionLinkInput | None:
        if _argument_basename(raw_path).casefold() in folded:
            return None
        source = resolved_source or _resolve_source_path(raw_path, path_roots)
        if source is None:
            raise ValueError(
                f"source-extension final link input does not exist: {raw_path}"
            )
        if publish_root is None:
            raise ValueError(
                "source-extension checksummed link inputs require a publication root"
            )
        digest = _sha256_file(source)
        destination = publish_root / "__molt_link__" / digest / source.name
        _atomic_copy_file(source, destination)
        return SourceExtensionLinkInput(
            path=destination.relative_to(publish_root).as_posix(),
            sha256=digest,
            loading=loading,
        )

    for argument in _canonical_link_arguments(link_args, dialect=dialect):
        symbol = _retained_symbol(argument, dialect=dialect)
        if symbol is not None:
            retained_symbols.add(symbol)
            continue
        forced_input = _forced_input_operand(argument, dialect=dialect)
        if forced_input is not None:
            item = make_input(
                forced_input,
                loading=SourceExtensionLinkLoadingPolicy.ALL_MEMBERS,
            )
            if item is not None:
                append_item(item)
            continue
        group_start = argument in {"-Wl,--start-group", "--start-group"}
        group_end = argument in {"-Wl,--end-group", "--end-group"}
        whole_start = argument in {"-Wl,--whole-archive", "--whole-archive"}
        whole_end = argument in {"-Wl,--no-whole-archive", "--no-whole-archive"}
        if group_start or group_end:
            if dialect not in _GROUP_DIALECTS:
                raise ValueError(
                    f"source-extension {dialect.value} does not support cyclic groups"
                )
            if whole_archive:
                raise ValueError(
                    "source-extension whole-archive scope must close before a group boundary"
                )
            if group_start:
                if group_members is not None:
                    raise ValueError("source-extension cyclic groups cannot be nested")
                group_members = []
            else:
                if group_members is None:
                    raise ValueError("source-extension cyclic group end has no start")
                if group_members:
                    items.append(SourceExtensionLinkCyclicGroup(tuple(group_members)))
                group_members = None
            continue
        if whole_start or whole_end:
            if dialect not in _GROUP_DIALECTS:
                raise ValueError(
                    f"source-extension {dialect.value} does not support GNU whole-archive scopes"
                )
            if whole_start:
                if whole_archive:
                    raise ValueError(
                        "source-extension whole-archive scopes cannot be nested"
                    )
                whole_archive = True
            else:
                if not whole_archive:
                    raise ValueError("source-extension whole-archive end has no start")
                whole_archive = False
            continue
        if argument == "-Wl,--as-needed":
            if dialect is not SourceExtensionLinkDialect.ELF_GNU or as_needed:
                raise ValueError("source-extension as-needed scope is invalid")
            as_needed = True
            continue
        if argument == "-Wl,--no-as-needed":
            if dialect is not SourceExtensionLinkDialect.ELF_GNU or not as_needed:
                raise ValueError("source-extension no-as-needed has no active scope")
            as_needed = False
            continue

        loading = (
            SourceExtensionLinkLoadingPolicy.ALL_MEMBERS
            if whole_archive
            else (
                SourceExtensionLinkLoadingPolicy.AS_NEEDED
                if as_needed
                else SourceExtensionLinkLoadingPolicy.DEFAULT
            )
        )
        if _is_static_input_path(argument):
            if _argument_basename(argument).casefold() in folded:
                continue
            source = _resolve_source_path(argument, path_roots)
            if source is not None:
                item = make_input(
                    argument,
                    loading=loading,
                    resolved_source=source,
                )
                if item is not None:
                    append_item(item)
                continue
        provider = _provider_from_argument(
            argument,
            dialect=dialect,
            loading=loading,
        )
        if provider is None:
            raise ValueError(
                f"source-extension {dialect.value} final link requirement is not a "
                "typed provider, retained symbol, cyclic group, or checksummed input: "
                f"{argument!r}"
            )
        append_item(provider)

    if group_members is not None:
        raise ValueError("source-extension cyclic group start has no end")
    if whole_archive:
        raise ValueError("source-extension whole-archive start has no end")
    result = SourceExtensionLinkRequirements(
        target_triple=target_triple.strip().lower(),
        items=tuple(items),
        retained_symbols=tuple(sorted(retained_symbols)),
    )
    for item in result.items:
        _validate_item(item, dialect=dialect, package_relative=True)
    return result


def _parse_loading(
    value: object, location: str, errors: list[str]
) -> SourceExtensionLinkLoadingPolicy | None:
    try:
        return SourceExtensionLinkLoadingPolicy(value)
    except (TypeError, ValueError):
        errors.append(f"{location}.loading is invalid")
        return None


def _parse_atom(
    raw: object,
    *,
    location: str,
    dialect: SourceExtensionLinkDialect,
    errors: list[str],
) -> SourceExtensionLinkAtom | None:
    if not isinstance(raw, Mapping):
        errors.append(f"{location} must be an object")
        return None
    kind = raw.get("kind")
    if kind == "provider":
        if set(raw) != {"kind", "provider_kind", "name", "loading"}:
            errors.append(
                f"{location} provider keys must be exactly kind, provider_kind, name, loading"
            )
        try:
            provider_kind = SourceExtensionLinkProviderKind(raw.get("provider_kind"))
        except (TypeError, ValueError):
            errors.append(f"{location}.provider_kind is invalid")
            return None
        name = raw.get("name")
        loading = _parse_loading(raw.get("loading"), location, errors)
        if not isinstance(name, str) or not name:
            errors.append(f"{location}.name must be a non-empty string")
            return None
        if loading is None:
            return None
        item: SourceExtensionLinkAtom = SourceExtensionLinkProvider(
            provider_kind,
            name,
            loading,
        )
    elif kind == "input":
        if set(raw) != {"kind", "path", "sha256", "loading"}:
            errors.append(
                f"{location} input keys must be exactly kind, path, sha256, loading"
            )
        path = raw.get("path")
        sha256 = raw.get("sha256")
        loading = _parse_loading(raw.get("loading"), location, errors)
        if not isinstance(path, str) or not path:
            errors.append(f"{location}.path must be a non-empty string")
            return None
        if not isinstance(sha256, str):
            errors.append(f"{location}.sha256 must be a string")
            return None
        if loading is None:
            return None
        item = SourceExtensionLinkInput(path, sha256, loading)
    else:
        errors.append(f"{location}.kind must be provider or input")
        return None
    try:
        _validate_item(item, dialect=dialect, package_relative=True)
    except ValueError as exc:
        errors.append(f"{location}: {exc}")
        return None
    return item


def parse_source_extension_link_requirements(
    manifest: Mapping[str, Any],
    *,
    expected_target_triple: str,
) -> tuple[SourceExtensionLinkRequirements | None, list[str]]:
    raw = manifest.get("link_requirements")
    if raw is None:
        return None, ["link_requirements must be an explicit object"]
    if not isinstance(raw, Mapping):
        return None, ["link_requirements must be an object"]
    errors: list[str] = []
    if set(raw) != {"target_triple", "items", "retained_symbols"}:
        errors.append(
            "link_requirements keys must be exactly target_triple, items, retained_symbols"
        )
    try:
        dialect = source_extension_link_dialect(expected_target_triple)
    except RuntimeError as exc:
        return None, [str(exc)]
    target = raw.get("target_triple")
    if not isinstance(target, str) or not target:
        errors.append("link_requirements.target_triple must be a non-empty string")
        normalized_target = ""
    else:
        normalized_target = target.strip().lower()
        if target != normalized_target:
            errors.append("link_requirements.target_triple must be canonical lowercase")
        if normalized_target != expected_target_triple.lower():
            errors.append(
                "link_requirements.target_triple must match target_triple "
                f"({normalized_target!r} != {expected_target_triple.lower()!r})"
            )
    raw_symbols = raw.get("retained_symbols")
    if not isinstance(raw_symbols, list) or not all(
        isinstance(symbol, str) and _is_link_symbol(symbol) for symbol in raw_symbols
    ):
        errors.append(
            "link_requirements.retained_symbols must be a list of canonical symbols"
        )
        retained_symbols: tuple[str, ...] = ()
    else:
        retained_symbols = tuple(raw_symbols)
        if list(retained_symbols) != sorted(set(retained_symbols)):
            errors.append(
                "link_requirements.retained_symbols must be unique and sorted"
            )
    raw_items = raw.get("items")
    if not isinstance(raw_items, list):
        errors.append("link_requirements.items must be a list")
        raw_items = []
    items: list[SourceExtensionLinkItem] = []
    for index, raw_item in enumerate(raw_items):
        location = f"link_requirements.items[{index}]"
        if isinstance(raw_item, Mapping) and raw_item.get("kind") == "cyclic-group":
            if set(raw_item) != {"kind", "members"}:
                errors.append(
                    f"{location} cyclic-group keys must be exactly kind, members"
                )
            raw_members = raw_item.get("members")
            if not isinstance(raw_members, list) or not raw_members:
                errors.append(f"{location}.members must be a non-empty list")
                continue
            members = tuple(
                member
                for member_index, raw_member in enumerate(raw_members)
                if (
                    member := _parse_atom(
                        raw_member,
                        location=f"{location}.members[{member_index}]",
                        dialect=dialect,
                        errors=errors,
                    )
                )
                is not None
            )
            if len(members) == len(raw_members):
                group = SourceExtensionLinkCyclicGroup(members)
                try:
                    _validate_item(group, dialect=dialect, package_relative=True)
                except ValueError as exc:
                    errors.append(f"{location}: {exc}")
                else:
                    items.append(group)
            continue
        item = _parse_atom(
            raw_item,
            location=location,
            dialect=dialect,
            errors=errors,
        )
        if item is not None:
            items.append(item)
    if errors:
        return None, errors
    return (
        SourceExtensionLinkRequirements(
            normalized_target,
            tuple(items),
            retained_symbols,
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
    if actual != item.sha256:
        return (
            None,
            f"link requirement checksum mismatch for {item.path}: "
            f"expected {item.sha256}, got {actual}",
        )
    return selected, None


def _map_link_inputs(
    requirements: SourceExtensionLinkRequirements,
    mapper: Callable[[SourceExtensionLinkInput], SourceExtensionLinkInput],
) -> SourceExtensionLinkRequirements:
    items: list[SourceExtensionLinkItem] = []
    for item in requirements.items:
        if isinstance(item, SourceExtensionLinkInput):
            items.append(mapper(item))
        elif isinstance(item, SourceExtensionLinkCyclicGroup):
            items.append(
                SourceExtensionLinkCyclicGroup(
                    tuple(
                        mapper(member)
                        if isinstance(member, SourceExtensionLinkInput)
                        else member
                        for member in item.members
                    )
                )
            )
        else:
            items.append(item)
    return SourceExtensionLinkRequirements(
        requirements.target_triple,
        tuple(items),
        requirements.retained_symbols,
    )


def resolve_source_extension_link_requirements(
    requirements: SourceExtensionLinkRequirements,
    *,
    package_root: Path,
    manifest_dir: Path,
) -> tuple[SourceExtensionLinkRequirements | None, list[str]]:
    resolved: dict[SourceExtensionLinkInput, SourceExtensionLinkInput] = {}
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
        resolved[item] = SourceExtensionLinkInput(
            str(selected),
            item.sha256,
            item.loading,
        )
    if errors:
        return None, errors
    return _map_link_inputs(requirements, resolved.__getitem__), []


def resolve_source_extension_link_arguments(
    requirements: SourceExtensionLinkRequirements,
    *,
    package_root: Path,
    manifest_dir: Path,
) -> tuple[tuple[str, ...] | None, list[str]]:
    resolved, errors = resolve_source_extension_link_requirements(
        requirements,
        package_root=package_root,
        manifest_dir=manifest_dir,
    )
    return (
        None if resolved is None else render_source_extension_link_arguments(resolved),
        errors,
    )


def relocate_source_extension_link_inputs(
    requirements: SourceExtensionLinkRequirements,
    *,
    relocated_paths: Mapping[str, Path],
) -> SourceExtensionLinkRequirements:
    missing = sorted({item.path for item in requirements.inputs} - set(relocated_paths))
    if missing:
        raise ValueError(
            "missing relocated source-extension link inputs: " + ", ".join(missing)
        )
    return _map_link_inputs(
        requirements,
        lambda item: SourceExtensionLinkInput(
            str(relocated_paths[item.path]),
            item.sha256,
            item.loading,
        ),
    )


def materialize_source_extension_link_requirements(
    requirements: SourceExtensionLinkRequirements,
    *,
    package_root: Path,
    manifest_dir: Path,
    publish_root: Path,
) -> tuple[SourceExtensionLinkRequirements | None, list[str]]:
    published: dict[SourceExtensionLinkInput, SourceExtensionLinkInput] = {}
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
        published[item] = SourceExtensionLinkInput(
            destination.relative_to(publish_root).as_posix(),
            item.sha256,
            item.loading,
        )
    if errors:
        return None, errors
    return _map_link_inputs(requirements, published.__getitem__), []
