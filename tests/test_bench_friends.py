from __future__ import annotations

import importlib.util
import json
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.native_process_guard import run_native_test_process


REPO_ROOT = Path(__file__).resolve().parents[1]
TOOL_PATH = REPO_ROOT / "tools" / "bench_friends.py"
ADAPTER_PATH = REPO_ROOT / "tools" / "tinygrad_off_shelf_adapter.py"


def _load_tool_module():
    spec = importlib.util.spec_from_file_location("bench_friends_under_test", TOOL_PATH)
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _load_tinygrad_adapter_module():
    spec = importlib.util.spec_from_file_location(
        "tinygrad_off_shelf_adapter_under_test", ADAPTER_PATH
    )
    assert spec is not None
    assert spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def _run_tool(*args: str) -> subprocess.CompletedProcess[str]:
    return run_native_test_process(
        ["python3", "tools/bench_friends.py", *args],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def _git(repo: Path, *args: str) -> subprocess.CompletedProcess[str]:
    return run_native_test_process(
        ["git", *args],
        cwd=repo,
        text=True,
        capture_output=True,
        check=True,
    )


def _init_git_repo(repo: Path) -> str:
    repo.mkdir(parents=True, exist_ok=True)
    _git(repo, "init")
    (repo / "script.py").write_text("print('ok')\n", encoding="utf-8")
    _git(repo, "add", "script.py")
    _git(
        repo,
        "-c",
        "user.email=molt@example.invalid",
        "-c",
        "user.name=Molt Test",
        "commit",
        "-m",
        "initial",
    )
    return _git(repo, "rev-parse", "HEAD").stdout.strip()


def test_default_output_root_is_canonical_bench_results(monkeypatch) -> None:
    module = _load_tool_module()
    monkeypatch.setenv("MOLT_EXT_ROOT", str(REPO_ROOT))

    output_root = module._default_output_root()

    assert output_root.parent == REPO_ROOT / "bench" / "results" / "friends"


def test_bench_friends_local_suite_runs(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)

    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "local_smoke"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "runs_unmodified"
            repeat = 2
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.01)"]

            [suite.runners.molt]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.02)"]

            [suite.runners.friend]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.03)"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr

    results_json = output_root / "results.json"
    assert results_json.exists()
    payload = json.loads(results_json.read_text(encoding="utf-8"))
    assert payload["suites"]
    suite = payload["suites"][0]
    assert suite["id"] == "local_smoke"
    assert suite["status"] == "ok"
    assert suite["metrics"]["molt_vs_cpython_speedup"] is not None

    summary = output_root / "summary.md"
    assert summary.exists()
    summary_text = summary.read_text(encoding="utf-8")
    assert "local_smoke" in summary_text


def test_bench_friends_include_disabled_with_dry_run(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "dry_out"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "disabled_suite"
            enabled = false
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "requires_adapter"

            [suite.runners.cpython]
            skip_reason = "not configured"
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--include-disabled",
        "--dry-run",
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    assert payload["suites"][0]["status"] == "skipped"


def test_bench_friends_nuitka_pyodide_runners(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_ext"
    suite_root = tmp_path / "suite_ext"
    suite_root.mkdir(parents=True, exist_ok=True)

    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "ext_runners_smoke"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "requires_adapter"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.01)"]

            [suite.runners.molt]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.02)"]

            [suite.runners.nuitka]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.03)"]

            [suite.runners.pyodide]
            run_cmd = ["python3", "-c", "import time; time.sleep(0.04)"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "ok"
    assert suite["runners"]["nuitka"]["status"] == "ok"
    assert suite["runners"]["pyodide"]["status"] == "ok"
    assert suite["metrics"]["nuitka_median_s"] is not None
    assert suite["metrics"]["pyodide_median_s"] is not None
    assert suite["metrics"]["molt_vs_nuitka_speedup"] is not None
    assert suite["metrics"]["molt_vs_pyodide_speedup"] is not None

    summary_text = (output_root / "summary.md").read_text(encoding="utf-8")
    assert "Nuitka s" in summary_text
    assert "Pyodide s" in summary_text


def test_bench_friends_dynamic_runner_keys_are_manifest_authority() -> None:
    module = _load_tool_module()

    suite = module._parse_suite(
        {
            "id": "dynamic_runner_smoke",
            "enabled": True,
            "friend": "tinygrad",
            "source": "local",
            "local_path": ".",
            "semantic_mode": "runs_unmodified",
            "runners": {
                "tinygrad": {
                    "run_cmd": ["{python}", "-c", "print('ok')"],
                    "structured_stdout": "json",
                }
            },
        },
        {},
    )

    assert suite.runners["tinygrad"].json_stdout is True
    assert suite.runners["tinygrad"].run_cmd == ["{python}", "-c", "print('ok')"]

    with pytest.raises(ValueError, match="invalid runner name"):
        module._parse_suite(
            {
                "id": "bad_runner",
                "enabled": True,
                "friend": "local",
                "source": "local",
                "local_path": ".",
                "semantic_mode": "runs_unmodified",
                "runners": {"bad runner": {"run_cmd": ["python3", "-c", "pass"]}},
            },
            {},
        )


def test_bench_friends_suite_root_override_and_structured_json_metrics(
    tmp_path: Path,
) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_json"
    suite_root = tmp_path / "suite_override"
    suite_root.mkdir(parents=True, exist_ok=True)
    emitter = tmp_path / "emit_json.py"
    emitter.write_text(
        textwrap.dedent(
            """
            import json
            import sys

            scale = float(sys.argv[1])
            print(json.dumps({
                "status": "ok",
                "workloads": {
                    "elementwise_chain": {"elapsed_s": scale},
                    "matmul_2x2": {"elapsed_s": scale * 2.0},
                },
            }))
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "json_metrics"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{(tmp_path / 'missing').as_posix()}"
            semantic_mode = "requires_adapter"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            json_stdout = true
            run_cmd = ["{{python}}", "{emitter.as_posix()}", "0.20"]

            [suite.runners.molt]
            json_stdout = true
            run_cmd = ["{{python}}", "{emitter.as_posix()}", "0.10"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
        "--suite-root",
        f"json_metrics={suite_root}",
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "ok"
    assert suite["source_custody"]["suite_root_overridden"] is True
    assert suite["runners"]["cpython"]["structured_median_s"] == {
        "elementwise_chain": 0.2,
        "matmul_2x2": 0.4,
    }
    assert suite["runners"]["molt"]["structured_median_s"] == {
        "elementwise_chain": 0.1,
        "matmul_2x2": 0.2,
    }
    assert suite["metrics"]["cpython_elementwise_chain_median_s"] == 0.2
    assert suite["metrics"]["molt_elementwise_chain_median_s"] == 0.1
    assert suite["metrics"]["molt_vs_cpython_elementwise_chain_speedup"] == 2.0


def test_bench_friends_runner_filter_runs_only_selected_lane(tmp_path: Path) -> None:
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_runner_filter"
    suite_root = tmp_path / "suite"
    suite_root.mkdir(parents=True, exist_ok=True)
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "runner_filter"
            enabled = true
            friend = "local"
            source = "local"
            local_path = "{suite_root.as_posix()}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["{{python}}", "-c", "print('cpython')"]

            [suite.runners.molt]
            run_cmd = ["{{python}}", "-c", "raise SystemExit(99)"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
        "--runner",
        "cpython",
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert payload["options"]["runner_filter"] == ["cpython"]
    assert set(suite["runners"]) == {"cpython"}
    assert suite["runners"]["cpython"]["status"] == "ok"


def test_bench_friends_git_suite_records_clean_ref_custody(tmp_path: Path) -> None:
    origin = tmp_path / "origin"
    commit = _init_git_repo(origin)
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_git"
    manifest.write_text(
        textwrap.dedent(
            f"""
            schema_version = 1

            [[suite]]
            id = "git_smoke"
            enabled = true
            friend = "local"
            source = "git"
            repo_url = "{origin.as_posix()}"
            repo_ref = "{commit}"
            semantic_mode = "runs_unmodified"
            repeat = 1
            timeout_sec = 30

            [suite.runners.cpython]
            run_cmd = ["{{python}}", "script.py"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--output-root",
        str(output_root),
        "--repos-root",
        str(tmp_path / "repos"),
    )
    assert res.returncode == 0, res.stderr

    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["resolved_ref"] == commit
    assert suite["requested_ref"] == commit
    assert suite["source_custody"]["expected_ref"] == commit
    assert suite["source_custody"]["ref_verified"] is True
    assert suite["source_custody"]["git_clean"] is True


def test_bench_friends_git_suite_rejects_dirty_override_checkout(
    tmp_path: Path,
) -> None:
    checkout = tmp_path / "dirty_checkout"
    commit = _init_git_repo(checkout)
    (checkout / "untracked.py").write_text("print('dirty')\n", encoding="utf-8")
    manifest = tmp_path / "manifest.toml"
    output_root = tmp_path / "out_dirty"
    manifest.write_text(
        textwrap.dedent(
            """
            schema_version = 1

            [[suite]]
            id = "dirty_git"
            enabled = true
            friend = "local"
            source = "git"
            repo_url = "unused"
            repo_ref = "PINNED_COMMIT_REQUIRED"
            semantic_mode = "runs_unmodified"

            [suite.runners.cpython]
            run_cmd = ["{python}", "script.py"]
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = _run_tool(
        "--manifest",
        str(manifest),
        "--suite",
        "dirty_git",
        "--output-root",
        str(output_root),
        "--suite-root",
        f"dirty_git={checkout}",
        "--repo-ref",
        f"dirty_git={commit}",
        "--no-checkout",
    )
    assert res.returncode == 1
    payload = json.loads((output_root / "results.json").read_text(encoding="utf-8"))
    suite = payload["suites"][0]
    assert suite["status"] == "failed"
    assert "git checkout is dirty" in suite["reason"]
    assert suite["source_custody"]["suite_root_overridden"] is True


def test_friend_manifest_registers_tinygrad_off_the_shelf_suite() -> None:
    module = _load_tool_module()
    _meta, suites = module._load_manifest(REPO_ROOT / "bench/friends/manifest.toml")
    suite = next(s for s in suites if s.id == "tinygrad_off_the_shelf")

    assert suite.enabled is False
    assert suite.friend == "tinygrad"
    assert suite.source == "git"
    assert suite.repo_url == "https://github.com/tinygrad/tinygrad.git"
    assert suite.repo_ref == "a83710396c991272241e40da94489747c2393851"
    assert suite.semantic_mode == "runs_unmodified"
    assert {"gpu", "mlir", "tinygrad", "compatibility", "benchmark-suite"} <= set(
        suite.tags
    )

    cpython = suite.runners["cpython"]
    molt = suite.runners["molt"]
    assert cpython.json_stdout is True
    assert molt.json_stdout is True
    assert cpython.run_cmd[0] == "{python}"
    assert cpython.run_cmd is not None
    assert molt.run_cmd is not None
    assert "tools/tinygrad_off_shelf_adapter.py" in cpython.run_cmd[1]
    assert "tools/tinygrad_off_shelf_adapter.py" in molt.run_cmd[4]
    assert molt.env["MOLT_MODULE_ROOTS"] == "{suite_root}"
    assert molt.env["MOLT_EXTERNAL_STATIC_PACKAGES"] == "tinygrad"
    assert suite.runners["friend"].skip_reason


def test_tinygrad_off_shelf_adapter_runs_public_api_workloads(tmp_path: Path) -> None:
    tinygrad_pkg = tmp_path / "tinygrad"
    tinygrad_pkg.mkdir()
    (tinygrad_pkg / "__init__.py").write_text(
        textwrap.dedent(
            """
            class Tensor:
                def __init__(self, data):
                    self.data = data

                def _binary(self, other, op):
                    if isinstance(self.data[0], list):
                        return Tensor([
                            [op(a, b) for a, b in zip(row_a, row_b)]
                            for row_a, row_b in zip(self.data, other.data)
                        ])
                    return Tensor([op(a, b) for a, b in zip(self.data, other.data)])

                def __add__(self, other):
                    return self._binary(other, lambda a, b: a + b)

                def __mul__(self, other):
                    return self._binary(other, lambda a, b: a * b)

                def __matmul__(self, other):
                    rows = []
                    cols = list(zip(*other.data))
                    for row in self.data:
                        rows.append([sum(a * b for a, b in zip(row, col)) for col in cols])
                    return Tensor(rows)

                def realize(self):
                    return self

                def numpy(self):
                    return self

                def tolist(self):
                    return self.data
            """
        ).strip()
        + "\n",
        encoding="utf-8",
    )

    res = run_native_test_process(
        [
            "python3",
            "tools/tinygrad_off_shelf_adapter.py",
            "--suite-root",
            str(tmp_path),
            "--workload",
            "all",
            "--iterations",
            "2",
            "--json",
        ],
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
        check=False,
    )
    assert res.returncode == 0, res.stderr
    payload = json.loads(res.stdout)
    assert payload["status"] == "ok"
    assert sorted(payload["workloads"]) == ["elementwise_chain", "matmul_2x2"]
    assert payload["workloads"]["elementwise_chain"]["result"] == [
        5.0,
        10.0,
        15.0,
        20.0,
    ]
    assert payload["workloads"]["matmul_2x2"]["result"] == [
        [19.0, 22.0],
        [43.0, 50.0],
    ]


def test_tinygrad_off_shelf_adapter_prefers_tolist_without_numpy() -> None:
    module = _load_tinygrad_adapter_module()

    class TinygradLikeTensor:
        def tolist(self):
            return [1.0, 2.0]

        def numpy(self):
            raise AssertionError("adapter should not require numpy when tolist exists")

    assert module._as_nested_list(TinygradLikeTensor()) == [1.0, 2.0]
