"""Platform-gated loaded CPython native/system ABI dependency closure.

Runtime roots and the complete observed loaded-image census are attested here.
Mandatory imports must resolve within that census or explicit virtual OS
contracts. Optional declarations are retained, never inferred to be bound from
a matching basename. Importer-local Mach-O run paths are resolved exactly;
inherited dyld run-path stacks are not inferred. This snapshot does not attest
future loader selections or unloaded extension dependencies. Later loads need
readmission.
"""

from __future__ import annotations

import os
import struct
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import Literal

from molt.exact_json import canonical_json_sha256
from molt.native_artifact_header import (
    ElfHeader,
    LOADED_IMAGE_KINDS,
    MachOHeader,
    NativeArtifactError,
    NativeHeader,
    PeHeader,
    native_artifact_from_bytes,
)
from molt.native_target_shape import native_artifact_shape, native_object_format_for_os
from molt.python_file_node_custody import _FileNodePool
from molt.python_identity_common import PythonEnvironmentIdentityError
from molt.python_native_locations import (
    _loaded_native_module_snapshot,
    _loader_name,
    _macos_dyld_cache_contract,
    _native_contract_valid,
)


DependencyKind = Literal["required", "delay", "weak", "lazy", "reexport", "upward"]
DEFERRED_DEPENDENCY_KINDS = {
    "windows": frozenset({"delay"}),
    "macos": frozenset({"weak", "lazy"}),
    "linux": frozenset(),
}


def _canonical_deferred_dependency_key(
    source: str,
    name: str,
    kind: str,
    operating_system: str,
) -> tuple[int, str, str]:
    """Order by published component id, normalized loader name, then kind."""

    return (
        int(source.removeprefix("native-component-")),
        _loader_name(name, operating_system),
        kind,
    )


@dataclass(frozen=True, order=True, slots=True)
class NativeDependency:
    name: str
    kind: DependencyKind


def _pe_rva_offset(
    data: bytes,
    rva: int,
    sections: Sequence[tuple[int, int, int, int]],
    *,
    size: int = 1,
) -> int:
    for virtual_address, _virtual_size, raw_offset, raw_size in sections:
        extent = raw_size
        if virtual_address <= rva < virtual_address + extent:
            offset = raw_offset + rva - virtual_address
            if (
                offset < 0
                or size < 1
                or offset + size > min(len(data), raw_offset + raw_size)
            ):
                break
            return offset
    raise PythonEnvironmentIdentityError(
        "PE dependency table references an invalid RVA"
    )


def _dependency_header(
    data: bytes,
    operating_system: str,
    architecture: str | None,
    *,
    loaded_macho_identity: tuple[int, int] | None = None,
) -> NativeHeader:
    try:
        object_format = native_object_format_for_os(operating_system)
        shape = (
            native_artifact_shape(architecture, object_format=object_format)
            if architecture is not None
            else None
        )
        return native_artifact_from_bytes(data).admit(
            object_format=object_format,
            kinds=LOADED_IMAGE_KINDS,
            shape=shape,
            loaded_macho_identity=loaded_macho_identity,
        )
    except (NativeArtifactError, RuntimeError) as exc:
        raise PythonEnvironmentIdentityError(str(exc)) from exc


def _pe_dependencies(
    data: bytes, *, architecture: str | None = None
) -> tuple[NativeDependency, ...]:
    header = _dependency_header(data, "windows", architecture)
    metadata = header.metadata
    assert isinstance(metadata, PeHeader)
    sections = metadata.sections
    directory_offset = metadata.directory_offset
    directory_count = metadata.directory_count
    dependencies: set[NativeDependency] = set()

    def read_name(rva: int, kind: DependencyKind) -> None:
        offset = _pe_rva_offset(data, rva, sections)
        section_end = next(
            raw_offset + raw_size
            for virtual_address, _virtual_size, raw_offset, raw_size in sections
            if virtual_address <= rva < virtual_address + raw_size
        )
        end = data.find(b"\0", offset, section_end)
        if end < 0:
            raise PythonEnvironmentIdentityError("PE dependency name is unterminated")
        try:
            name = data[offset:end].decode("ascii").casefold()
        except UnicodeDecodeError as exc:
            raise PythonEnvironmentIdentityError(
                "PE dependency name is not ASCII"
            ) from exc
        if not name or "/" in name or "\\" in name:
            raise PythonEnvironmentIdentityError("PE dependency name is invalid")
        dependencies.add(NativeDependency(name, kind))

    def directory(index: int) -> tuple[int, int]:
        if index >= directory_count:
            return 0, 0
        rva, size = struct.unpack_from("<II", data, directory_offset + index * 8)
        if bool(rva) != bool(size):
            raise PythonEnvironmentIdentityError(
                "PE dependency directory is incomplete"
            )
        return rva, size

    import_rva, import_size = directory(1)
    if import_rva and import_size:
        offset = _pe_rva_offset(data, import_rva, sections, size=import_size)
        limit = offset + import_size
        while offset + 20 <= limit:
            descriptor = struct.unpack_from("<IIIII", data, offset)
            if not any(descriptor):
                break
            read_name(descriptor[3], "required")
            offset += 20
        else:
            raise PythonEnvironmentIdentityError("PE import table is unterminated")
    delay_rva, delay_size = directory(13)
    if delay_rva and delay_size:
        offset = _pe_rva_offset(data, delay_rva, sections, size=delay_size)
        limit = offset + delay_size
        while offset + 32 <= limit:
            descriptor = struct.unpack_from("<IIIIIIII", data, offset)
            if not any(descriptor):
                break
            if descriptor[0] not in (0, 1):
                raise PythonEnvironmentIdentityError(
                    "PE delay-import attributes are invalid"
                )
            read_name(
                descriptor[1] if descriptor[0] else descriptor[1] - metadata.image_base,
                "delay",
            )
            offset += 32
        else:
            raise PythonEnvironmentIdentityError(
                "PE delay-import table is unterminated"
            )
    return tuple(sorted(dependencies))


def _elf_dependencies(
    data: bytes, *, architecture: str | None = None
) -> tuple[NativeDependency, ...]:
    header = _dependency_header(data, "linux", architecture)
    metadata = header.metadata
    assert isinstance(metadata, ElfHeader)
    elf_class = 2 if header.bits == 64 else 1
    endian = header.endian
    phoff = metadata.program_offset
    phentsize = metadata.program_entry_size
    phnum = metadata.program_count
    ph_format = endian + ("IIQQQQQQ" if header.bits == 64 else "IIIIIIII")
    dyn_format = endian + ("qQ" if header.bits == 64 else "iI")
    program_headers: list[tuple[int, int, int, int]] = []
    dynamic: tuple[int, int] | None = None
    for index in range(phnum):
        offset = phoff + index * phentsize
        values = struct.unpack_from(ph_format, data, offset)
        if elf_class == 2:
            p_type, _flags, p_offset, p_vaddr, _paddr, p_filesz, p_memsz, _align = (
                values
            )
        else:
            p_type, p_offset, p_vaddr, _paddr, p_filesz, p_memsz, _flags, _align = (
                values
            )
        if p_type == 1:
            program_headers.append((p_vaddr, p_memsz, p_offset, p_filesz))
        elif p_type == 2:
            if dynamic is not None:
                raise PythonEnvironmentIdentityError(
                    "ELF image has multiple dynamic tables"
                )
            dynamic = (p_offset, p_filesz)
    if dynamic is None:
        return ()
    dyn_offset, dyn_size = dynamic
    entry_size = struct.calcsize(dyn_format)
    string_address: int | None = None
    string_size: int | None = None
    needed: list[int] = []
    if not dyn_size or dyn_size % entry_size or dyn_offset + dyn_size > len(data):
        raise PythonEnvironmentIdentityError("ELF dynamic table extent is invalid")
    for offset in range(dyn_offset, dyn_offset + dyn_size, entry_size):
        tag, value = struct.unpack_from(dyn_format, data, offset)
        if tag == 0:
            break
        if tag == 1:
            needed.append(value)
        elif tag == 5:
            if string_address is not None:
                raise PythonEnvironmentIdentityError(
                    "ELF dynamic string table is duplicated"
                )
            string_address = value
        elif tag == 10:
            if string_size is not None:
                raise PythonEnvironmentIdentityError(
                    "ELF dynamic string-table size is duplicated"
                )
            string_size = value
    else:
        raise PythonEnvironmentIdentityError("ELF dynamic table is unterminated")
    if string_address is None or string_size is None:
        if needed:
            raise PythonEnvironmentIdentityError(
                "ELF dependency table has no string table"
            )
        return ()
    string_offset: int | None = None
    for virtual_address, memory_size, file_offset, file_size in program_headers:
        if virtual_address <= string_address < virtual_address + memory_size:
            candidate = file_offset + string_address - virtual_address
            if (
                candidate + string_size <= file_offset + file_size
                and candidate + string_size <= len(data)
            ):
                string_offset = candidate
                break
    if string_offset is None:
        raise PythonEnvironmentIdentityError(
            "ELF dependency string table is outside loaded segments"
        )
    names: set[str] = set()
    for name_offset in needed:
        start = string_offset + name_offset
        end = data.find(b"\0", start, string_offset + string_size)
        if start < string_offset or end < 0:
            raise PythonEnvironmentIdentityError("ELF dependency name is invalid")
        try:
            name = data[start:end].decode("utf-8")
        except UnicodeDecodeError as exc:
            raise PythonEnvironmentIdentityError(
                "ELF dependency name is not UTF-8"
            ) from exc
        if not name:
            raise PythonEnvironmentIdentityError("ELF dependency name is invalid")
        names.add(name)
    return tuple(NativeDependency(name, "required") for name in sorted(names))


def _macho_load_commands(
    data: bytes,
    *,
    architecture: str | None = None,
    loaded_macho_identity: tuple[int, int] | None = None,
) -> tuple[str, tuple[tuple[int, int, int], ...]]:
    header = _dependency_header(
        data,
        "macos",
        architecture,
        loaded_macho_identity=loaded_macho_identity,
    )
    metadata = header.metadata
    assert isinstance(metadata, MachOHeader)
    endian = header.endian
    command_count = metadata.command_count
    offset = header.offset + metadata.header_size
    limit = offset + metadata.command_bytes
    commands: list[tuple[int, int, int]] = []
    for _index in range(command_count):
        if offset + 8 > limit:
            raise PythonEnvironmentIdentityError(
                "loaded macOS dependency has a truncated load command"
            )
        command, size = struct.unpack_from(endian + "II", data, offset)
        if size < 8 or offset + size > limit:
            raise PythonEnvironmentIdentityError(
                "loaded macOS dependency has an invalid load command"
            )
        commands.append((command & 0x7FFFFFFF, offset, size))
        offset += size
    if offset != limit:
        raise PythonEnvironmentIdentityError(
            "Mach-O load-command count/extent disagree"
        )
    return endian, tuple(commands)


def _macho_command_string(
    data: bytes, *, offset: int, size: int, minimum_offset: int, label: str, endian: str
) -> str:
    if size < minimum_offset:
        raise PythonEnvironmentIdentityError(f"Mach-O {label} is truncated")
    name_offset = struct.unpack_from(endian + "I", data, offset + 8)[0]
    start = offset + name_offset
    end = data.find(b"\0", start, offset + size)
    if name_offset < minimum_offset or end < 0:
        raise PythonEnvironmentIdentityError(f"Mach-O {label} is invalid")
    try:
        value = data[start:end].decode("utf-8")
    except UnicodeDecodeError as exc:
        raise PythonEnvironmentIdentityError(f"Mach-O {label} is not UTF-8") from exc
    if not value:
        raise PythonEnvironmentIdentityError(f"Mach-O {label} is empty")
    return value


def _macho_dependencies(
    data: bytes,
    *,
    architecture: str | None = None,
    loaded_macho_identity: tuple[int, int] | None = None,
) -> tuple[NativeDependency, ...]:
    endian, commands = _macho_load_commands(
        data,
        architecture=architecture,
        loaded_macho_identity=loaded_macho_identity,
    )
    dylib_commands: dict[int, DependencyKind] = {
        0xC: "required",
        0x18: "weak",
        0x1F: "reexport",
        0x20: "lazy",
        0x23: "upward",
    }
    dependencies: set[NativeDependency] = set()
    for command, offset, size in commands:
        if command in dylib_commands:
            if size < 24:
                raise PythonEnvironmentIdentityError(
                    "Mach-O dylib command is truncated"
                )
            name = _macho_command_string(
                data,
                offset=offset,
                size=size,
                minimum_offset=24,
                label="dependency name",
                endian=endian,
            )
            dependencies.add(NativeDependency(name, dylib_commands[command]))
    return tuple(sorted(dependencies))


def _macho_rpaths(
    data: bytes,
    *,
    architecture: str | None = None,
    loaded_macho_identity: tuple[int, int] | None = None,
) -> tuple[str, ...]:
    endian, commands = _macho_load_commands(
        data,
        architecture=architecture,
        loaded_macho_identity=loaded_macho_identity,
    )
    rpaths = {
        _macho_command_string(
            data,
            offset=offset,
            size=size,
            minimum_offset=12,
            label="LC_RPATH value",
            endian=endian,
        )
        for command, offset, size in commands
        if command == 0x1C
    }
    return tuple(sorted(rpaths))


def _resolve_macos_loaded_dependency(
    dependency: str,
    *,
    importer: Path,
    rpaths: Sequence[str],
    executable: Path,
    known_paths_by_object: Mapping[tuple[int, int], Path],
) -> Path | None:
    """Resolve direct and importer-local dyld scopes against the loaded census."""

    def scoped_path(value: str, *, loader: Path) -> Path | None:
        if value == "@loader_path":
            return loader.parent
        if value.startswith("@loader_path/"):
            return loader.parent / value.removeprefix("@loader_path/")
        if value == "@executable_path":
            return executable.parent
        if value.startswith("@executable_path/"):
            return executable.parent / value.removeprefix("@executable_path/")
        if PurePosixPath(value).is_absolute():
            return Path(value)
        return None

    declared: list[Path] = []
    if dependency.startswith("@rpath/"):
        suffix = dependency.removeprefix("@rpath/")
        for rpath in rpaths:
            root = scoped_path(rpath, loader=importer)
            if root is None:
                raise PythonEnvironmentIdentityError(
                    f"unsupported Mach-O LC_RPATH {rpath!r} in {importer.name}"
                )
            declared.append(root / suffix)
    else:
        direct = scoped_path(dependency, loader=importer)
        if direct is None:
            raise PythonEnvironmentIdentityError(
                f"unsupported Mach-O dependency install name {dependency!r}"
            )
        declared.append(direct)
    matches: set[Path] = set()
    for candidate in declared:
        try:
            resolved = candidate.resolve(strict=True)
        except OSError:
            continue
        metadata = resolved.stat()
        loaded = known_paths_by_object.get((metadata.st_dev, metadata.st_ino))
        if loaded is not None:
            matches.add(loaded)
    if len(matches) > 1:
        raise PythonEnvironmentIdentityError(
            "loaded native dependency closure has ambiguous dyld binding "
            f"{dependency!r} required by {importer.name}"
        )
    if dependency.startswith("@rpath/") and not matches:
        raise PythonEnvironmentIdentityError(
            "loaded native dependency closure cannot attest importer-local "
            f"dyld binding {dependency!r} required by {importer.name}; "
            "inherited run-path stacks are unsupported"
        )
    return next(iter(matches), None)


def _loaded_path_object_index(paths: Iterable[Path]) -> dict[tuple[int, int], Path]:
    """Index the observed census once by stable filesystem object identity."""

    by_object: dict[tuple[int, int], Path] = {}
    for path in sorted(paths, key=os.fspath):
        metadata = path.stat()
        if not metadata.st_ino:
            raise PythonEnvironmentIdentityError(
                f"loaded native image has no stable object identity: {path}"
            )
        by_object.setdefault((metadata.st_dev, metadata.st_ino), path)
    return by_object


def _native_dependencies(
    data: bytes,
    operating_system: str,
    *,
    architecture: str | None = None,
    loaded_macho_identity: tuple[int, int] | None = None,
) -> tuple[NativeDependency, ...]:
    if operating_system == "windows":
        return _pe_dependencies(data, architecture=architecture)
    if operating_system == "macos":
        return _macho_dependencies(
            data,
            architecture=architecture,
            loaded_macho_identity=loaded_macho_identity,
        )
    if operating_system == "linux":
        return _elf_dependencies(data, architecture=architecture)
    raise PythonEnvironmentIdentityError(
        f"native dependency parsing is unsupported on {operating_system}"
    )


def _native_dependency_closure(
    roots: Mapping[str, Path],
    *,
    operating_system: str,
    architecture: str | None = None,
    policy: str,
    pool: _FileNodePool,
) -> dict[str, object]:
    loader_snapshot = _loaded_native_module_snapshot(operating_system)
    loaded = loader_snapshot.paths
    loader_aliases = loader_snapshot.aliases
    loader_contracts = loader_snapshot.contracts
    macos_image_identities = loader_snapshot.macho_identities
    by_name: dict[str, Path] = dict(loader_aliases)
    if operating_system != "macos":
        for path in loaded:
            key = _loader_name(path.name, operating_system)
            prior = by_name.get(key)
            if prior is not None and not path.samefile(prior):
                raise PythonEnvironmentIdentityError(
                    f"loaded native modules have an ambiguous basename: {path.name}"
                )
            by_name[key] = path
    roles_by_path: dict[Path, set[str]] = {}
    for role, path in roots.items():
        canonical = path.resolve(strict=True)
        roles_by_path.setdefault(canonical, set()).add(role)

    observed_paths = {path.resolve(strict=True) for path in loaded}
    known_paths = observed_paths | set(roles_by_path)
    macos_executable = loader_snapshot.executable
    macos_paths_by_object = (
        _loaded_path_object_index(observed_paths) if operating_system == "macos" else {}
    )

    def image_key(path: Path) -> str:
        # File components and loader import names have different identities.
        # A configured launcher may share a basename with a distinct live image.
        return os.fspath(path)

    discovered: dict[str, Path] = {}
    nodes: dict[str, str] = {}
    contracts: set[str] = set(loader_contracts)
    raw_edges: set[tuple[str, str]] = set()
    deferred: set[tuple[str, NativeDependency]] = set()
    pending = sorted(
        known_paths,
        key=lambda path: (_loader_name(path.name, operating_system), image_key(path)),
    )
    while pending:
        path = pending.pop()
        name = image_key(path)
        if name in discovered:
            continue
        metadata = path.lstat()
        node = pool.bind(path, metadata, label="loaded native dependency")
        data = pool.read_bound(node, label="loaded native dependency")
        nodes[name] = node
        discovered[name] = path
        if path not in observed_paths:
            # A configured launcher is a content-bound runtime input, not a
            # loaded importer/provider. Its pre-exec dependencies and run-path
            # scope cannot be inferred from the current process's loader census.
            del data
            continue
        loaded_macho_identity = macos_image_identities.get(path)
        rpaths = (
            _macho_rpaths(
                data,
                architecture=architecture,
                loaded_macho_identity=loaded_macho_identity,
            )
            if operating_system == "macos"
            else ()
        )
        dependencies = _native_dependencies(
            data,
            operating_system,
            architecture=architecture,
            loaded_macho_identity=loaded_macho_identity,
        )
        del data
        for declaration in dependencies:
            if declaration.kind in DEFERRED_DEPENDENCY_KINDS[operating_system]:
                # A loaded basename does not establish this importer's optional
                # binding (delay hooks and dyld weak/lazy resolution can differ).
                deferred.add((name, declaration))
                continue
            dependency = declaration.name
            dependency_key = _loader_name(Path(dependency).name, operating_system)
            if operating_system == "macos":
                target = _resolve_macos_loaded_dependency(
                    dependency,
                    importer=path,
                    rpaths=rpaths,
                    executable=macos_executable,
                    known_paths_by_object=macos_paths_by_object,
                )
            else:
                target = by_name.get(dependency_key)
            contract: str | None = None
            if (
                target is None
                and operating_system == "windows"
                and _native_contract_valid(
                    f"windows-api-set:{dependency_key}", operating_system
                )
            ):
                contract = f"windows-api-set:{dependency_key}"
            elif target is None:
                exact_macos_contract = (
                    _macos_dyld_cache_contract(dependency)
                    if operating_system == "macos"
                    else None
                )
                contract = (
                    exact_macos_contract
                    if exact_macos_contract in loader_contracts
                    else next(
                        (
                            value
                            for value in loader_contracts
                            if operating_system != "macos"
                            and value.partition(":")[2] == dependency_key
                        ),
                        None,
                    )
                )
            if target is None and contract is not None:
                contracts.add(contract)
                raw_edges.add((name, contract))
                continue
            if target is None:
                raise PythonEnvironmentIdentityError(
                    f"loaded native dependency closure cannot resolve {dependency!r} required by {path.name}"
                )
            target = target.resolve(strict=True)
            if operating_system == "linux" and "/" in dependency:
                # DT_NEEDED can name a path; a same-basename loader image is not
                # evidence that this particular path is the bound dependency.
                try:
                    same = Path(dependency).samefile(target)
                except OSError as exc:
                    raise PythonEnvironmentIdentityError(
                        f"cannot attest path-qualified native dependency: {dependency}"
                    ) from exc
                if not same:
                    raise PythonEnvironmentIdentityError(
                        f"native dependency path disagrees with loaded image: {dependency}"
                    )
            target_key = image_key(target)
            raw_edges.add((name, target_key))
            if target_key not in discovered:
                pending.append(target)
    ordered_names = sorted(
        discovered,
        key=lambda name: (
            _loader_name(discovered[name].name, operating_system),
            int(nodes[name].removeprefix("file-node-")),
        ),
    )
    ids = {
        name: f"native-component-{index}" for index, name in enumerate(ordered_names)
    }
    components: list[dict[str, object]] = []
    for name in ordered_names:
        path = discovered[name]
        components.append(
            {
                "id": ids[name],
                "filename": path.name,
                "node": nodes[name],
                "roles": sorted(roles_by_path.get(path, set())),
            }
        )
    edges = [
        {
            "from": ids[source],
            "to": ids[target] if target in ids else target,
        }
        for source, target in sorted(raw_edges)
    ]
    edges.sort(
        key=lambda edge: (
            int(str(edge["from"]).removeprefix("native-component-")),
            (
                int(str(edge["to"]).removeprefix("native-component-"))
                if str(edge["to"]).startswith("native-component-")
                else len(ids)
            ),
            "" if str(edge["to"]).startswith("native-component-") else str(edge["to"]),
        )
    )
    root_components = sorted(
        ids[name] for name in ordered_names if roles_by_path.get(discovered[name])
    )
    if not root_components:
        raise PythonEnvironmentIdentityError(
            "native dependency closure has no runtime roots"
        )
    observed_components = [
        ids[name] for name in ordered_names if discovered[name] in observed_paths
    ]
    deferred_imports: list[dict[str, str]] = [
        {"from": ids[source], "name": declaration.name, "kind": declaration.kind}
        for source, declaration in deferred
    ]
    deferred_imports.sort(
        key=lambda declaration: _canonical_deferred_dependency_key(
            declaration["from"],
            declaration["name"],
            declaration["kind"],
            operating_system,
        )
    )

    def verify_census() -> None:
        if _loaded_native_module_snapshot(operating_system) != loader_snapshot:
            raise PythonEnvironmentIdentityError(
                "loaded native image census changed during dependency capture"
            )

    verify_census()
    pool.capture_context.register_verification_fence(verify_census)
    material = {
        "policy": policy,
        "executable_component": ids[
            image_key(loader_snapshot.executable.resolve(strict=True))
        ],
        "root_components": root_components,
        "observed_components": observed_components,
        "observed_contracts": sorted(loader_contracts),
        "components": components,
        "contracts": sorted(contracts),
        "edges": edges,
        "deferred_imports": deferred_imports,
    }
    return {
        "status": "closed",
        **material,
        "closure_sha256": canonical_json_sha256(material),
    }
