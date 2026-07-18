from __future__ import annotations

import json
import os
import shutil
import subprocess
import time
from pathlib import Path

import pytest

from molt.browser_asset_closure import (
    BROWSER_HOST_ENTRY_ASSETS,
    BROWSER_WASM_ENTRY_ASSETS,
    NODE_RUNNER_ENTRY_ASSETS,
    browser_asset_manifest_key,
    browser_asset_manifest_keys,
    canonical_text_bytes,
    canonical_wasm_loader_asset_bytes,
    wasm_loader_asset_closure,
    wasm_loader_asset_scope_paths,
)
from tools.gen_browser_asset_graph import (
    AssetSource,
    generate,
    generated_output_is_current,
    scan_sources,
)


ROOT = Path(__file__).resolve().parents[1]


def _scan(source: str, *, source_type: str = "module") -> list[dict[str, object]]:
    results, _telemetry = scan_sources(
        (
            AssetSource(
                "fixture.js",
                "browser",
                source_type,
                "test",
                source.encode(),
            ),
        )
    )
    return results["fixture.js"]


def _requests(source: str, *, source_type: str = "module") -> set[tuple[str, str]]:
    return {
        (str(fact["kind"]), str(fact["request"]))
        for fact in _scan(source, source_type=source_type)
    }


def test_canonical_browser_and_node_entries_close_over_declared_roles() -> None:
    assert wasm_loader_asset_closure(
        ROOT / "wasm",
        BROWSER_WASM_ENTRY_ASSETS,
    ) == (
        "browser_embed.js",
        "browser_gpu_dispatch.js",
        "browser_gpu_worker.js",
        "browser_host.js",
        "browser_target_features.js",
        "callable_table_abi_generated.js",
        "loader_bridge.js",
        "molt_vfs_browser.js",
        "target_feature_constants.generated.js",
    )
    assert wasm_loader_asset_closure(ROOT / "wasm", NODE_RUNNER_ENTRY_ASSETS) == (
        "callable_table_abi_generated.js",
        "loader_bridge.js",
        "run_wasm.js",
        "target_feature_manifest.json",
        "wasm_abi_generated.json",
    )


def test_scanner_uses_one_batch_process(monkeypatch: pytest.MonkeyPatch) -> None:
    calls = 0
    real_run = subprocess.run

    def counted_run(
        *args: object, **kwargs: object
    ) -> subprocess.CompletedProcess[bytes]:
        nonlocal calls
        calls += 1
        return real_run(*args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(subprocess, "run", counted_run)
    rows = tuple(
        AssetSource(f"fixture-{idx}.js", "browser", "module", "test", b"export {};\n")
        for idx in range(32)
    )
    results, telemetry = scan_sources(rows)

    assert calls == 1
    assert len(results) == 32
    assert telemetry["source_count"] == 32


def test_generated_output_check_accepts_git_platform_line_endings(
    tmp_path: Path,
) -> None:
    output = tmp_path / "graph.json"
    generated = b'{\n  "schema_version": 2\n}\n'
    output.write_bytes(generated.replace(b"\n", b"\r\n"))
    assert generated_output_is_current(output, generated)

    output.write_bytes(output.read_bytes().replace(b"2", b"3"))
    assert not generated_output_is_current(output, generated)


def test_canonical_text_bytes_make_generator_dependency_hashes_host_invariant(
    tmp_path: Path,
) -> None:
    dependency = tmp_path / "scanner.mjs"
    dependency.write_bytes(b"export const scan = true;\n")
    lf = canonical_text_bytes(dependency)

    dependency.write_bytes(b"export const scan = true;\r\n")
    crlf = canonical_text_bytes(dependency)

    assert crlf == lf == b"export const scan = true;\n"


def test_scanner_waits_for_chunked_nonblocking_stdin() -> None:
    node = shutil.which("node")
    if node is None:
        pytest.skip("node is required")
    scanner_root = ROOT / "tools" / "browser_asset_graph"
    payload = json.dumps(
        {
            "sources": [
                {
                    "id": "fixture",
                    "path": "fixture.js",
                    "role": "browser",
                    "source": "new Worker('./worker.js');\n",
                    "source_type": "module",
                }
            ]
        },
        separators=(",", ":"),
    ).encode()
    read_fd, write_fd = os.pipe()
    os.set_blocking(read_fd, False)
    process = subprocess.Popen(
        [node, str(scanner_root / "scan.mjs")],
        stdin=read_fd,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=scanner_root,
    )
    os.close(read_fd)
    try:
        # Give the child a chance to observe an empty nonblocking pipe. The old
        # readFileSync(0) authority failed here with EAGAIN on hosted runners.
        time.sleep(0.05)
        with os.fdopen(write_fd, "wb", buffering=0) as stream:
            for offset in range(0, len(payload), 7):
                stream.write(payload[offset : offset + 7])
                time.sleep(0.001)
        write_fd = -1
        stdout, stderr = process.communicate(timeout=10)
    finally:
        if write_fd >= 0:
            os.close(write_fd)
        if process.poll() is None:
            process.kill()
            process.wait()

    assert process.returncode == 0, stderr.decode(errors="replace")
    result = json.loads(stdout)
    assert result["telemetry"]["source_bytes"] == len(payload)
    assert result["results"][0]["references"] == [
        {
            "column": 1,
            "kind": "worker",
            "line": 1,
            "request": "./worker.js",
        }
    ]


@pytest.mark.parametrize(
    "source",
    [
        "new Worker('./bad.js'); const Worker = class {};\n",
        "{ new Worker('./bad.js'); let Worker; }\n",
        "function f() { new Worker('./bad.js'); var Worker; }\n",
        "function f({Worker}) { new Worker('./bad.js'); }\n",
        "try {} catch (Worker) { new Worker('./bad.js'); }\n",
        "import Worker from './worker-factory.js'; new Worker('./bad.js');\n",
        "const {require} = hooks; require('./bad.js');\n",
    ],
)
def test_lexical_tdz_hoisting_and_patterns_shadow_loader_globals(source: str) -> None:
    assert not any(
        kind in {"require", "worker", "shared-worker"}
        for kind, _request in _requests(source)
    )


def test_block_local_shadow_does_not_hide_loader_in_sibling_scope() -> None:
    source = "{ const Worker = class {}; }\nnew Worker(`./real.js`);\n"
    assert _requests(source) == {("worker", "./real.js")}


def test_url_shadow_makes_real_worker_operand_nonstatic_and_fails_closed() -> None:
    with pytest.raises(ValueError, match="is not statically resolvable"):
        _scan(
            "new Worker(new URL('./bad.js', import.meta.url)); const URL = class {};\n"
        )


def test_static_loader_forms_include_literals_templates_urls_and_modules() -> None:
    source = """
import './static.js';
export { value } from './exported.js';
import(new URL(`./dynamic.js`, import.meta.url));
new Worker('./worker.js');
new SharedWorker(new URL('./shared.js', import.meta.url));
importScripts('./classic-a.js', `./classic-b.js`);
fetch(new URL('./data.json', import.meta.url));
"""
    assert _requests(source) == {
        ("dynamic-import", "./dynamic.js"),
        ("fetch", "./data.json"),
        ("import-scripts", "./classic-a.js"),
        ("import-scripts", "./classic-b.js"),
        ("module", "./exported.js"),
        ("module", "./static.js"),
        ("shared-worker", "./shared.js"),
        ("worker", "./worker.js"),
    }


def test_node_require_shadow_from_destructuring_prevents_false_worker_edge() -> None:
    source = """
const { Worker } = require('worker_threads');
const fs = require('fs');
new Worker(__filename);
"""
    assert _requests(source, source_type="script") == {
        ("require", "fs"),
        ("require", "worker_threads"),
    }


def test_nonliteral_loader_reference_fails_closed() -> None:
    with pytest.raises(ValueError, match="is not statically resolvable"):
        _scan("new Worker(path);\n")


def test_generated_graph_hash_and_role_drift_fail_closed(tmp_path: Path) -> None:
    graph_path = ROOT / "wasm" / "browser_asset_graph.generated.json"
    payload = json.loads(graph_path.read_text(encoding="utf-8"))
    for asset in payload["assets"]:
        target = tmp_path / asset
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(ROOT / "wasm" / asset, target)
    (tmp_path / graph_path.name).write_text(json.dumps(payload), encoding="utf-8")
    (tmp_path / "browser_host.js").write_text("export {};\n", encoding="utf-8")
    with pytest.raises(ValueError, match="hash drift"):
        wasm_loader_asset_closure(tmp_path)

    shutil.copy2(ROOT / "wasm" / "browser_host.js", tmp_path / "browser_host.js")
    payload["assets"]["loader_bridge.js"]["role"] = "node"
    (tmp_path / graph_path.name).write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="role violation"):
        wasm_loader_asset_closure(tmp_path)


def test_asset_hash_and_generator_are_line_ending_invariant(tmp_path: Path) -> None:
    wasm_root = tmp_path / "wasm"
    wasm_root.mkdir()
    manifest = tmp_path / "browser_asset_graph.toml"
    manifest.write_text(
        """schema_version = 2
[entry_groups.browser-wasm]
role = "browser"
assets = ["entry.js"]
[[asset]]
path = "entry.js"
role = "browser"
source_type = "module"
authority = "test"
""",
        encoding="utf-8",
    )
    source = wasm_root / "entry.js"
    source.write_bytes(b"export const value = 1;\nexport default value;\n")
    lf_graph = generate(manifest, wasm_root)

    source.write_bytes(b"export const value = 1;\r\nexport default value;\r\n")
    crlf_graph = generate(manifest, wasm_root)

    assert crlf_graph == lf_graph
    assert canonical_wasm_loader_asset_bytes(source) == (
        b"export const value = 1;\nexport default value;\n"
    )


def test_browser_asset_manifest_keys_and_proof_scopes_share_authority() -> None:
    assert browser_asset_manifest_key("target_feature_constants.generated.js") == (
        "target_feature_constants"
    )
    scopes = wasm_loader_asset_scope_paths(BROWSER_HOST_ENTRY_ASSETS)
    assert "wasm/browser_gpu_worker.js" in scopes
    assert scopes == tuple(sorted(scopes))

    with pytest.raises(ValueError, match="collide at manifest key"):
        browser_asset_manifest_keys(("a/runtime.js", "b/runtime.js"))


def test_wasm_run_matrix_stages_the_canonical_browser_host_closure(
    tmp_path: Path,
) -> None:
    from tools import wasm_run_matrix

    staged = wasm_run_matrix._stage_browser_static_assets(tmp_path)

    assert staged == wasm_loader_asset_closure(
        ROOT / "wasm",
        BROWSER_HOST_ENTRY_ASSETS,
    )
    for name in staged:
        staged_path = tmp_path.joinpath(*Path(name).parts)
        assert staged_path.is_file()
        assert staged_path.read_bytes() == canonical_wasm_loader_asset_bytes(
            ROOT.joinpath("wasm", *Path(name).parts)
        )
