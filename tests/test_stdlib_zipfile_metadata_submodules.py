from __future__ import annotations

import builtins
import importlib
import importlib.util
import posixpath
import re
import sys
import types
import uuid
import zlib
from pathlib import Path

import pytest


REPO_ROOT = Path(__file__).resolve().parents[1]
STDLIB_ROOT = REPO_ROOT / "src" / "molt" / "stdlib"
ZIPFILE_DIR = STDLIB_ROOT / "zipfile"
ZIPFILE_INIT = ZIPFILE_DIR / "__init__.py"
DIAGNOSE_MODULE = STDLIB_ROOT / "importlib" / "metadata" / "diagnose.py"
INTRINSICS_MODULE = STDLIB_ROOT / "_intrinsics.py"


def _ensure_intrinsics_module() -> None:
    if "_intrinsics" in sys.modules:
        return
    spec = importlib.util.spec_from_file_location("_intrinsics", INTRINSICS_MODULE)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules["_intrinsics"] = module
    spec.loader.exec_module(module)


def _install_intrinsics() -> None:
    _ensure_intrinsics_module()
    registry = getattr(builtins, "_molt_intrinsics", None)
    if not isinstance(registry, dict):
        registry = {}
        setattr(builtins, "_molt_intrinsics", registry)

    def _deflate_raw(data: bytes, level: int | None):
        compressor = zlib.compressobj(
            level if level is not None else zlib.Z_DEFAULT_COMPRESSION,
            zlib.DEFLATED,
            -15,
        )
        return compressor.compress(data) + compressor.flush()

    def _inflate_raw(data: bytes):
        return zlib.decompress(data, -15)

    def _zipfile_path_implied_dirs(names):
        names = list(names)
        parents: list[str] = []
        for name in names:
            path = name.rstrip("/")
            ancestry: list[str] = []
            while path.rstrip("/"):
                ancestry.append(path)
                head, _tail = posixpath.split(path)
                path = head
            parents.extend(ancestry[1:])
        seen: set[str] = set()
        out: list[str] = []
        names_set = set(names)
        for parent in parents:
            entry = f"{parent}/"
            if entry in names_set or entry in seen:
                continue
            seen.add(entry)
            out.append(entry)
        return out

    def _zipfile_path_resolve_dir(name: str, names):
        names_set = set(names)
        dirname = f"{name}/"
        if name not in names_set and dirname in names_set:
            return dirname
        return name

    def _zipfile_path_is_child(path_at: str, parent_at: str) -> bool:
        return posixpath.dirname(path_at.rstrip("/")) == parent_at.rstrip("/")

    def _zipfile_path_translate_glob(pattern: str, seps: str, py313_plus: bool) -> str:
        if py313_plus:
            import zipfile._path.glob as host_glob

            return host_glob.Translator(seps=seps).translate(pattern)
        import zipfile._path.glob as host_glob

        if hasattr(host_glob, "translate"):
            return host_glob.translate(pattern)
        return host_glob.Translator(seps="/").translate(pattern)

    def _zipfile_normalize_member_path(member: str) -> str | None:
        normalized = posixpath.normpath(member.replace("\\", "/")).lstrip("/\\")
        if normalized in {"", "."}:
            return None
        if normalized == ".." or normalized.startswith("../"):
            return None
        return normalized

    def _zipfile_parse_central_directory(data: bytes):
        zip64_limit = 0xFFFFFFFF
        zip64_count_limit = 0xFFFF
        zip64_extra_id = 0x0001
        central_sig = b"PK\x01\x02"
        eocd_sig = b"PK\x05\x06"
        zip64_eocd_sig = b"PK\x06\x06"
        zip64_locator_sig = b"PK\x06\x07"

        def read_u16(blob: bytes, offset: int) -> int:
            return int.from_bytes(blob[offset : offset + 2], "little", signed=False)

        def read_u32(blob: bytes, offset: int) -> int:
            return int.from_bytes(blob[offset : offset + 4], "little", signed=False)

        def read_u64(blob: bytes, offset: int) -> int:
            return int.from_bytes(blob[offset : offset + 8], "little", signed=False)

        def find_eocd(blob: bytes) -> int:
            max_comment = 65535
            start = len(blob) - (22 + max_comment)
            if start < 0:
                start = 0
            return blob.rfind(eocd_sig, start)

        def parse_zip64_extra(
            extra: bytes,
            comp_size: int,
            uncomp_size: int,
            local_offset: int,
        ) -> tuple[int, int, int]:
            pos = 0
            while pos + 4 <= len(extra):
                header_id = read_u16(extra, pos)
                data_size = read_u16(extra, pos + 2)
                pos += 4
                if pos + data_size > len(extra):
                    break
                if header_id == zip64_extra_id:
                    cursor = pos
                    if uncomp_size == zip64_limit:
                        if cursor + 8 > pos + data_size:
                            raise ValueError("zip64 extra missing size")
                        uncomp_size = read_u64(extra, cursor)
                        cursor += 8
                    if comp_size == zip64_limit:
                        if cursor + 8 > pos + data_size:
                            raise ValueError("zip64 extra missing comp size")
                        comp_size = read_u64(extra, cursor)
                        cursor += 8
                    if local_offset == zip64_limit:
                        if cursor + 8 > pos + data_size:
                            raise ValueError("zip64 extra missing offset")
                        local_offset = read_u64(extra, cursor)
                    return comp_size, uncomp_size, local_offset
                pos += data_size
            raise ValueError("zip64 extra missing")

        if len(data) < 22:
            raise ValueError("file is not a zip file")
        eocd_offset = find_eocd(data)
        if eocd_offset < 0:
            raise ValueError("end of central directory not found")
        cd_size = read_u32(data, eocd_offset + 12)
        cd_offset = read_u32(data, eocd_offset + 16)
        total_entries = read_u16(data, eocd_offset + 10)
        if (
            total_entries == zip64_count_limit
            or cd_size == zip64_limit
            or cd_offset == zip64_limit
        ):
            locator_offset = eocd_offset - 20
            if locator_offset < 0:
                raise ValueError("zip64 locator missing")
            if data[locator_offset : locator_offset + 4] != zip64_locator_sig:
                raise ValueError("zip64 locator missing")
            zip64_eocd_offset = read_u64(data, locator_offset + 8)
            if data[zip64_eocd_offset : zip64_eocd_offset + 4] != zip64_eocd_sig:
                raise ValueError("zip64 eocd missing")
            cd_size = read_u64(data, zip64_eocd_offset + 40)
            cd_offset = read_u64(data, zip64_eocd_offset + 48)

        pos = cd_offset
        end = cd_offset + cd_size
        index: dict[str, tuple[int, int, int, int, int]] = {}
        while pos + 46 <= end:
            if data[pos : pos + 4] != central_sig:
                break
            comp_method = read_u16(data, pos + 10)
            comp_size = read_u32(data, pos + 20)
            uncomp_size = read_u32(data, pos + 24)
            name_len = read_u16(data, pos + 28)
            extra_len = read_u16(data, pos + 30)
            comment_len = read_u16(data, pos + 32)
            local_offset = read_u32(data, pos + 42)
            name_start = pos + 46
            name_bytes = data[name_start : name_start + name_len]
            name = name_bytes.decode("utf-8", errors="replace")
            extra_start = name_start + name_len
            extra = data[extra_start : extra_start + extra_len]
            if (
                comp_size == zip64_limit
                or uncomp_size == zip64_limit
                or local_offset == zip64_limit
            ):
                comp_size, uncomp_size, local_offset = parse_zip64_extra(
                    extra,
                    comp_size,
                    uncomp_size,
                    local_offset,
                )
            index[name] = (local_offset, comp_size, comp_method, name_len, uncomp_size)
            pos = name_start + name_len + extra_len + comment_len
        return index

    def _zipfile_build_zip64_extra(
        size: int, comp_size: int, offset: int | None
    ) -> bytes:
        def u16(value: int) -> bytes:
            return int(value).to_bytes(2, "little", signed=False)

        def u64(value: int) -> bytes:
            return int(value).to_bytes(8, "little", signed=False)

        data = bytearray()
        data.extend(u64(size))
        data.extend(u64(comp_size))
        if offset is not None:
            data.extend(u64(offset))
        return u16(0x0001) + u16(len(data)) + data

    registry.update(
        {
            "molt_capabilities_has": lambda _name: True,
            "molt_capabilities_trusted": lambda: True,
            "molt_capabilities_require": lambda _name: None,
            "molt_zipfile_crc32": lambda data: zlib.crc32(data) & 0xFFFFFFFF,
            "molt_zipfile_parse_central_directory": _zipfile_parse_central_directory,
            "molt_zipfile_build_zip64_extra": _zipfile_build_zip64_extra,
            "molt_deflate_raw": _deflate_raw,
            "molt_inflate_raw": _inflate_raw,
            "molt_zipfile_path_implied_dirs": _zipfile_path_implied_dirs,
            "molt_zipfile_path_resolve_dir": _zipfile_path_resolve_dir,
            "molt_zipfile_path_is_child": _zipfile_path_is_child,
            "molt_zipfile_path_translate_glob": _zipfile_path_translate_glob,
            "molt_zipfile_normalize_member_path": _zipfile_normalize_member_path,
        }
    )


def _load_zipfile_package(name: str):
    _install_intrinsics()
    for key in list(sys.modules):
        if key == name or key.startswith(f"{name}."):
            sys.modules.pop(key, None)

    spec = importlib.util.spec_from_file_location(
        name,
        ZIPFILE_INIT,
        submodule_search_locations=[str(ZIPFILE_DIR)],
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def test_zipfile_path_iterdir_and_read_text(tmp_path: Path) -> None:
    package_name = f"molt_zipfile_{uuid.uuid4().hex}"
    zipfile_mod = _load_zipfile_package(package_name)

    archive = tmp_path / "archive.zip"
    with zipfile_mod.ZipFile(
        str(archive), "w", compression=zipfile_mod.ZIP_DEFLATED
    ) as zf:
        zf.writestr("root.txt", "root")
        zf.writestr("nested/leaf.txt", "leaf")

    with zipfile_mod.ZipFile(str(archive), "r") as zf:
        root = zipfile_mod.Path(zf)
        children = sorted(path.at for path in root.iterdir())
        assert children == ["nested/", "root.txt"]
        assert (root / "nested" / "leaf.txt").read_text(encoding="utf-8") == "leaf"


def test_zipfile_path_glob_version_surface() -> None:
    package_name = f"molt_zipfile_{uuid.uuid4().hex}"
    _load_zipfile_package(package_name)
    glob_mod = importlib.import_module(f"{package_name}._path.glob")

    if sys.version_info >= (3, 13):
        translator = glob_mod.Translator(seps="/")
        pattern = translator.translate("*.txt")
        assert re.compile(pattern).fullmatch("file.txt") is not None
        assert not hasattr(glob_mod, "translate")
    else:
        pattern = glob_mod.translate("*.txt")
        assert re.compile(pattern).fullmatch("file.txt") is not None
        assert not hasattr(glob_mod, "Translator")


def test_zipfile_main_module_create_list_test_extract(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    package_name = f"molt_zipfile_{uuid.uuid4().hex}"
    _load_zipfile_package(package_name)
    main_mod = importlib.import_module(f"{package_name}.__main__")

    source = tmp_path / "payload.txt"
    source.write_text("payload-data", encoding="utf-8")
    archive = tmp_path / "bundle.zip"

    main_mod.main(["-c", str(archive), str(source)])
    assert archive.exists()

    main_mod.main(["-l", str(archive)])
    listed = capsys.readouterr().out
    assert "payload.txt" in listed

    main_mod.main(["-t", str(archive)])
    tested = capsys.readouterr().out
    assert "Done testing" in tested

    output_dir = tmp_path / "extract"
    main_mod.main(["-e", str(archive), str(output_dir)])
    assert (output_dir / "payload.txt").read_text(encoding="utf-8") == "payload-data"


def test_importlib_metadata_diagnose_inspect_and_run(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    _install_intrinsics()

    package_name = f"molt_metadata_{uuid.uuid4().hex}"
    module_name = f"{package_name}.diagnose"
    for key in list(sys.modules):
        if key == package_name or key.startswith(f"{package_name}."):
            sys.modules.pop(key, None)

    package = types.ModuleType(package_name)
    package.__path__ = []

    class _FakeDistribution:
        seen: list[tuple[str, ...]] = []

        @classmethod
        def discover(cls, path=None):
            entries = tuple(path or [])
            cls.seen.append(entries)
            if entries and entries[0] == "empty":
                return []
            return [
                types.SimpleNamespace(name="alpha"),
                types.SimpleNamespace(name="beta"),
            ]

    package.Distribution = _FakeDistribution
    sys.modules[package_name] = package

    spec = importlib.util.spec_from_file_location(module_name, DIAGNOSE_MODULE)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[module_name] = module
    spec.loader.exec_module(module)

    module.inspect("empty")
    inspect_empty = capsys.readouterr().out
    assert inspect_empty.strip() == "Inspecting empty"

    module.inspect("packages")
    inspect_output = capsys.readouterr().out
    assert "Inspecting packages" in inspect_output
    assert "Found 2 packages: alpha, beta" in inspect_output

    monkeypatch.setattr(module.sys, "path", ["left", "right"])
    module.run()
    run_output = capsys.readouterr().out
    assert "Inspecting left" in run_output
    assert "Inspecting right" in run_output
    assert _FakeDistribution.seen[-2:] == [("left",), ("right",)]
