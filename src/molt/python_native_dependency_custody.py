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


def _pe_dependencies(data: bytes) -> tuple[NativeDependency, ...]:
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise PythonEnvironmentIdentityError(
            "loaded Windows dependency is not a PE image"
        )
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset + 24 > len(data) or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise PythonEnvironmentIdentityError(
            "loaded Windows dependency has an invalid PE header"
        )
    section_count = struct.unpack_from("<H", data, pe_offset + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
    optional = pe_offset + 24
    if optional_size < 2 or optional + optional_size > len(data):
        raise PythonEnvironmentIdentityError(
            "loaded Windows dependency has a truncated PE header"
        )
    magic = struct.unpack_from("<H", data, optional)[0]
    directory_offset = optional + (
        112 if magic == 0x20B else 96 if magic == 0x10B else -1
    )
    if directory_offset < optional or directory_offset > optional + optional_size:
        raise PythonEnvironmentIdentityError(
            "loaded Windows dependency has an unsupported PE format"
        )
    directory_count = struct.unpack_from("<I", data, directory_offset - 4)[0]
    if directory_count > (optional + optional_size - directory_offset) // 8:
        raise PythonEnvironmentIdentityError("PE data directories are truncated")
    section_offset = optional + optional_size
    sections: list[tuple[int, int, int, int]] = []
    for index in range(section_count):
        offset = section_offset + index * 40
        if offset + 40 > len(data):
            raise PythonEnvironmentIdentityError(
                "loaded Windows dependency has truncated sections"
            )
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
            "<IIII", data, offset + 8
        )
        if raw_offset + raw_size > len(data):
            raise PythonEnvironmentIdentityError("PE section raw data is truncated")
        sections.append((virtual_address, virtual_size, raw_offset, raw_size))
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
            image_base = struct.unpack_from(
                "<Q" if magic == 0x20B else "<I",
                data,
                optional + (24 if magic == 0x20B else 28),
            )[0]
            read_name(
                descriptor[1] if descriptor[0] else descriptor[1] - image_base,
                "delay",
            )
            offset += 32
        else:
            raise PythonEnvironmentIdentityError(
                "PE delay-import table is unterminated"
            )
    return tuple(sorted(dependencies))


def _elf_dependencies(data: bytes) -> tuple[NativeDependency, ...]:
    if len(data) < 64 or data[:4] != b"\x7fELF":
        raise PythonEnvironmentIdentityError(
            "loaded Linux dependency is not an ELF image"
        )
    elf_class = data[4]
    byte_order = data[5]
    if byte_order not in {1, 2} or elf_class not in {1, 2}:
        raise PythonEnvironmentIdentityError(
            "loaded Linux dependency has unsupported ELF metadata"
        )
    endian = "<" if byte_order == 1 else ">"
    if elf_class == 2:
        phoff = struct.unpack_from(endian + "Q", data, 32)[0]
        phentsize, phnum = struct.unpack_from(endian + "HH", data, 54)
        ph_format = endian + "IIQQQQQQ"
        dyn_format = endian + "qQ"
    else:
        phoff = struct.unpack_from(endian + "I", data, 28)[0]
        phentsize, phnum = struct.unpack_from(endian + "HH", data, 42)
        ph_format = endian + "IIIIIIII"
        dyn_format = endian + "iI"
    program_headers: list[tuple[int, int, int, int]] = []
    dynamic: tuple[int, int] | None = None
    if phentsize < struct.calcsize(ph_format) or phoff + phnum * phentsize > len(data):
        raise PythonEnvironmentIdentityError("ELF program-header extent is invalid")
    for index in range(phnum):
        offset = phoff + index * phentsize
        if offset + struct.calcsize(ph_format) > len(data):
            raise PythonEnvironmentIdentityError(
                "loaded Linux dependency has truncated program headers"
            )
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
    fat_formats = {
        b"\xca\xfe\xba\xbe": (">", False),
        b"\xbe\xba\xfe\xca": ("<", False),
        b"\xca\xfe\xba\xbf": (">", True),
        b"\xbf\xba\xfe\xca": ("<", True),
    }
    cpu_types = {"x86_64": 0x01000007, "arm64": 0x0100000C}
    if data[:4] in fat_formats:
        endian, fat64 = fat_formats[data[:4]]
        if len(data) < 8:
            raise PythonEnvironmentIdentityError("Mach-O universal header is truncated")
        count = struct.unpack_from(endian + "I", data, 4)[0]
        row_format = endian + ("IIQQII" if fat64 else "IIIII")
        row_size = struct.calcsize(row_format)
        table_end = 8 + count * row_size
        if not count or table_end > len(data):
            raise PythonEnvironmentIdentityError(
                "Mach-O universal slice table is invalid"
            )
        selected_arch = architecture
        if selected_arch is None:
            raise PythonEnvironmentIdentityError(
                "Mach-O universal images require an explicit runtime architecture"
            )
        target = cpu_types.get(selected_arch)
        if target is None:
            raise PythonEnvironmentIdentityError(
                "Mach-O runtime architecture is unsupported"
            )
        slices: list[tuple[int, int]] = []
        selected: tuple[int, int] | None = None
        for index in range(count):
            values = struct.unpack_from(row_format, data, 8 + index * row_size)
            cpu, _subtype, start, size, alignment = values[:5]
            if (
                not size
                or start < table_end
                or start + size > len(data)
                or alignment > 63
                or start % (1 << alignment)
                or any(start < end and begin < start + size for begin, end in slices)
            ):
                raise PythonEnvironmentIdentityError(
                    "Mach-O universal slice extent is invalid"
                )
            slices.append((start, start + size))
            if cpu == target:
                if selected is not None:
                    raise PythonEnvironmentIdentityError(
                        "Mach-O universal architecture is ambiguous"
                    )
                selected = (start, start + size)
        if selected is None:
            raise PythonEnvironmentIdentityError(
                "Mach-O universal image lacks runtime architecture"
            )
        data = data[selected[0] : selected[1]]
        architecture = selected_arch
    if len(data) < 28:
        raise PythonEnvironmentIdentityError(
            "loaded macOS dependency has a truncated Mach-O header"
        )
    magic = data[:4]
    formats = {
        b"\xce\xfa\xed\xfe": ("<", False),
        b"\xcf\xfa\xed\xfe": ("<", True),
        b"\xfe\xed\xfa\xce": (">", False),
        b"\xfe\xed\xfa\xcf": (">", True),
    }
    try:
        endian, is_64 = formats[magic]
    except KeyError as exc:
        raise PythonEnvironmentIdentityError(
            "loaded macOS dependency is not a thin Mach-O image"
        ) from exc
    command_count, command_bytes = struct.unpack_from(endian + "II", data, 16)
    if architecture is not None and struct.unpack_from(endian + "I", data, 4)[
        0
    ] != cpu_types.get(architecture):
        raise PythonEnvironmentIdentityError(
            "Mach-O image does not match runtime architecture"
        )
    offset = 32 if is_64 else 28
    limit = offset + command_bytes
    if limit > len(data):
        raise PythonEnvironmentIdentityError(
            "loaded macOS dependency has truncated load commands"
        )
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
        names = _pe_dependencies(data)
        if architecture is not None:
            offset = struct.unpack_from("<I", data, 0x3C)[0]
            machine = struct.unpack_from("<H", data, offset + 4)[0]
            if machine != {"x86_64": 0x8664, "arm64": 0xAA64}.get(architecture):
                raise PythonEnvironmentIdentityError(
                    "PE image does not match runtime architecture"
                )
        return names
    if operating_system == "macos":
        return _macho_dependencies(data, architecture=architecture)
    if operating_system == "linux":
        names = _elf_dependencies(data)
        if architecture is not None and (
            data[4:6] != b"\x02\x01"
            or struct.unpack_from("<H", data, 18)[0]
            != {"x86_64": 62, "arm64": 183}.get(architecture)
        ):
            raise PythonEnvironmentIdentityError(
                "ELF image does not match runtime architecture"
            )
        return names
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
