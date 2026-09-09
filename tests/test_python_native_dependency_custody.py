"""Synthetic binary/loader proofs; no host compiler or live runtime capture."""

from __future__ import annotations

import struct
from dataclasses import replace
from pathlib import Path

import pytest

from molt import python_native_dependency_custody as native
from molt.python_file_node_custody import _FileNodePool
from molt.python_identity_common import PythonEnvironmentIdentityError
from molt.python_native_locations import (
    LoadedNativeModuleSnapshot,
    _native_contract_valid,
)
from molt.python_runtime_identity import _NATIVE_DEPENDENCY_POLICIES
from tests.native_artifact_fixtures import (
    elf_header,
    pe_header,
    macho_header,
    fat_macho,
)


def _loader_snapshot(
    paths: tuple[Path, ...],
    aliases: dict[str, Path],
    contracts: tuple[str, ...] = (),
    *,
    macho_identities: dict[Path, tuple[int, int]] | None = None,
) -> LoadedNativeModuleSnapshot:
    return LoadedNativeModuleSnapshot(
        executable=paths[0],
        paths=paths,
        aliases=aliases,
        contracts=contracts,
        macho_identities=macho_identities or {},
    )


def _macos_loader_snapshot(
    paths: tuple[Path, ...],
    aliases: dict[str, Path],
    contracts: tuple[str, ...] = (),
) -> LoadedNativeModuleSnapshot:
    return _loader_snapshot(
        paths,
        aliases,
        contracts,
        macho_identities={path.resolve(): (0x01000007, 3) for path in paths},
    )


def test_loader_snapshot_freezes_mapping_inputs(tmp_path: Path) -> None:
    executable = tmp_path / "Python"
    aliases = {"Python": executable}
    identities = {executable: (0x01000007, 3)}
    snapshot = _loader_snapshot((executable,), aliases, macho_identities=identities)

    aliases.clear()
    identities.clear()

    assert snapshot.aliases == {"Python": executable}
    assert snapshot.macho_identities == {executable: (0x01000007, 3)}


def test_framework_launcher_retains_file_custody_but_not_executable_scope(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launcher = tmp_path / "bin" / "python3.12"
    executable = tmp_path / "Python.app" / "Contents" / "MacOS" / "Python"
    dependency = executable.parent / "libdependency.dylib"
    decoy = launcher.parent / dependency.name
    for path in (launcher, executable, dependency, decoy):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(_macho_image())
    executable.write_bytes(
        _macho_image(name=b"@executable_path/libdependency.dylib", command=0xC)
    )
    launcher.write_bytes(
        _macho_image(name=b"@loader_path/launcher-only-unloaded.dylib", command=0xC)
    )
    snapshot = _macos_loader_snapshot(
        (executable, dependency, decoy),
        {str(path.resolve()): path for path in (executable, dependency, decoy)},
    )
    monkeypatch.setattr(native, "_loaded_native_module_snapshot", lambda _os: snapshot)
    pool = _FileNodePool()
    closure = native._native_dependency_closure(
        {"base-executable": launcher},
        operating_system="macos",
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES["macos"],
        pool=pool,
    )
    components = {row["node"]: row for row in closure["components"]}
    launcher_component = components[
        pool.bind(launcher, launcher.lstat(), label="launcher")
    ]
    executable_component = components[
        pool.bind(executable, executable.lstat(), label="image")
    ]
    dependency_component = components[
        pool.bind(dependency, dependency.lstat(), label="dependency")
    ]
    assert launcher_component["id"] in closure["root_components"]
    assert launcher_component["id"] not in closure["observed_components"]
    assert executable_component["id"] in closure["observed_components"]
    assert closure["edges"] == [
        {"from": executable_component["id"], "to": dependency_component["id"]}
    ]
    pool.capture_context.verify()
    snapshot = replace(snapshot, executable=decoy)
    with pytest.raises(PythonEnvironmentIdentityError, match="census changed"):
        pool.capture_context.verify()


def _pe_image(
    name: bytes = b"KERNEL32.dll", *, delay: bool = False, va: bool = False
) -> bytearray:
    image = pe_header()
    optional = 0x98
    struct.pack_into("<IIII", image, optional + 240 + 8, 0x400, 0x1000, 0x300, 0x200)
    if delay:
        struct.pack_into("<II", image, optional + 112 + 13 * 8, 0x1040, 64)
        struct.pack_into(
            "<II", image, 0x240, 0 if va else 1, 0x401080 if va else 0x1080
        )
    else:
        struct.pack_into("<II", image, optional + 112 + 8, 0x1000, 40)
        struct.pack_into("<IIIII", image, 0x200, 0, 0, 0, 0x1080, 0)
    image[0x280 : 0x280 + len(name) + 1] = name + b"\0"
    return image


def _elf_image(name: bytes = b"libc.so") -> bytearray:
    image = elf_header(kind=3, image_size=0x400)
    struct.pack_into("<Q", image, 32, 64)
    struct.pack_into("<HH", image, 54, 56, 2)
    struct.pack_into("<IIQQQQQQ", image, 64, 1, 0, 0, 0, 0, len(image), len(image), 1)
    struct.pack_into("<IIQQQQQQ", image, 120, 2, 0, 0x200, 0x200, 0, 64, 64, 8)
    struct.pack_into("<qQ", image, 0x200, 1, 1)
    struct.pack_into("<qQ", image, 0x210, 5, 0x300)
    strings = b"\0" + name + b"\0"
    struct.pack_into("<qQ", image, 0x220, 10, len(strings))
    image[0x300 : 0x300 + len(strings)] = strings
    return image


def _macho_image(
    *,
    cpu: int = 0x01000007,
    name: bytes = b"/usr/lib/libSystem.B.dylib",
    command: int = 0x80000018,
) -> bytearray:
    command_size = (24 + len(name) + 1 + 7) & ~7
    image = macho_header(cpu=cpu, kind=6, command_count=1, command_bytes=command_size)
    struct.pack_into("<III", image, 32, command, command_size, 24)
    image[56 : 56 + len(name)] = name
    return image


def _macho_image_with_rpaths(dependency: bytes, rpaths: tuple[bytes, ...]) -> bytearray:
    def command(command_id: int, value: bytes, minimum_size: int) -> bytes:
        size = (minimum_size + len(value) + 1 + 7) & ~7
        payload = bytearray(size)
        struct.pack_into("<III", payload, 0, command_id, size, minimum_size)
        payload[minimum_size : minimum_size + len(value)] = value
        return bytes(payload)

    commands = [command(0xC, dependency, 24)]
    commands.extend(command(0x8000001C, rpath, 12) for rpath in rpaths)
    image = macho_header(
        cpu=0x01000007,
        kind=6,
        command_count=len(commands),
        command_bytes=sum(map(len, commands)),
    )
    cursor = 32
    for payload in commands:
        image[cursor : cursor + len(payload)] = payload
        cursor += len(payload)
    return image


def _fat_macho(*, endian: str = ">", fat64: bool = False) -> bytearray:
    return fat_macho(
        (
            _macho_image(cpu=0x01000007, name=b"@rpath/x86.dylib"),
            _macho_image(cpu=0x0100000C, name=b"@rpath/arm.dylib"),
        ),
        endian=endian,
        fat64=fat64,
    )


@pytest.mark.parametrize("delay,va", [(False, False), (True, False), (True, True)])
def test_pe_import_and_delay_import_address_modes(delay: bool, va: bool) -> None:
    image = bytes(_pe_image(delay=delay, va=va))
    assert native._native_dependencies(image, "windows", architecture="x86_64") == (
        native.NativeDependency("kernel32.dll", "delay" if delay else "required"),
    )


@pytest.mark.parametrize(
    "fmt,offset,value,reason",
    [
        ("<H", 0x94, 1, "truncated PE header"),
        ("<I", 0x98 + 108, 17, "directories are truncated"),
        ("<I", 0x98 + 112 + 12, 20, "import table is unterminated"),
        ("<I", 0x98 + 112 + 12, 0, "directory is incomplete"),
        ("<I", 0x200 + 12, 0x1320, "invalid RVA"),
        ("<I", 0x98 + 240 + 16, 0x500, "raw data is truncated"),
    ],
)
def test_pe_rejects_malformed_extents(
    fmt: str, offset: int, value: int, reason: str
) -> None:
    image = _pe_image()
    struct.pack_into(fmt, image, offset, value)
    with pytest.raises(PythonEnvironmentIdentityError, match=reason):
        native._pe_dependencies(bytes(image))


def test_pe_name_cannot_use_zero_fill_or_terminate_outside_raw_section() -> None:
    image = _pe_image()
    image[0x280:0x500] = b"x" * (0x500 - 0x280)
    with pytest.raises(PythonEnvironmentIdentityError, match="name is unterminated"):
        native._pe_dependencies(bytes(image))


@pytest.mark.parametrize("attributes", [2, 3, 0xFFFFFFFF])
def test_pe_rejects_unknown_delay_import_attributes(attributes: int) -> None:
    image = _pe_image(delay=True)
    struct.pack_into("<I", image, 0x240, attributes)
    with pytest.raises(PythonEnvironmentIdentityError, match="attributes are invalid"):
        native._pe_dependencies(bytes(image))


@pytest.mark.parametrize(
    "name", [b"libc.so", b"/loader/bound/lib.so", b"relative/lib.so"]
)
def test_elf_preserves_loader_name_including_path_qualified_needed(name: bytes) -> None:
    assert native._native_dependencies(
        bytes(_elf_image(name)), "linux", architecture="x86_64"
    ) == (native.NativeDependency(name.decode(), "required"),)


@pytest.mark.parametrize(
    "fmt,offset,value,reason",
    [
        ("<H", 54, 55, "program-header extent"),
        ("<Q", 120 + 32, 65, "dynamic table extent"),
        ("<q", 0x230, 1, "dynamic table is unterminated"),
        ("<q", 0x220, 5, "string table is duplicated"),
        ("<Q", 64 + 32, 0x300, "outside loaded segments"),
        ("<Q", 0x200 + 8, 0x400, "name is invalid"),
    ],
)
def test_elf_rejects_malformed_dynamic_metadata(
    fmt: str, offset: int, value: int, reason: str
) -> None:
    image = _elf_image()
    struct.pack_into(fmt, image, offset, value)
    with pytest.raises(PythonEnvironmentIdentityError, match=reason):
        native._elf_dependencies(bytes(image))


def test_elf_requires_name_termination_inside_string_table() -> None:
    image = _elf_image()
    image[0x308] = ord("x")
    with pytest.raises(PythonEnvironmentIdentityError, match="name is invalid"):
        native._elf_dependencies(bytes(image))


@pytest.mark.parametrize("endian", ["<", ">"])
@pytest.mark.parametrize("fat64", [False, True])
@pytest.mark.parametrize(
    "architecture,name", [("x86_64", "@rpath/x86.dylib"), ("arm64", "@rpath/arm.dylib")]
)
def test_macho_universal_selects_only_explicit_architecture(
    endian: str, fat64: bool, architecture: str, name: str
) -> None:
    assert native._native_dependencies(
        bytes(_fat_macho(endian=endian, fat64=fat64)),
        "macos",
        architecture=architecture,
    ) == (native.NativeDependency(name, "weak"),)


@pytest.mark.parametrize("arm64e_first", [False, True])
def test_macho_universal_uses_loaded_dyld_slice_identity(
    arm64e_first: bool,
) -> None:
    generic = _macho_image(cpu=0x0100000C, name=b"@rpath/generic.dylib")
    arm64e = _macho_image(cpu=0x0100000C, name=b"@rpath/arm64e.dylib")
    struct.pack_into("<I", arm64e, 8, 0x80000002)
    slices = (arm64e, generic) if arm64e_first else (generic, arm64e)

    assert native._native_dependencies(
        bytes(fat_macho(slices)),
        "macos",
        architecture="arm64",
        loaded_macho_identity=(0x0100000C, 0x80000002),
    ) == (native.NativeDependency("@rpath/arm64e.dylib", "weak"),)


def test_macos_closure_uses_injected_loader_snapshot_slice_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    generic = _macho_image(cpu=0x0100000C, name=b"@rpath/generic.dylib")
    arm64e = _macho_image(cpu=0x0100000C, name=b"@rpath/arm64e.dylib")
    struct.pack_into("<I", arm64e, 8, 0x80000002)
    executable = tmp_path / "Python"
    executable.write_bytes(fat_macho((generic, arm64e)))
    snapshot = _loader_snapshot(
        (executable,),
        {str(executable.resolve()): executable},
        macho_identities={executable.resolve(): (0x0100000C, 0x80000002)},
    )
    monkeypatch.setattr(native, "_loaded_native_module_snapshot", lambda _os: snapshot)

    closure = native._native_dependency_closure(
        {"base-executable": executable},
        operating_system="macos",
        architecture="arm64",
        policy=_NATIVE_DEPENDENCY_POLICIES["macos"],
        pool=_FileNodePool(),
    )

    assert closure["deferred_imports"] == [
        {
            "from": "native-component-0",
            "name": "@rpath/arm64e.dylib",
            "kind": "weak",
        }
    ]


@pytest.mark.parametrize(
    "fmt,offset,value,reason",
    [
        ("<I", 32 + 8, 23, "name is invalid"),
        ("<I", 16, 0, "count/extent disagree"),
        ("<I", 20, 0x1000, "load commands.*truncated"),
    ],
)
def test_macho_rejects_malformed_commands(
    fmt: str, offset: int, value: int, reason: str
) -> None:
    image = _macho_image()
    struct.pack_into(fmt, image, offset, value)
    with pytest.raises(PythonEnvironmentIdentityError, match=reason):
        native._macho_dependencies(bytes(image), architecture="x86_64")


def test_macho_rejects_truncated_dylib_command() -> None:
    image = _macho_image()
    struct.pack_into("<I", image, 20, 8)
    struct.pack_into("<I", image, 36, 8)
    with pytest.raises(
        PythonEnvironmentIdentityError, match="dylib command is truncated"
    ):
        native._macho_dependencies(bytes(image), architecture="x86_64")


def test_macho_rejects_truncated_rpath_command() -> None:
    image = macho_header(cpu=0x01000007, kind=6, command_count=1, command_bytes=8)
    struct.pack_into("<II", image, 32, 0x8000001C, 8)
    with pytest.raises(PythonEnvironmentIdentityError, match="LC_RPATH.*truncated"):
        native._macho_rpaths(bytes(image), architecture="x86_64")


def test_macho_universal_rejects_overlapping_slices() -> None:
    image = _fat_macho()
    struct.pack_into(">I", image, 28 + 8, 0x100)
    with pytest.raises(PythonEnvironmentIdentityError, match="slice extent"):
        native._macho_dependencies(bytes(image), architecture="arm64")


def test_macho_universal_rejects_slice_header_architecture_disagreement() -> None:
    image = _fat_macho()
    struct.pack_into("<I", image, 0x100 + 4, 0x0100000C)
    with pytest.raises(
        PythonEnvironmentIdentityError, match="does not match runtime architecture"
    ):
        native._macho_dependencies(bytes(image), architecture="x86_64")


def test_macho_universal_never_guesses_host_architecture() -> None:
    with pytest.raises(
        PythonEnvironmentIdentityError, match="explicit runtime architecture"
    ):
        native._macho_dependencies(bytes(_fat_macho()))


@pytest.mark.parametrize(
    "operating_system,image",
    [
        ("windows", bytes(_pe_image())),
        ("linux", bytes(_elf_image())),
        ("macos", bytes(_macho_image())),
    ],
)
def test_native_images_reject_wrong_runtime_architecture(
    operating_system: str, image: bytes
) -> None:
    with pytest.raises(
        PythonEnvironmentIdentityError, match="does not match runtime architecture"
    ):
        native._native_dependencies(image, operating_system, architecture="arm64")


@pytest.mark.parametrize(
    "operating_system", ["windows", "linux", "macos", "unsupported"]
)
@pytest.mark.parametrize("data", [b"", b"MZ", b"\x7fELF", b"\xcf\xfa\xed\xfe"])
def test_truncated_headers_raise_custody_error_not_struct_error(
    operating_system: str, data: bytes
) -> None:
    with pytest.raises(PythonEnvironmentIdentityError):
        native._native_dependencies(data, operating_system, architecture="x86_64")


@pytest.mark.parametrize(
    "operating_system,contract,valid",
    [
        ("windows", "windows-api-set:api-ms-win-core-file-l1-1-0.dll", True),
        ("windows", "windows-api-set:ext-ms-win-ntuser-window-l1-1-0.dll", True),
        ("windows", "windows-system-import:missing.dll", False),
        ("windows", "windows-api-set:api-ms-arbitrary.dll", False),
        ("windows", "windows-api-set:api-ms-../evil-l1-1-0.dll", False),
        ("linux", "linux-loader-image:linux-vdso.so.1", True),
        ("linux", "linux-loader-image:arbitrary.so", False),
        ("macos", "macos-dyld-cache-image:/usr/lib/libSystem.B.dylib", True),
        ("macos", "macos-dyld-cache-image:libSystem.B.dylib", False),
        ("macos", "macos-dyld-cache-image:/usr/lib/../libSystem.B.dylib", False),
        ("linux", "macos-dyld-cache-image:/usr/lib/libSystem.B.dylib", False),
        ("windows", ["unhashable"], False),
    ],
)
def test_native_contracts_are_explicit_and_platform_gated(
    operating_system: str, contract: object, valid: bool
) -> None:
    assert _native_contract_valid(contract, operating_system) is valid


@pytest.mark.parametrize(
    "name,closed",
    [
        (b"missing-vendor.dll", False),
        (b"api-ms-not-a-contract.dll", False),
        (b"api-ms-win-core-file-l1-1-0.dll", True),
    ],
)
def test_closure_never_turns_arbitrary_missing_dll_into_system_contract(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, name: bytes, closed: bool
) -> None:
    executable = tmp_path / "python.exe"
    executable.write_bytes(_pe_image(name))
    monkeypatch.setattr(
        native,
        "_loaded_native_module_snapshot",
        lambda _os: _loader_snapshot((executable,), {"python.exe": executable}),
    )
    if not closed:
        with pytest.raises(PythonEnvironmentIdentityError, match="cannot resolve"):
            native._native_dependency_closure(
                {"base-executable": executable},
                operating_system="windows",
                architecture="x86_64",
                policy=_NATIVE_DEPENDENCY_POLICIES["windows"],
                pool=_FileNodePool(),
            )
        return
    closure = native._native_dependency_closure(
        {"base-executable": executable},
        operating_system="windows",
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES["windows"],
        pool=_FileNodePool(),
    )
    contract = "windows-api-set:" + name.decode()
    assert closure["contracts"] == [contract]
    assert closure["edges"] == [{"from": "native-component-0", "to": contract}]
    assert closure["deferred_imports"] == []
    assert closure["observed_components"] == ["native-component-0"]
    assert closure["status"] == "closed"


def _empty_pe_image() -> bytearray:
    image = _pe_image()
    struct.pack_into("<II", image, 0x98 + 112 + 8, 0, 0)
    return image


@pytest.mark.parametrize("operating_system", ["windows", "linux", "macos"])
def test_configured_launcher_and_loaded_image_share_no_component_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operating_system: str
) -> None:
    launcher = tmp_path / "configured" / "python"
    executable = tmp_path / "loaded" / "python"
    for path in (launcher, executable):
        path.parent.mkdir()
    launcher.write_bytes(b"content-bound launcher, not a loaded image")
    if operating_system == "windows":
        image = _empty_pe_image()
    elif operating_system == "linux":
        image = _elf_image()
        struct.pack_into("<q", image, 0x200, 0)
    else:
        image = _macho_image()
    executable.write_bytes(image)
    snapshot = (
        _macos_loader_snapshot((executable,), {})
        if operating_system == "macos"
        else _loader_snapshot((executable,), {"python": executable})
    )
    monkeypatch.setattr(native, "_loaded_native_module_snapshot", lambda _os: snapshot)
    closure = native._native_dependency_closure(
        {"base-executable": launcher},
        operating_system=operating_system,
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES[operating_system],
        pool=_FileNodePool(),
    )
    assert len(closure["components"]) == 2
    assert len({row["node"] for row in closure["components"]}) == 2
    assert set(closure["root_components"]).isdisjoint(closure["observed_components"])
    assert closure["executable_component"] == closure["observed_components"][0]


@pytest.mark.parametrize("operating_system", ["windows", "linux", "macos"])
def test_unloaded_configured_root_cannot_provide_a_loaded_import(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, operating_system: str
) -> None:
    executable = tmp_path / "actual"
    launcher = tmp_path / "configured"
    launcher.write_bytes(b"launcher bytes are not a loaded provider")
    if operating_system == "windows":
        executable.write_bytes(_pe_image(b"configured"))
    elif operating_system == "linux":
        executable.write_bytes(_elf_image(b"configured"))
    else:
        executable.write_bytes(
            _macho_image(name=b"@executable_path/configured", command=0xC)
        )
    snapshot = (
        _macos_loader_snapshot((executable,), {})
        if operating_system == "macos"
        else _loader_snapshot((executable,), {"actual": executable})
    )
    monkeypatch.setattr(native, "_loaded_native_module_snapshot", lambda _os: snapshot)
    with pytest.raises(PythonEnvironmentIdentityError, match="cannot resolve"):
        native._native_dependency_closure(
            {"base-executable": launcher},
            operating_system=operating_system,
            architecture="x86_64",
            policy=_NATIVE_DEPENDENCY_POLICIES[operating_system],
            pool=_FileNodePool(),
        )


def test_main_image_designation_changes_persisted_closure_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    first, second = tmp_path / "first", tmp_path / "second"
    for path in (first, second):
        path.write_bytes(_macho_image())
    snapshot = _macos_loader_snapshot((first, second), {})
    monkeypatch.setattr(native, "_loaded_native_module_snapshot", lambda _os: snapshot)

    def capture():
        return native._native_dependency_closure(
            {"base-executable": first},
            operating_system="macos",
            architecture="x86_64",
            policy=_NATIVE_DEPENDENCY_POLICIES["macos"],
            pool=_FileNodePool(),
        )

    before = capture()
    snapshot = replace(snapshot, executable=second)
    after = capture()
    assert before["components"] == after["components"]
    assert before["edges"] == after["edges"]
    assert before["executable_component"] != after["executable_component"]
    assert before["closure_sha256"] != after["closure_sha256"]


def _capture_loaded_closure(
    monkeypatch: pytest.MonkeyPatch,
    executable: Path,
    loaded: tuple[Path, ...],
    *,
    operating_system: str = "windows",
    pool: _FileNodePool | None = None,
) -> dict[str, object]:
    monkeypatch.setattr(
        native,
        "_loaded_native_module_snapshot",
        lambda _os: _loader_snapshot(loaded, {path.name: path for path in loaded}),
    )
    return native._native_dependency_closure(
        {"base-executable": executable},
        operating_system=operating_system,
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES[operating_system],
        pool=pool if pool is not None else _FileNodePool(),
    )


def test_pe_same_name_required_and_delay_declarations_are_not_collapsed() -> None:
    image = _pe_image(b"SSPICLI.dll", delay=True)
    struct.pack_into("<II", image, 0x98 + 112 + 8, 0x1000, 40)
    struct.pack_into("<IIIII", image, 0x200, 0, 0, 0, 0x1080, 0)
    assert native._pe_dependencies(bytes(image)) == (
        native.NativeDependency("sspicli.dll", "delay"),
        native.NativeDependency("sspicli.dll", "required"),
    )


@pytest.mark.parametrize(
    "loaded_target,on_disk", [(False, False), (False, True), (True, True)]
)
def test_delay_declaration_never_fabricates_importer_specific_binding(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, loaded_target: bool, on_disk: bool
) -> None:
    executable = tmp_path / "python.exe"
    executable.write_bytes(_pe_image(b"sspicli.dll", delay=True))
    target = tmp_path / "sspicli.dll"
    # File existence is not loader evidence, and a loaded same-basename image
    # still cannot prove this importer's delay hook selected that image.
    if on_disk:
        target.write_bytes(_empty_pe_image())
    loaded = (executable, target) if loaded_target else (executable,)
    closure = _capture_loaded_closure(monkeypatch, executable, loaded)
    assert closure["edges"] == []
    assert closure["contracts"] == []
    assert closure["deferred_imports"] == [
        {"from": "native-component-0", "name": "sspicli.dll", "kind": "delay"}
    ]
    assert closure["root_components"] == ["native-component-0"]
    expected_observed = [f"native-component-{index}" for index in range(len(loaded))]
    assert closure["observed_components"] == expected_observed
    assert [row["filename"] for row in closure["components"]] == [
        path.name for path in loaded
    ]


def test_mixed_required_and_delay_closure_retains_both_facts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable = tmp_path / "python.exe"
    image = _pe_image(b"sspicli.dll", delay=True)
    struct.pack_into("<II", image, 0x98 + 112 + 8, 0x1000, 40)
    struct.pack_into("<IIIII", image, 0x200, 0, 0, 0, 0x1080, 0)
    executable.write_bytes(image)
    target = tmp_path / "sspicli.dll"
    target.write_bytes(_empty_pe_image())
    closure = _capture_loaded_closure(monkeypatch, executable, (executable, target))
    assert closure["edges"] == [
        {"from": "native-component-0", "to": "native-component-1"}
    ]
    assert closure["deferred_imports"] == [
        {"from": "native-component-0", "name": "sspicli.dll", "kind": "delay"}
    ]


def test_loaded_census_closes_unrelated_images_required_dependencies(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable = tmp_path / "python.exe"
    executable.write_bytes(_empty_pe_image())
    unrelated = tmp_path / "unrelated.dll"
    unrelated.write_bytes(_pe_image(b"missing-required.dll"))
    with pytest.raises(PythonEnvironmentIdentityError, match="cannot resolve"):
        _capture_loaded_closure(monkeypatch, executable, (executable, unrelated))


def test_deferred_census_order_is_numeric_and_independent_of_enumeration(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    paths = tuple(tmp_path / f"component-{index:02}.dll" for index in range(12))
    for index, path in enumerate(paths):
        path.write_bytes(_pe_image(f"future-{index:02}.dll".encode(), delay=True))
    closure = _capture_loaded_closure(monkeypatch, paths[0], tuple(reversed(paths)))
    assert closure["observed_components"] == [
        f"native-component-{index}" for index in range(12)
    ]
    assert closure["deferred_imports"] == [
        {
            "from": f"native-component-{index}",
            "name": f"future-{index:02}.dll",
            "kind": "delay",
        }
        for index in range(12)
    ]


def test_native_census_file_nodes_are_invariant_under_directory_order_relocation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    images = {
        "python.exe": bytes(_pe_image(b"system.dll")),
        "system.dll": bytes(_empty_pe_image()),
        "extra.dll": bytes(_pe_image(b"future.dll", delay=True)),
    }
    captures: list[dict[str, object]] = []
    host_orders: list[list[str]] = []
    for layout, directories in (
        ("first", ("a-runtime", "m-system", "z-extra")),
        ("relocated", ("z-runtime", "m-system", "a-extra")),
    ):
        paths: list[Path] = []
        for (filename, image), directory in zip(
            images.items(), directories, strict=True
        ):
            path = tmp_path / layout / directory / filename
            path.parent.mkdir(parents=True)
            path.write_bytes(image)
            paths.append(path)
        host_orders.append([path.name for path in sorted(paths)])
        pool = _FileNodePool()
        closure = _capture_loaded_closure(
            monkeypatch, paths[0], tuple(paths), pool=pool
        )
        pool.capture_context.verify()
        assert len(closure["components"]) == 3
        assert len(closure["observed_components"]) == 3
        captures.append({"closure": closure, "file_nodes": pool.nodes})
    # The fixture crosses filesystem sort boundaries, not merely a shared
    # prefix relocation that would accidentally preserve node encounter order.
    assert host_orders[0] != host_orders[1]
    assert captures[0] == captures[1]


@pytest.mark.parametrize(
    "command,kind",
    [
        (0xC, "required"),
        (0x80000018, "weak"),
        (0x8000001F, "reexport"),
        (0x20, "lazy"),
        (0x80000023, "upward"),
    ],
)
def test_macho_load_command_semantics_survive_parser_and_closure(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, command: int, kind: str
) -> None:
    name = "@rpath/optional.dylib"
    image = _macho_image(command=command, name=name.encode())
    assert native._macho_dependencies(bytes(image), architecture="x86_64") == (
        native.NativeDependency(name, kind),
    )
    executable = tmp_path / "python"
    executable.write_bytes(image)
    if kind not in {"weak", "lazy"}:
        with pytest.raises(
            PythonEnvironmentIdentityError,
            match="cannot attest importer-local.*inherited run-path stacks",
        ):
            _capture_loaded_closure(
                monkeypatch, executable, (executable,), operating_system="macos"
            )
        return
    closure = _capture_loaded_closure(
        monkeypatch, executable, (executable,), operating_system="macos"
    )
    assert closure["edges"] == []
    assert closure["contracts"] == []
    assert closure["deferred_imports"] == [
        {"from": "native-component-0", "name": name, "kind": kind}
    ]


def test_elf_needed_is_required_not_a_delayed_symbol_binding(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable = tmp_path / "python"
    executable.write_bytes(_elf_image(b"unloaded.so"))
    with pytest.raises(PythonEnvironmentIdentityError, match="cannot resolve"):
        _capture_loaded_closure(
            monkeypatch, executable, (executable,), operating_system="linux"
        )


@pytest.mark.parametrize("change", ["paths", "aliases", "contracts"])
def test_loaded_census_change_rejects_closure_publication(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str
) -> None:
    executable = tmp_path / "python.exe"
    executable.write_bytes(_empty_pe_image())
    extra = tmp_path / "extra.dll"
    extra.write_bytes(_empty_pe_image())
    snapshots = 0

    def inventory(_os):
        nonlocal snapshots
        snapshots += 1
        if snapshots == 1:
            return _loader_snapshot((executable,), {"python.exe": executable})
        paths = (executable, extra) if change == "paths" else (executable,)
        aliases = {"python.exe": executable}
        if change == "aliases":
            aliases["another-name.exe"] = executable
        contracts = (
            ("windows-api-set:api-ms-win-core-file-l1-1-0.dll",)
            if change == "contracts"
            else ()
        )
        return _loader_snapshot(paths, aliases, contracts)

    monkeypatch.setattr(native, "_loaded_native_module_snapshot", inventory)
    with pytest.raises(PythonEnvironmentIdentityError, match="changed"):
        native._native_dependency_closure(
            {"base-executable": executable},
            operating_system="windows",
            architecture="x86_64",
            policy=_NATIVE_DEPENDENCY_POLICIES["windows"],
            pool=_FileNodePool(),
        )
    assert snapshots == 2


@pytest.mark.parametrize("change", ["paths", "aliases", "contracts"])
def test_outer_capture_verification_rechecks_native_census_after_inventory(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, change: str
) -> None:
    executable = tmp_path / "python.exe"
    executable.write_bytes(_empty_pe_image())
    extra = tmp_path / "extra.dll"
    extra.write_bytes(_empty_pe_image())
    inventory_finished = False
    snapshots = 0

    def inventory(_os: str):
        nonlocal snapshots
        snapshots += 1
        paths = (executable,)
        aliases = {"python.exe": executable}
        contracts: tuple[str, ...] = ()
        if inventory_finished:
            if change == "paths":
                paths = (executable, extra)
            elif change == "aliases":
                aliases["another-name.exe"] = executable
            else:
                contracts = ("windows-api-set:api-ms-win-core-file-l1-1-0.dll",)
        return _loader_snapshot(paths, aliases, contracts)

    monkeypatch.setattr(native, "_loaded_native_module_snapshot", inventory)
    pool = _FileNodePool()
    closure = native._native_dependency_closure(
        {"base-executable": executable},
        operating_system="windows",
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES["windows"],
        pool=pool,
    )
    assert closure["status"] == "closed"
    assert snapshots == 2
    # Prove the registered fence accepts a stable outer publication before
    # modeling a loader change during subsequent runtime/environment inventory.
    pool.capture_context.verify()
    assert snapshots == 3
    inventory_finished = True
    with pytest.raises(PythonEnvironmentIdentityError, match="census changed"):
        pool.capture_context.verify()
    assert snapshots == 4


def test_outer_capture_verification_rechecks_dyld_slice_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable = tmp_path / "Python"
    executable.write_bytes(_macho_image())
    inventory_finished = False
    snapshots = 0

    def inventory(_os: str) -> LoadedNativeModuleSnapshot:
        nonlocal snapshots
        snapshots += 1
        subtype = 8 if inventory_finished else 3
        return _loader_snapshot(
            (executable,),
            {str(executable.resolve()): executable},
            macho_identities={executable.resolve(): (0x01000007, subtype)},
        )

    monkeypatch.setattr(native, "_loaded_native_module_snapshot", inventory)
    pool = _FileNodePool()
    closure = native._native_dependency_closure(
        {"base-executable": executable},
        operating_system="macos",
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES["macos"],
        pool=pool,
    )
    assert closure["status"] == "closed"
    assert snapshots == 2
    pool.capture_context.verify()
    assert snapshots == 3
    inventory_finished = True
    with pytest.raises(PythonEnvironmentIdentityError, match="census changed"):
        pool.capture_context.verify()
    assert snapshots == 4


@pytest.mark.parametrize(
    "operating_system,contract",
    [
        ("macos", "macos-dyld-cache-image:/usr/lib/libSystem.B.dylib"),
        ("linux", "linux-loader-image:linux-vdso.so.1"),
    ],
)
def test_observed_virtual_images_are_census_roots_not_invented_bindings(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    operating_system: str,
    contract: str,
) -> None:
    executable = tmp_path / "python"
    if operating_system == "macos":
        image = _macho_image()
    else:
        image = _elf_image()
        struct.pack_into("<q", image, 0x200, 0)
    executable.write_bytes(image)
    loader_snapshot = (
        _macos_loader_snapshot((executable,), {str(executable.resolve()): executable})
        if operating_system == "macos"
        else _loader_snapshot((executable,), {"python": executable})
    )
    monkeypatch.setattr(
        native,
        "_loaded_native_module_snapshot",
        lambda _os: LoadedNativeModuleSnapshot(
            executable=loader_snapshot.executable,
            paths=loader_snapshot.paths,
            aliases=loader_snapshot.aliases,
            contracts=(contract,),
            macho_identities=loader_snapshot.macho_identities,
        ),
    )
    closure = native._native_dependency_closure(
        {"base-executable": executable},
        operating_system=operating_system,
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES[operating_system],
        pool=_FileNodePool(),
    )
    assert closure["observed_contracts"] == [contract]
    assert closure["contracts"] == [contract]
    assert closure["edges"] == []
    assert len(closure["components"]) == 1
    assert closure["deferred_imports"] == (
        [
            {
                "from": "native-component-0",
                "name": "/usr/lib/libSystem.B.dylib",
                "kind": "weak",
            }
        ]
        if operating_system == "macos"
        else []
    )


@pytest.mark.parametrize(
    "dependency,closed",
    [
        ("/usr/lib/libSystem.B.dylib", True),
        ("/wrong/path/libSystem.B.dylib", False),
    ],
)
def test_macos_cached_image_contract_requires_exact_install_path(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    dependency: str,
    closed: bool,
) -> None:
    executable = tmp_path / "Python"
    executable.write_bytes(_macho_image(name=dependency.encode(), command=0xC))
    contract = "macos-dyld-cache-image:/usr/lib/libSystem.B.dylib"
    monkeypatch.setattr(
        native,
        "_loaded_native_module_snapshot",
        lambda _os: _macos_loader_snapshot(
            (executable,),
            {str(executable.resolve()): executable},
            (contract,),
        ),
    )

    if not closed:
        with pytest.raises(PythonEnvironmentIdentityError, match="cannot resolve"):
            native._native_dependency_closure(
                {"base-executable": executable},
                operating_system="macos",
                architecture="x86_64",
                policy=_NATIVE_DEPENDENCY_POLICIES["macos"],
                pool=_FileNodePool(),
            )
        return
    closure = native._native_dependency_closure(
        {"base-executable": executable},
        operating_system="macos",
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES["macos"],
        pool=_FileNodePool(),
    )
    assert closure["edges"] == [{"from": "native-component-0", "to": contract}]
    assert closure["contracts"] == [contract]


def test_macos_loaded_images_with_same_basename_remain_distinct_components(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    framework = tmp_path / "Frameworks/Python.framework/Versions/3.12/Python"
    framework.parent.mkdir(parents=True)
    framework.write_bytes(_macho_image())
    executable = tmp_path / "bin/Python"
    executable.parent.mkdir()
    executable.write_bytes(_macho_image())
    loaded = (framework, executable)
    aliases = {str(path.resolve()): path for path in loaded}
    monkeypatch.setattr(
        native,
        "_loaded_native_module_snapshot",
        lambda _os: _macos_loader_snapshot(loaded, aliases),
    )

    closure = native._native_dependency_closure(
        {"base-executable": executable},
        operating_system="macos",
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES["macos"],
        pool=_FileNodePool(),
    )

    assert [row["filename"] for row in closure["components"]] == ["Python", "Python"]
    assert len({row["node"] for row in closure["components"]}) == 2
    assert closure["observed_components"] == [
        "native-component-0",
        "native-component-1",
    ]


def test_macos_rpath_binding_uses_declared_loader_scope_not_basename(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    executable = tmp_path / "bin/Python"
    executable.parent.mkdir()
    framework = tmp_path / "Frameworks/Runtime.framework/Versions/3.12/Python"
    framework.parent.mkdir(parents=True)
    executable.write_bytes(
        _macho_image_with_rpaths(
            b"@rpath/Python",
            (b"@loader_path/../Frameworks/Runtime.framework/Versions/3.12",),
        )
    )
    framework.write_bytes(_macho_image())
    loaded = (executable, framework)
    aliases = {str(path.resolve()): path for path in loaded}
    monkeypatch.setattr(
        native,
        "_loaded_native_module_snapshot",
        lambda _os: _macos_loader_snapshot(loaded, aliases),
    )

    closure = native._native_dependency_closure(
        {"base-executable": executable},
        operating_system="macos",
        architecture="x86_64",
        policy=_NATIVE_DEPENDENCY_POLICIES["macos"],
        pool=_FileNodePool(),
    )

    assert [row["filename"] for row in closure["components"]] == ["Python", "Python"]
    assert closure["edges"] == [
        {"from": "native-component-0", "to": "native-component-1"}
    ]


def test_macos_rpath_binding_rejects_conflicting_declared_targets(
    tmp_path: Path,
) -> None:
    importer = tmp_path / "bin/Python"
    importer.parent.mkdir()
    importer.write_bytes(_macho_image())
    first = tmp_path / "first/Python"
    second = tmp_path / "second/Python"
    first.parent.mkdir()
    second.parent.mkdir()
    first.write_bytes(_macho_image())
    second.write_bytes(_macho_image())

    with pytest.raises(PythonEnvironmentIdentityError, match="ambiguous dyld binding"):
        native._resolve_macos_loaded_dependency(
            "@rpath/Python",
            importer=importer,
            rpaths=("@loader_path/../first", "@loader_path/../second"),
            executable=importer,
            known_paths_by_object=native._loaded_path_object_index(
                {first.resolve(), second.resolve()}
            ),
        )


@pytest.mark.parametrize("scope", ["@loader_path", "@executable_path"])
def test_macos_rpath_binding_accepts_bare_dyld_scope_tokens(
    tmp_path: Path, scope: str
) -> None:
    importer = tmp_path / "loader/importer"
    importer.parent.mkdir()
    importer.write_bytes(_macho_image())
    executable = tmp_path / "executable/Python"
    executable.parent.mkdir()
    executable.write_bytes(_macho_image())
    root = importer.parent if scope == "@loader_path" else executable.parent
    target = root / "Library"
    target.write_bytes(_macho_image())

    assert (
        native._resolve_macos_loaded_dependency(
            "@rpath/Library",
            importer=importer,
            rpaths=(scope,),
            executable=executable,
            known_paths_by_object=native._loaded_path_object_index({target.resolve()}),
        )
        == target.resolve()
    )


def test_macos_rpath_binding_rejects_unattested_inherited_scope(
    tmp_path: Path,
) -> None:
    importer = tmp_path / "importer"
    importer.write_bytes(_macho_image())

    with pytest.raises(PythonEnvironmentIdentityError, match="inherited run-path"):
        native._resolve_macos_loaded_dependency(
            "@rpath/Library",
            importer=importer,
            rpaths=(),
            executable=importer,
            known_paths_by_object={},
        )


def test_macos_loaded_object_index_selects_alias_deterministically(
    tmp_path: Path,
) -> None:
    first = tmp_path / "a-Python"
    second = tmp_path / "z-Python"
    first.write_bytes(_macho_image())
    try:
        second.hardlink_to(first)
    except OSError as exc:
        pytest.skip(f"hardlink creation unavailable: {exc}")

    forward = native._loaded_path_object_index([first, second])
    reverse = native._loaded_path_object_index([second, first])
    assert forward == reverse
    assert list(forward.values()) == [first]
