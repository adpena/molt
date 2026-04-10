def test_generate_worker_produces_valid_js(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.bundle.read"], tmp_quota_mb=32)
    content = output.read_text()
    assert "fetch" in content
    assert "WebAssembly" in content


def test_generate_worker_contains_tmpfs(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.tmp.read", "fs.tmp.write"], tmp_quota_mb=64)
    content = output.read_text()
    assert "class TmpFs" in content
    assert "TMP_QUOTA_MB = 64" in content
    assert "ENOSPC: quota exceeded" in content


def test_generate_worker_contains_host_imports(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.bundle.read"], tmp_quota_mb=16)
    content = output.read_text()
    assert "createHostImports" in content
    assert "molt_vfs_read" in content
    assert "molt_vfs_write" in content
    assert "molt_log_host" in content


def test_generate_worker_contains_fetch_handler(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, ["http.fetch"])
    content = output.read_text()
    assert "async fetch(request, env, ctx)" in content
    assert "export default" in content
    assert "molt_main" in content


def test_generate_worker_contains_wasi_shim(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, [])
    content = output.read_text()
    assert "buildWasiShim" in content
    assert "wasi_snapshot_preview1" in content
    assert "fd_write" in content
    assert "fd_read" in content
    assert "proc_exit" in content
    assert "clock_time_get" in content


def test_generate_worker_capabilities_substituted(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, ["fs.bundle.read", "http.fetch"], tmp_quota_mb=32)
    content = output.read_text()
    assert '"fs.bundle.read"' in content
    assert '"http.fetch"' in content
    assert "{{CAPABILITIES}}" not in content
    assert "{{TMP_QUOTA_MB}}" not in content
    assert "{{WASM_FILENAME}}" not in content


def test_generate_worker_custom_wasm_filename(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, [], wasm_filename="custom.wasm")
    content = output.read_text()
    assert "custom.wasm" in content
    assert "worker_linked.wasm" not in content


def test_generate_worker_no_scaffold_warning(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, [])
    content = output.read_text()
    assert "SCAFFOLD" not in content
    assert "NOT PRODUCTION READY" not in content


def test_generate_worker_stdio_capture(tmp_path):
    from tools.generate_worker import generate_worker
    output = tmp_path / "worker.js"
    generate_worker(output, [])
    content = output.read_text()
    assert "class StdioCapture" in content
    assert "writeStdout" in content
    assert "writeStderr" in content
    assert "readStdin" in content


def test_generate_split_worker_contains_vfs_adapter_import() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert 'import "./molt_vfs_browser.js";' in content
    assert "new globalThis.MoltVfs()" in content


def test_generate_split_worker_contains_vfs_host_imports() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=32,
    )

    assert "molt_vfs_read" in content
    assert "molt_vfs_write" in content
    assert "molt_vfs_exists" in content
    assert "molt_vfs_unlink" in content


def test_generate_split_worker_uses_worker_env_for_static_assets() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "async fetch(request, env, ctx)" in content
    assert "env.__STATIC_CONTENT" in content


def test_generate_split_worker_defines_utf8_decoder_for_vfs_paths() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "const utf8Decoder = new TextDecoder();" in content
    assert "utf8Decoder.decode" in content
    assert "UTF8_DECODER" not in content


def test_generate_split_worker_defines_wasi_vfs_errno_and_preopen_state() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "const WASI_ERRNO_NOENT = 44;" in content
    assert "const WASI_ERRNO_NOSYS = 52;" in content
    assert "const WASI_OFLAGS_CREAT = 1;" in content
    assert "const wasiFiles = new Map();" in content
    assert "const wasiPreopens = [" in content
    assert "const preopenByFd = (fdNum) =>" in content


def test_generate_split_worker_replaces_path_stubs_with_vfs_backed_wasi_ops() -> None:
    from molt.cli import _generate_split_worker_js

    content = _generate_split_worker_js(
        shared_memory_initial_pages=8,
        shared_table_initial=16,
        shared_table_base=None,
    )

    assert "path_open() { return 44; }" not in content
    assert "path_filestat_get() { return 44; }" not in content
    assert "fd_prestat_get(fd, prestatPtr)" in content
    assert "fd_prestat_dir_name(fd, pathPtr, pathLen)" in content
    assert "path_open(fd, _dirflags, pathPtr, pathLen, oflags, _rightsBase, _rightsInheriting, _fdflags, openedFdPtr)" in content
    assert "path_filestat_get(fd, _flags, pathPtr, pathLen, bufPtr)" in content
