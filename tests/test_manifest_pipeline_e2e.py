"""End-to-end test: molt build --capability-manifest pipeline.

Tests the full workflow: manifest -> build -> run -> enforcement.
The build proof always enters through the current checkout's CLI module.
"""

from __future__ import annotations

import gzip
import json
import os
import shutil
import sys
import tempfile
import time
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process
from tests import process_guard_common

sys.path.insert(0, "src")

PROJECT_ROOT = Path(__file__).parent.parent


def _write_manifest(path: Path) -> None:
    """Write a strict capability manifest for testing."""
    path.write_text(
        "[manifest]\n"
        'version = "2.0"\n'
        "[capabilities]\n"
        'allow = ["time.wall"]\n'
        "[resources]\n"
        'max_memory = "16MB"\n'
        'max_duration = "3s"\n'
        "max_recursion_depth = 100\n"
        "[audit]\n"
        "enabled = true\n"
        'sink = "stderr"\n',
        encoding="utf-8",
    )


# ---------------------------------------------------------------------------
# Test 1: Manifest parses and produces correct env vars
# ---------------------------------------------------------------------------


def test_manifest_parses_and_converts():
    """Manifest loads and produces correct env vars."""
    from molt.capability_manifest import load_manifest

    with tempfile.NamedTemporaryFile(suffix=".toml", mode="w", delete=False) as f:
        _write_manifest(Path(f.name))
        path = f.name
    try:
        m = load_manifest(path)
        env = m.to_env_vars()
        assert env["MOLT_RESOURCE_MAX_MEMORY"] == str(16 * 1024 * 1024), (
            f"expected {16 * 1024 * 1024}, got {env.get('MOLT_RESOURCE_MAX_MEMORY')}"
        )
        assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "3000", (
            f"expected 3000, got {env.get('MOLT_RESOURCE_MAX_DURATION_MS')}"
        )
        assert env["MOLT_RESOURCE_MAX_RECURSION_DEPTH"] == "100", (
            f"expected 100, got {env.get('MOLT_RESOURCE_MAX_RECURSION_DEPTH')}"
        )
        assert env["MOLT_AUDIT_ENABLED"] == "1"
        assert env["MOLT_AUDIT_SINK"] == "stderr"
        # Capabilities should include time.wall
        assert "time.wall" in env["MOLT_CAPABILITIES"]
    finally:
        os.unlink(path)


# ---------------------------------------------------------------------------
# Test 2: All resource fields map to env vars
# ---------------------------------------------------------------------------


def test_manifest_env_vars_are_complete():
    """All resource fields map to env vars."""
    from molt.capability_manifest import CapabilityManifest, ResourceLimits

    m = CapabilityManifest(
        resources=ResourceLimits(
            max_memory=1048576,
            max_duration=5.0,
            max_allocations=1000,
            max_recursion_depth=50,
        )
    )
    env = m.to_env_vars()
    expected_keys = [
        "MOLT_CAPABILITIES",
        "MOLT_RESOURCE_MAX_MEMORY",
        "MOLT_RESOURCE_MAX_DURATION_MS",
        "MOLT_RESOURCE_MAX_ALLOCATIONS",
        "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
    ]
    for key in expected_keys:
        assert key in env, f"missing env var: {key}"


# ---------------------------------------------------------------------------
# Test 3: Effective capabilities respect deny list
# ---------------------------------------------------------------------------


def test_manifest_deny_removes_capabilities():
    """Denied capabilities are excluded from effective set."""
    from molt.capability_manifest import CapabilityManifest

    m = CapabilityManifest(
        allow=["net", "time.wall"],
        deny=["websocket.connect"],
    )
    effective = m.effective_capabilities()
    assert "time.wall" in effective
    assert "net" not in effective
    assert "net.connect" in effective
    # The profile expands to exact operation grants; deny removes one grant
    # without preserving the old broad bypass token.
    assert "websocket.connect" not in effective


# ---------------------------------------------------------------------------
# Test 6: Env var round-trip (duration and memory edge cases)
# ---------------------------------------------------------------------------


def test_env_var_round_trip_edge_cases():
    """Verify env var values for edge-case resource limits."""
    from molt.capability_manifest import (
        CapabilityManifest,
        ManifestError,
        ResourceLimits,
    )

    m = CapabilityManifest(resources=ResourceLimits(max_duration=0.0, max_memory=0))
    with pytest.raises(ManifestError, match="max_memory must be positive"):
        m.to_env_vars()

    # Fractional seconds
    m2 = CapabilityManifest(resources=ResourceLimits(max_duration=1.5))
    env2 = m2.to_env_vars()
    assert env2["MOLT_RESOURCE_MAX_DURATION_MS"] == "1500"


# ---------------------------------------------------------------------------
# Test 7: Audit disabled produces no audit env vars
# ---------------------------------------------------------------------------


def test_audit_disabled_no_env_vars():
    """When audit is disabled, no MOLT_AUDIT_* env vars are emitted."""
    from molt.capability_manifest import CapabilityManifest, AuditConfig

    m = CapabilityManifest(audit=AuditConfig(enabled=False))
    env = m.to_env_vars()
    assert "MOLT_AUDIT_ENABLED" not in env
    assert "MOLT_AUDIT_SINK" not in env


# ---------------------------------------------------------------------------
# Test 8: Manifest with strict limits parses size/duration strings
# ---------------------------------------------------------------------------


def test_parse_size_and_duration_from_manifest():
    """Size and duration strings in TOML are parsed to numeric values."""
    from molt.capability_manifest import load_manifest

    toml_content = (
        "[manifest]\n"
        'version = "2.0"\n'
        "[resources]\n"
        'max_memory = "256KB"\n'
        'max_duration = "500ms"\n'
    )
    with tempfile.NamedTemporaryFile(suffix=".toml", mode="w", delete=False) as f:
        f.write(toml_content)
        path = f.name
    try:
        m = load_manifest(path)
        assert m.resources.max_memory == 256 * 1024
        assert m.resources.max_duration == 0.5
        env = m.to_env_vars()
        assert env["MOLT_RESOURCE_MAX_MEMORY"] == str(256 * 1024)
        assert env["MOLT_RESOURCE_MAX_DURATION_MS"] == "500"
    finally:
        os.unlink(path)


# ---------------------------------------------------------------------------
# Test 9: Full molt build pipeline (skipped if molt unavailable)
# ---------------------------------------------------------------------------


@pytest.mark.slow
def test_molt_build_with_manifest():
    """Build and execute through the current checkout's complete CLI graph."""
    with process_guard_common.guarded_temporary_directory(
        prefix="molt-manifest-build-"
    ) as tmpdir:
        manifest = tmpdir / "test.capabilities.toml"
        _write_manifest(manifest)

        src = tmpdir / "hello.py"
        src.write_text('print("hello from molt")\n', encoding="utf-8")

        result = run_native_test_process(
            [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                "--profile",
                "dev",
                "--capability-manifest",
                str(manifest),
                str(src),
                "--out-dir",
                str(tmpdir),
                "--json",
            ],
            capture_output=True,
            text=True,
            timeout=600,
            cwd=str(PROJECT_ROOT),
        )
        assert result.returncode == 0, (
            f"stdout:\n{result.stdout}\n\nstderr:\n{result.stderr}"
        )
        payload = json.loads(result.stdout)
        output = Path(payload["data"]["output"])
        assert output.is_file()

        from molt.capability_manifest import load_manifest

        loaded = load_manifest(str(manifest))
        env = {**os.environ, **loaded.to_env_vars()}
        run_result = run_native_test_process(
            [str(output)],
            capture_output=True,
            text=True,
            timeout=10,
            env=env,
        )
        assert run_result.returncode == 0, run_result.stderr
        assert run_result.stdout.strip() == "hello from molt"


@pytest.mark.slow
def test_native_warm_code_cache_keeps_current_capability_policy() -> None:
    """Shared code must not carry the previous final binary's authority."""
    with process_guard_common.guarded_temporary_directory(
        prefix="molt-native-cache-policy-"
    ) as tmpdir:
        payload_path = tmpdir / "payload.txt"
        payload_path.write_text("cache-policy-payload\n", encoding="utf-8")
        source = tmpdir / "read_payload.py"
        source.write_text(
            "try:\n"
            f"    with open({str(payload_path)!r}, 'r') as stream:\n"
            "        print(stream.read(), end='')\n"
            "except PermissionError:\n"
            "    print('PermissionError')\n",
            encoding="utf-8",
        )
        source_bytes = source.read_bytes()
        manifest = tmpdir / "policy.toml"
        cache_dir = tmpdir / "code-cache"
        output_dir = tmpdir / "output"
        empty_path = tmpdir / "no-python-on-path"
        empty_path.mkdir()
        observations = []
        output = None
        for index, allowed in enumerate((True, False, True)):
            manifest.write_text(
                '[manifest]\nversion = "2.0"\n[capabilities]\n'
                + ('allow = ["fs.read"]\n' if allowed else "allow = []\n"),
                encoding="utf-8",
            )
            started = time.monotonic()
            built = run_native_test_process(
                [
                    sys.executable,
                    "-m",
                    "molt.cli",
                    "build",
                    "--profile",
                    "dev",
                    "--cache",
                    "--cache-dir",
                    str(cache_dir),
                    "--capability-manifest",
                    str(manifest),
                    str(source),
                    "--out-dir",
                    str(output_dir),
                    "--json",
                ],
                cwd=PROJECT_ROOT,
                timeout=1200,
            )
            elapsed = time.monotonic() - started
            assert built.returncode == 0, (
                f"build {index}: stdout:\n{built.stdout}\nstderr:\n{built.stderr}"
            )
            data = json.loads(built.stdout)["data"]
            current_output = Path(data["output"])
            assert current_output.is_file()
            if output is None:
                output = current_output
            assert current_output == output
            assert source.read_bytes() == source_bytes
            cache = data["cache"]
            assert cache["enabled"] is True
            assert isinstance(cache["key"], str) and cache["key"]
            if observations:
                assert cache["key"] == observations[0]["cache_key"]
                assert cache["hit"] is True, data
            assert data["capabilities"] == (["fs.read"] if allowed else [])
            observation = {
                "allow_fs_read": allowed,
                "elapsed_seconds": elapsed,
                "cache_key": cache["key"],
                "cache_hit": cache["hit"],
                "policy_digest": data["capability_policy_digest"],
            }
            observations.append(observation)
            print(
                "native-cache-policy-build " + json.dumps(observation, sort_keys=True)
            )

            # The emitted stub, not ambient grants or Python, owns this run.
            # This is a checkout-built executable control, not installed-package
            # or general standalone-distribution acceptance.
            environment = dict(os.environ)
            environment.pop("PYTHONPATH", None)
            environment["PATH"] = str(empty_path)
            environment["MOLT_CAPABILITY_TIER"] = "none" if allowed else "full"
            environment["MOLT_CAPABILITIES"] = "" if allowed else "fs.read"
            executed = run_native_test_process(
                [str(current_output)],
                cwd=tmpdir,
                env=environment,
                timeout=30,
            )
            assert executed.returncode == 0, (
                f"run {index}: stdout:\n{executed.stdout}\nstderr:\n{executed.stderr}"
            )
            assert executed.stdout == (
                "cache-policy-payload\n" if allowed else "PermissionError\n"
            )
            assert executed.stderr == ""
            assert payload_path.read_text(encoding="utf-8") == "cache-policy-payload\n"
        assert observations[0]["policy_digest"] != observations[1]["policy_digest"]
        assert observations[0]["policy_digest"] == observations[2]["policy_digest"]


@pytest.mark.slow
def test_native_stdlib_objects_reuse_across_code_slot_layouts() -> None:
    """New dense frame and module IDs must not invalidate shared machine code."""
    from molt.backend_environment import environment_keys
    from molt.cli.native_symbol_inspection import _native_archive_global_symbol_facts
    from molt.memory_guard_paths import harness_guard_artifact_dir
    from molt.toolchain_identity import stable_file_content_identity
    from molt.cli.function_records import decode_lowering_record
    from molt.cli.source_record_store import STORE_RELATIVE_PATH
    import sqlite3
    from tests.executable_test_support import native_executable_name

    # Keep reviewable evidence beside the existing guard receipts. The queue
    # owns this root; the larger, disposable code cache stays in guarded scratch.
    evidence_root = harness_guard_artifact_dir(PROJECT_ROOT)
    evidence_root.mkdir(parents=True, exist_ok=True)
    evidence = Path(tempfile.mkdtemp(prefix="native-stdlib-reuse-", dir=evidence_root))
    print(f"native-stdlib-reuse-evidence {evidence}", flush=True)
    common_source = (
        "import sys\n"
        "import warnings\n"
        "def application_owner():\n"
        "    return sys._getframe().f_code.co_name\n"
        "print(application_owner())\n"
        "print(warnings.formatwarning.__code__.co_name)\n"
        "print(warnings.formatwarning('hi', UserWarning, 'owner.py', 7, line='').strip())\n"
    )
    sources = (
        common_source,
        "import shlex\n" + common_source + "print(shlex.split(\"a 'b c'\"))\n",
    )
    environment = dict(os.environ)
    for name in environment_keys("diagnostic"):
        environment.pop(name, None)
    environment.update(MOLT_BACKEND_DAEMON="0", MOLT_KEEP_SYMBOLS="1")
    application_keys: list[str] = []
    archive_keys: list[str] = []
    layouts: list[dict[str, int]] = []
    module_layouts: list[dict[str, int]] = []
    members: list[dict[str, frozenset[str]]] = []
    inventories: list[dict[str, dict[str, object]]] = []

    with process_guard_common.guarded_temporary_directory(
        prefix="molt-stdlib-reuse-"
    ) as scratch:
        cache = scratch / "code-cache"
        environment["MOLT_CACHE"] = str(cache)
        empty_path = scratch / "no-python-on-path"
        empty_path.mkdir()
        known_archives: set[Path] = set()
        for index, source_text in enumerate(sources):
            case = evidence / str(index)
            case.mkdir()
            source = case / "program.py"
            source.write_text(source_text, encoding="utf-8")
            output = case / native_executable_name("program")
            ir_path = case / "ir.json"
            diagnostics_path = case / "diagnostics.json"
            command = [
                sys.executable,
                "-m",
                "molt.cli",
                "build",
                "--profile",
                "dev",
                "--stdlib-profile",
                "full",
                "--cache",
                "--cache-dir",
                str(cache),
                "--backend",
                "cranelift",
                "--verbose",
                "--emit-ir",
                str(ir_path),
                "--diagnostics-file",
                str(diagnostics_path),
                "--out-dir",
                str(case),
                "--output",
                str(output),
                str(source),
            ]
            started = time.monotonic()
            built = run_native_test_process(
                command,
                cwd=PROJECT_ROOT,
                env=environment,
                timeout=1200,
            )
            elapsed = time.monotonic() - started
            (case / "build.stdout").write_text(built.stdout, encoding="utf-8")
            (case / "build.stderr").write_text(built.stderr, encoding="utf-8")
            assert built.returncode == 0, (
                f"build {index}: stdout:\n{built.stdout}\nstderr:\n{built.stderr}"
            )
            assert output.is_file()
            # Inspect the real producer records from this same build. Object
            # reuse must not turn a missing source selection into completeness.
            source_store = cache / STORE_RELATIVE_PATH
            assert source_store.is_file()
            with sqlite3.connect(source_store.as_uri() + "?mode=ro", uri=True) as db:
                producer_rows = db.execute(
                    "SELECT payload FROM records WHERE kind = 'module-lowering'"
                ).fetchall()
            assert producer_rows
            selections = []
            for (encoded,) in producer_rows:
                producer = json.loads(decode_lowering_record(encoded).metadata)[
                    "result"
                ]
                selection = producer["body_selection"]
                assert set(selection) == {"kind", "roots"}
                assert selection["kind"] in {"complete_source", "native_support"}
                assert bool(selection["roots"]) is (
                    selection["kind"] == "native_support"
                )
                selections.append(selection)
            (case / "producer-selections.json").write_text(
                json.dumps(selections, sort_keys=True), encoding="utf-8"
            )
            cache_lines = [
                line for line in built.stdout.splitlines() if line.startswith("Cache: ")
            ]
            assert len(cache_lines) == 1 and cache_lines[0].startswith(
                "Cache: miss ("
            ), built.stdout
            application_keys.append(cache_lines[0].split("(", 1)[1].split(")", 1)[0])
            ir = json.loads(ir_path.read_text(encoding="utf-8"))
            layouts.append(ir["code_slots"])
            module_layouts.append(
                {
                    row["name"]: slot
                    for slot, row in enumerate(ir["module_registry"]["rows"])
                }
            )
            # Preserve full compiler output without keeping an uncompressed IR
            # duplicate. No compiler dump/verification switch disables reuse.
            with (
                ir_path.open("rb") as raw,
                gzip.open(case / "ir.json.gz", "wb") as compressed,
            ):
                shutil.copyfileobj(raw, compressed)
            ir_path.unlink()
            del ir

            reuse_lines = [
                line.split("stdlib object reuse: ", 1)[1]
                for line in built.stderr.splitlines()
                if "stdlib object reuse: " in line
            ]
            assert len(reuse_lines) == 1, built.stderr
            counts = [
                int(field.split()[0])
                for field in reuse_lines[0].split(";", 1)[0].split(",")
            ]
            assert len(counts) == 4, reuse_lines
            hits, misses, reused_bytes, emitted_bytes = counts
            assert misses > 0 and emitted_bytes > 0
            entries = {}
            for entry in sorted(
                (cache / "native-stdlib-objects-v1").glob("*/object.entry")
            ):
                entries[entry.parent.name] = {
                    **stable_file_content_identity(entry, label="stdlib cache object"),
                    "mtime_ns": entry.stat().st_mtime_ns,
                }
            assert entries
            if inventories:
                previous = inventories[-1]
                assert all(entries.get(key) == value for key, value in previous.items())
                assert len(entries.keys() - previous.keys()) == misses
                assert hits == len(previous), (
                    "adding shlex must reuse every existing stdlib object; "
                    f"observed {hits} hits for {len(previous)} unchanged units"
                )
                assert reused_bytes > 0
            else:
                assert hits == 0 and reused_bytes == 0
                assert len(entries) == misses
            inventories.append(entries)

            archives = set((cache / "shared-stdlib").glob("*/stdlib.a"))
            new_archives = archives - known_archives
            assert len(new_archives) == 1, sorted(map(str, new_archives))
            archive = next(iter(new_archives))
            archive_keys.append(archive.parent.name)
            known_archives = archives
            facts = _native_archive_global_symbol_facts(archive)
            assert facts.members
            members.append(
                {
                    member.identity.sha256: member.symbols.defined_functions
                    for member in facts.members
                }
            )
            oracle = run_native_test_process(
                [sys.executable, str(source)], cwd=case, timeout=30
            )
            guest_environment = dict(environment)
            guest_environment.pop("PYTHONPATH", None)
            guest_environment["PATH"] = str(empty_path)
            executed = run_native_test_process(
                [str(output)], cwd=case, env=guest_environment, timeout=30
            )
            for label, result in (("cpython", oracle), ("molt", executed)):
                assert result.returncode == 0, (
                    f"{label}: {result.stdout}\n{result.stderr}"
                )
            assert executed.stdout == oracle.stdout
            assert executed.stderr == oracle.stderr == ""
            assert source.read_text(encoding="utf-8") == source_text
            diagnostics = json.loads(diagnostics_path.read_text(encoding="utf-8"))
            print(
                f"native-stdlib-reuse-build case={index} elapsed_seconds={elapsed:.3f} "
                f"diagnostics_seconds={diagnostics['total_sec']} slots={len(layouts[-1])} "
                f"application_key={application_keys[-1]} stdlib_key={archive_keys[-1]} "
                f"{reuse_lines[0]} stdout={executed.stdout!r}",
                flush=True,
            )

        moved = {
            name
            for name in layouts[0].keys() & layouts[1].keys()
            if layouts[0][name] != layouts[1][name]
        }
        assert moved, (
            "the second application must change actual dense owner assignments"
        )
        moved_modules = {
            name
            for name in module_layouts[0].keys() & module_layouts[1].keys()
            if module_layouts[0][name] != module_layouts[1][name]
        }
        assert moved_modules, "the added module must change existing dense module IDs"
        reused_members = members[0].keys() & members[1].keys()
        reused_moved = sorted(
            moved & set().union(*(members[1][key] for key in reused_members))
        )
        assert reused_moved, (
            "an identical emitted stdlib object must define a moved code owner"
        )
        assert archive_keys[0] != archive_keys[1]
        assert application_keys[0] != application_keys[1]
        print(
            f"native-stdlib-reuse-members count={len(reused_members)} "
            f"moved_module_count={len(moved_modules)} "
            f"moved_owner_count={len(reused_moved)} representative={reused_moved[0]}",
            flush=True,
        )


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------

if __name__ == "__main__":
    tests = [
        v for k, v in sorted(globals().items()) if k.startswith("test_") and callable(v)
    ]
    passed = failed = 0
    for t in tests:
        try:
            t()
            passed += 1
            print(f"  PASS  {t.__name__}")
        except Exception as e:
            failed += 1
            print(f"  FAIL  {t.__name__}: {e}")
    print(f"\n{passed}/{passed + failed} passed")
    sys.exit(1 if failed else 0)
