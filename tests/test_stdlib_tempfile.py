from __future__ import annotations

import builtins
import importlib.util
import os
import shutil
import sys
import tempfile
from pathlib import Path

import pytest

REPO_ROOT = Path(__file__).resolve().parents[1]
TEMPFILE_MODULE = REPO_ROOT / "src" / "molt" / "stdlib" / "tempfile.py"


@pytest.fixture
def molt_tempfile(monkeypatch):
    # CPython is the host filesystem boundary, not the policy under test.
    def probe(candidates):
        with monkeypatch.context() as context:
            context.setattr(tempfile, "_candidate_tempdir_list", lambda: candidates)
            try:
                return tempfile._get_default_tempdir()
            except FileNotFoundError:
                return None

    registry = {
        "molt_capabilities_require_operation": lambda operation: None,
        "molt_tempfile_gettempdir": probe,
        "molt_tempfile_mkdtemp": tempfile.mkdtemp,
        "molt_tempfile_mkstemp": tempfile.mkstemp,
        "molt_tempfile_cleanup": shutil.rmtree,
    }
    monkeypatch.setattr(builtins, "_molt_intrinsic_lookup", registry.get, raising=False)
    name = "molt_tempfile_test"
    spec = importlib.util.spec_from_file_location(name, TEMPFILE_MODULE)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, name, module)
    spec.loader.exec_module(module)
    return module


def test_default_selection_cache_and_public_override(
    molt_tempfile, monkeypatch, tmp_path
):
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()
    monkeypatch.setenv("TMPDIR", str(tmp_path / "missing"))
    monkeypatch.setenv("TEMP", str(first))
    monkeypatch.setenv("TMP", str(second))
    assert molt_tempfile.gettempdir() == str(first)
    assert molt_tempfile.tempdir == str(first)
    assert list(first.iterdir()) == []  # probe cannot leave a file behind
    monkeypatch.setenv("TEMP", str(second))
    assert molt_tempfile.gettempdirb() == os.fsencode(first)
    molt_tempfile.tempdir = None
    assert molt_tempfile.gettempdir() == str(second)
    molt_tempfile.tempdir = os.fsencode(first)
    assert molt_tempfile.gettempdir() == str(first)
    assert molt_tempfile.gettempdirb() == os.fsencode(first)


def test_relative_environment_candidate_uses_canonical_abspath(
    molt_tempfile, monkeypatch, tmp_path
):
    monkeypatch.chdir(tmp_path)
    (tmp_path / "selected").mkdir()
    monkeypatch.setenv("TMPDIR", "selected/../selected/.")
    assert molt_tempfile.gettempdir() == str(tmp_path / "selected")
    assert list((tmp_path / "selected").iterdir()) == []
    molt_tempfile.tempdir = None
    monkeypatch.setenv("TMPDIR", "./")
    assert molt_tempfile.gettempdir() == str(tmp_path)


def test_probe_failure_is_uncached_and_capabilities_precede_discovery(
    molt_tempfile, monkeypatch, tmp_path
):
    candidates = [str(tmp_path / "missing")]
    monkeypatch.setattr(molt_tempfile, "_candidate_tempdir_list", lambda: candidates)
    with pytest.raises(FileNotFoundError) as error:
        molt_tempfile.gettempdir()
    assert error.value.errno == 2
    assert repr(candidates) in str(error.value)
    assert molt_tempfile.tempdir is None
    calls = []

    def deny(operation):
        calls.append(operation)
        raise PermissionError("env.read, fs.read, fs.write")

    def unexpected_discovery():
        pytest.fail("directory discovery happened before capability admission")

    monkeypatch.setattr(molt_tempfile, "_MOLT_REQUIRE_OPERATION", deny)
    monkeypatch.setattr(molt_tempfile, "_candidate_tempdir_list", unexpected_discovery)
    with pytest.raises(PermissionError, match="env.read, fs.read, fs.write"):
        molt_tempfile.gettempdir()
    assert calls == ["tempfile.gettempdir"]
    molt_tempfile.tempdir = os.fsencode(tmp_path)
    assert molt_tempfile.gettempdir() == str(tmp_path)
    assert molt_tempfile.gettempdirb() == os.fsencode(tmp_path)
    assert calls == ["tempfile.gettempdir"]


def test_candidates_use_environment_platform_and_cwd(
    molt_tempfile, monkeypatch, tmp_path
):
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("TMPDIR", "relative")
    monkeypatch.setenv("TEMP", "")
    monkeypatch.setenv("TMP", "other")
    candidates = molt_tempfile._candidate_tempdir_list()
    assert candidates[:2] == ["relative", "other"]
    assert candidates[-1] == str(tmp_path)
    if os.name == "posix":
        assert candidates[2:-1] == ["/tmp", "/var/tmp", "/usr/tmp"]


@pytest.mark.parametrize("as_bytes", [False, True])
def test_all_omitted_directory_consumers_share_override(
    molt_tempfile, tmp_path, as_bytes
):
    root = os.fsencode(tmp_path) if as_bytes else str(tmp_path)
    molt_tempfile.tempdir = root
    directory = molt_tempfile.mkdtemp()
    assert type(directory) is type(root)
    assert os.path.dirname(directory) == root
    os.rmdir(directory)
    fd, path = molt_tempfile.mkstemp()
    try:
        assert type(path) is type(root)
        assert os.path.dirname(path) == root
    finally:
        os.close(fd)
        os.unlink(path)
    with molt_tempfile.TemporaryDirectory() as directory:
        assert type(directory) is type(root)
        assert os.path.dirname(directory) == root
    assert not os.path.exists(directory)
    with molt_tempfile.NamedTemporaryFile() as named:
        assert type(named.name) is type(root)
        assert os.path.dirname(named.name) == root
        assert named.file.name == named.name
        assert named.file.raw.name == named.name
        named.write(b"named")
    assert not os.path.exists(named.name)
    with molt_tempfile.TemporaryFile() as unnamed:
        unnamed.write(b"unnamed")
        unnamed.seek(0)
        assert unnamed.read() == b"unnamed"
    with molt_tempfile.SpooledTemporaryFile(max_size=1) as spooled:
        spooled.write(b"rolled")
        assert os.path.dirname(spooled.name) == root
        spooled.seek(0)
        assert spooled.read() == b"rolled"


def test_explicit_directory_pathlike_and_mixed_types(
    molt_tempfile, monkeypatch, tmp_path
):
    def unexpected_probe(_candidates):
        pytest.fail("explicit dir triggered default selection")

    monkeypatch.setattr(molt_tempfile, "_MOLT_TEMPFILE_GETTEMPDIR", unexpected_probe)
    path = molt_tempfile.mkdtemp(dir=tmp_path, prefix="prefix", suffix="suffix")
    assert Path(path).parent == tmp_path
    assert Path(path).name.startswith("prefix") and path.endswith("suffix")
    os.rmdir(path)
    for implementation in (tempfile, molt_tempfile):
        with pytest.raises(TypeError, match="Can't mix bytes and non-bytes"):
            implementation.mkdtemp(dir=tmp_path, prefix=b"bytes")


def test_named_file_keeps_secure_descriptor_when_path_is_replaced(
    molt_tempfile, monkeypatch, tmp_path
):
    original = tmp_path / "original"

    def create(suffix, prefix, dir, text):
        fd, path = tempfile.mkstemp(suffix, prefix, dir, text)
        os.rename(path, original)
        Path(path).write_bytes(b"replacement")
        return fd, path

    monkeypatch.setattr(molt_tempfile, "_MOLT_TEMPFILE_MKSTEMP", create)
    with molt_tempfile.NamedTemporaryFile(dir=tmp_path) as handle:
        handle.write(b"secure")
        handle.flush()
        assert original.read_bytes() == b"secure"
        assert Path(handle.name).read_bytes() == b"replacement"


def test_named_open_failure_retires_descriptor_and_path(
    molt_tempfile, monkeypatch, tmp_path
):
    created = []

    def create(suffix, prefix, dir, text):
        result = tempfile.mkstemp(suffix, prefix, dir, text)
        created.append(result)
        return result[0], result[1]

    monkeypatch.setattr(molt_tempfile, "_MOLT_TEMPFILE_MKSTEMP", create)
    with pytest.raises(LookupError):
        molt_tempfile.NamedTemporaryFile(
            mode="w", encoding="molt-invalid-codec", dir=tmp_path
        )
    assert len(created) == 1
    fd, path = created[0]
    with pytest.raises(OSError):
        os.fstat(fd)
    assert not os.path.exists(path)
    assert list(tmp_path.iterdir()) == []


def test_module_exports_expected_names(molt_tempfile):
    assert set(tempfile.__all__) <= set(molt_tempfile.__all__)
    assert molt_tempfile.gettempprefix() == tempfile.gettempprefix()
    assert molt_tempfile.gettempprefixb() == tempfile.gettempprefixb()


def test_relative_creation_paths_match_cpython(molt_tempfile, monkeypatch, tmp_path):
    monkeypatch.chdir(tmp_path)
    Path("relative").mkdir()
    for implementation in (tempfile, molt_tempfile):
        directory = implementation.mkdtemp(dir="relative/../relative/.")
        assert os.path.isabs(directory)
        assert Path(directory).parent == tmp_path / "relative"
        os.rmdir(directory)
        fd, path = implementation.mkstemp(dir="relative/../relative/.")
        try:
            assert os.path.isabs(path)
            assert Path(path).parent == tmp_path / "relative"
        finally:
            os.close(fd)
            os.unlink(path)


def test_default_probe_is_published_once_under_concurrent_getters(
    molt_tempfile, monkeypatch, tmp_path
):
    import concurrent.futures
    import threading

    entered = threading.Event()
    release = threading.Event()
    calls = []

    def probe(_candidates):
        calls.append("probe")
        entered.set()
        assert release.wait(5)
        return str(tmp_path)

    monkeypatch.setattr(molt_tempfile, "_MOLT_TEMPFILE_GETTEMPDIR", probe)
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
        first = executor.submit(molt_tempfile.gettempdir)
        assert entered.wait(5)
        second = executor.submit(molt_tempfile.gettempdirb)
        release.set()
        assert first.result() == str(tmp_path)
        assert second.result() == os.fsencode(tmp_path)
    assert calls == ["probe"]


def test_named_text_and_unbuffered_file_names_share_secure_path(
    molt_tempfile, tmp_path
):
    for mode, buffering in (("w+", -1), ("w+b", 0)):
        with molt_tempfile.NamedTemporaryFile(
            mode=mode, buffering=buffering, dir=tmp_path
        ) as handle:
            raw = getattr(handle.file, "buffer", handle.file)
            raw = getattr(raw, "raw", raw)
            assert raw.name == handle.name
            assert handle.file.name == handle.name


def test_mkstemp_text_callback_precedes_relative_directory_resolution(
    molt_tempfile, monkeypatch, tmp_path
):
    first = tmp_path / "first"
    second = tmp_path / "second"
    first.mkdir()
    second.mkdir()

    class SelectSecond:
        def __bool__(self):
            os.chdir(second)
            return False

    for implementation in (tempfile, molt_tempfile):
        monkeypatch.chdir(first)
        fd, path = implementation.mkstemp(dir=".", text=SelectSecond())
        try:
            assert os.path.dirname(path) == str(second)
        finally:
            os.close(fd)
            os.unlink(path)
