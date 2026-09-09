"""Platform-gated loaded CPython native/system ABI dependency closure.

Runtime roots and the complete observed loaded-image census are attested here.
Mandatory imports must resolve within that census or explicit virtual OS
contracts. Optional declarations are retained, never inferred to be bound from
a matching basename. This snapshot does not attest future loader selections,
unloaded extension dependencies, or resolve rpaths. Later loads need readmission.
"""

from __future__ import annotations

import struct
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
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
    _loaded_native_module_paths,
    _loader_name,
    _native_contract_valid,
)


DependencyKind = Literal["required", "delay", "weak", "lazy", "reexport", "upward"]
DEFERRED_DEPENDENCY_KINDS = {
    "windows": frozenset({"delay"}),
    "macos": frozenset({"weak", "lazy"}),
    "linux": frozenset(),
}


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
    data: bytes, operating_system: str, architecture: str | None
) -> NativeHeader:
    try:
        object_format = native_object_format_for_os(operating_system)
        shape = (
            native_artifact_shape(architecture, object_format=object_format)
            if architecture is not None
            else None
        )
        return native_artifact_from_bytes(data).admit(
            object_format=object_format, kinds=LOADED_IMAGE_KINDS, shape=shape
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


def _macho_dependencies(
    data: bytes, *, architecture: str | None = None
) -> tuple[NativeDependency, ...]:
    header = _dependency_header(data, "macos", architecture)
    metadata = header.metadata
    assert isinstance(metadata, MachOHeader)
    endian = header.endian
    command_count = metadata.command_count
    offset = header.offset + metadata.header_size
    limit = offset + metadata.command_bytes
    dylib_commands: dict[int, DependencyKind] = {
        0xC: "required",
        0x18: "weak",
        0x1F: "reexport",
        0x20: "lazy",
        0x23: "upward",
    }
    dependencies: set[NativeDependency] = set()
    for _index in range(command_count):
        if offset + 8 > limit:
            raise PythonEnvironmentIdentityError(
                "loaded macOS dependency has a truncated load command"
            )
        command, size = struct.unpack_from(endian + "II", data, offset)
        base_command = command & 0x7FFFFFFF
        if size < 8 or offset + size > limit:
            raise PythonEnvironmentIdentityError(
                "loaded macOS dependency has an invalid load command"
            )
        if base_command in dylib_commands:
            if size < 24:
                raise PythonEnvironmentIdentityError(
                    "Mach-O dylib command is truncated"
                )
            name_offset = struct.unpack_from(endian + "I", data, offset + 8)[0]
            start = offset + name_offset
            end = data.find(b"\0", start, offset + size)
            if name_offset < 24 or end < 0:
                raise PythonEnvironmentIdentityError(
                    "Mach-O dependency name is invalid"
                )
            try:
                name = data[start:end].decode("utf-8")
            except UnicodeDecodeError as exc:
                raise PythonEnvironmentIdentityError(
                    "Mach-O dependency name is not UTF-8"
                ) from exc
            if not name:
                raise PythonEnvironmentIdentityError("Mach-O dependency name is empty")
            dependencies.add(NativeDependency(name, dylib_commands[base_command]))
        offset += size
    if offset != limit:
        raise PythonEnvironmentIdentityError(
            "Mach-O load-command count/extent disagree"
        )
    return tuple(sorted(dependencies))


def _native_dependencies(
    data: bytes, operating_system: str, *, architecture: str | None = None
) -> tuple[NativeDependency, ...]:
    if operating_system == "windows":
        return _pe_dependencies(data, architecture=architecture)
    if operating_system == "macos":
        return _macho_dependencies(data, architecture=architecture)
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
    loaded, loader_aliases, loader_contracts = _loaded_native_module_paths(
        operating_system
    )
    by_name: dict[str, Path] = dict(loader_aliases)
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
        root_name = _loader_name(canonical.name, operating_system)
        prior = by_name.get(root_name)
        if prior is not None and not canonical.samefile(prior):
            raise PythonEnvironmentIdentityError(
                f"runtime root has an ambiguous loader name: {canonical.name}"
            )
        by_name[root_name] = canonical
    discovered: dict[str, Path] = {}
    nodes: dict[str, str] = {}
    contracts: set[str] = set(loader_contracts)
    raw_edges: set[tuple[str, str]] = set()
    deferred: set[tuple[str, NativeDependency]] = set()
    observed_paths = {path.resolve(strict=True) for path in loaded}
    pending = sorted(
        observed_paths | set(roles_by_path),
        key=lambda path: _loader_name(path.name, operating_system),
    )
    while pending:
        path = pending.pop()
        name = _loader_name(path.name, operating_system)
        if name in discovered:
            continue
        metadata = path.lstat()
        node = pool.bind(path, metadata, label="loaded native dependency")
        data = pool.read_bound(node, label="loaded native dependency")
        nodes[name] = node
        discovered[name] = path
        dependencies = _native_dependencies(
            data, operating_system, architecture=architecture
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
                contract = next(
                    (
                        value
                        for value in loader_contracts
                        if value.partition(":")[2] == dependency_key
                    ),
                    None,
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
            target_key = _loader_name(target.name, operating_system)
            raw_edges.add((name, target_key))
            if target_key not in discovered:
                pending.append(target)
    ordered_names = sorted(discovered)
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
        component["id"] for component in components if component["roles"]
    )
    if not root_components:
        raise PythonEnvironmentIdentityError(
            "native dependency closure has no runtime roots"
        )
    observed_components = [
        ids[name] for name in ordered_names if discovered[name] in observed_paths
    ]
    deferred_imports = [
        {"from": ids[source], "name": declaration.name, "kind": declaration.kind}
        for source, declaration in sorted(deferred)
    ]

    def verify_census() -> None:
        after_loaded, after_aliases, after_contracts = _loaded_native_module_paths(
            operating_system
        )
        if (
            set(after_loaded) != set(loaded)
            or after_aliases != loader_aliases
            or set(after_contracts) != set(loader_contracts)
        ):
            raise PythonEnvironmentIdentityError(
                "loaded native image census changed during dependency capture"
            )

    verify_census()
    pool.capture_context.register_verification_fence(verify_census)
    material = {
        "policy": policy,
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
