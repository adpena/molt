"""Loader census contracts without live native image probes or compilation."""

from __future__ import annotations

import ctypes
import io
import os
from pathlib import Path
import struct
from types import SimpleNamespace

import pytest

from molt import python_native_locations as locations
from molt.python_identity_common import PythonEnvironmentIdentityError


def _mock_dyld(monkeypatch: pytest.MonkeyPatch, images: tuple[Path, ...]) -> None:
    headers = [
        ctypes.create_string_buffer(struct.pack("<III", 0xFEEDFACF, 0x01000007, 3))
        for _path in images
    ]
    process = SimpleNamespace(
        _dyld_image_count=lambda: len(images),
        _dyld_get_image_name=lambda index: os.fsencode(images[index]),
        _dyld_get_image_header=lambda index: ctypes.addressof(headers[index]),
    )
    monkeypatch.setattr(ctypes, "CDLL", lambda _name: process)


def test_macos_census_owns_actual_image_not_configured_launcher(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    launcher = tmp_path / "bin" / "python3.12"
    executable = tmp_path / "Python.app" / "Contents" / "MacOS" / "Python"
    library = tmp_path / "Python.framework" / "Python"
    for path in (launcher, executable, library):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(b"independent file generation")
    monkeypatch.setattr(
        locations,
        "sys",
        SimpleNamespace(
            platform="darwin", executable=str(launcher), _base_executable=str(launcher)
        ),
    )
    _mock_dyld(monkeypatch, (executable, library))

    snapshot = locations._loaded_native_module_snapshot("macos")

    assert snapshot.executable == executable.resolve()
    assert set(snapshot.paths) == {executable.resolve(), library.resolve()}
    assert launcher.resolve() not in snapshot.macho_identities
    assert snapshot.macho_identities == {
        executable.resolve(): (0x01000007, 3),
        library.resolve(): (0x01000007, 3),
    }


def test_macos_census_does_not_invent_missing_main_image(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(locations, "sys", SimpleNamespace(platform="darwin"))
    _mock_dyld(monkeypatch, ())
    with pytest.raises(PythonEnvironmentIdentityError, match="no executable image"):
        locations._loaded_native_module_snapshot("macos")


def test_snapshot_rejects_executable_outside_census(tmp_path: Path) -> None:
    with pytest.raises(PythonEnvironmentIdentityError, match="absent.*census"):
        locations.LoadedNativeModuleSnapshot(
            executable=tmp_path / "launcher",
            paths=(tmp_path / "actual",),
            aliases={},
            contracts=(),
            macho_identities={},
        )


def _mock_linux(
    monkeypatch: pytest.MonkeyPatch,
    *,
    images: tuple[tuple[Path | None, int], ...],
    mapped_file: Path,
    kernel_executable: Path,
    auxiliary_headers: int = 0x1040,
    mapped_inode: int | None = None,
) -> None:
    """Exercise real callback/mapping attribution without querying the host OS."""
    metadata = mapped_file.stat()
    inode = metadata.st_ino if mapped_inode is None else mapped_inode
    maps_text = f"1000-2000 r--p 00000000 01:00 {inode} /synthetic/main\n"
    original_open = Path.open
    original_resolve = Path.resolve

    def open_path(path, *args, **kwargs):
        if path == Path("/proc/self/maps"):
            return io.StringIO(maps_text)
        return original_open(path, *args, **kwargs)

    def resolve_path(path, *args, **kwargs):
        if path == Path("/proc/self/exe"):
            return original_resolve(kernel_executable, *args, **kwargs)
        return original_resolve(path, *args, **kwargs)

    def make_device(major: int, minor: int) -> int:
        assert (major, minor) == (1, 0)
        return metadata.st_dev

    def getauxval(key: int) -> int:
        assert key == 3
        return auxiliary_headers

    def iterate(callback, _data):
        info_type = callback._argtypes_[0]._type_
        for path, headers in images:
            info = info_type(0, os.fsencode(path) if path else b"", headers, 1)
            result = callback(ctypes.pointer(info), ctypes.sizeof(info), None)
            if result:
                return result
        return 0

    process = SimpleNamespace(getauxval=getauxval, dl_iterate_phdr=iterate)
    monkeypatch.setattr(ctypes, "CDLL", lambda _name: process)
    monkeypatch.setattr(locations, "sys", SimpleNamespace(platform="linux"))
    monkeypatch.setattr(Path, "open", open_path)
    monkeypatch.setattr(Path, "resolve", resolve_path)
    # Windows runs these same synthetic tests without a native os.makedev.
    monkeypatch.setattr(os, "makedev", make_device, raising=False)


@pytest.mark.parametrize("named_main", [False, True])
@pytest.mark.parametrize("explicit_interpreter", [False, True])
def test_linux_main_image_matches_program_headers_and_kernel_backing(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    named_main: bool,
    explicit_interpreter: bool,
) -> None:
    main = tmp_path / "python"
    interpreter = tmp_path / "ld.so"
    launcher = tmp_path / "configured-python"
    for path in (main, interpreter, launcher):
        path.write_bytes(path.name.encode())
    _mock_linux(
        monkeypatch,
        images=((main if named_main else None, 0x1040), (interpreter, 0x3040)),
        mapped_file=main,
        kernel_executable=interpreter if explicit_interpreter else main,
    )
    if explicit_interpreter and not named_main:
        with pytest.raises(
            PythonEnvironmentIdentityError, match="disagrees with its kernel mapping"
        ):
            locations._loaded_native_module_snapshot("linux")
        return
    snapshot = locations._loaded_native_module_snapshot("linux")
    assert snapshot.executable == main.resolve()
    assert set(snapshot.paths) == {main.resolve(), interpreter.resolve()}
    assert launcher.resolve() not in snapshot.paths


@pytest.mark.parametrize(
    "auxiliary_headers,reason",
    [
        (0, "no AT_PHDR"),
        (0x4040, "no main image matching AT_PHDR"),
        (0x3040, "does not identify the first main ELF image"),
    ],
)
def test_linux_main_image_rejects_missing_or_nonmain_auxiliary_identity(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    auxiliary_headers: int,
    reason: str,
) -> None:
    main = tmp_path / "python"
    interpreter = tmp_path / "ld.so"
    for path in (main, interpreter):
        path.write_bytes(b"image")
    _mock_linux(
        monkeypatch,
        images=((main, 0x1040), (interpreter, 0x3040)),
        mapped_file=main,
        kernel_executable=interpreter,
        auxiliary_headers=auxiliary_headers,
    )
    with pytest.raises(PythonEnvironmentIdentityError, match=reason):
        locations._loaded_native_module_snapshot("linux")


def test_linux_main_image_rejects_anonymous_program_header_mapping(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    main = tmp_path / "python"
    main.write_bytes(b"image")
    _mock_linux(
        monkeypatch,
        images=((main, 0x1040),),
        mapped_file=main,
        kernel_executable=main,
        mapped_inode=0,
    )
    with pytest.raises(PythonEnvironmentIdentityError, match="not file-backed"):
        locations._loaded_native_module_snapshot("linux")


def test_linux_main_image_requires_auxiliary_vector_authority(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(locations, "sys", SimpleNamespace(platform="linux"))
    monkeypatch.setattr(ctypes, "CDLL", lambda _name: SimpleNamespace())
    with pytest.raises(PythonEnvironmentIdentityError, match="without getauxval"):
        locations._loaded_native_module_snapshot("linux")


def test_linux_named_main_rejects_file_unrelated_to_mapped_image(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    mapped = tmp_path / "mapped-python"
    replacement = tmp_path / "replacement-python"
    for path in (mapped, replacement):
        path.write_bytes(b"image")
    _mock_linux(
        monkeypatch,
        images=((replacement, 0x1040),),
        mapped_file=mapped,
        kernel_executable=mapped,
    )
    with pytest.raises(
        PythonEnvironmentIdentityError, match="disagrees with its kernel mapping"
    ):
        locations._loaded_native_module_snapshot("linux")
