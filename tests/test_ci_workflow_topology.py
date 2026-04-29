from __future__ import annotations

from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_ROOT = REPO_ROOT / ".github" / "workflows"


def _read(path: str) -> str:
    return (REPO_ROOT / path).read_text(encoding="utf-8")


def _default_python_version() -> str:
    version = _read(".python-version").strip()
    components = version.split(".")
    assert len(components) == 2
    assert all(component.isdigit() for component in components)
    return version


def test_ci_push_path_is_cheap_only() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert "docs-gates:" in ci_text
    assert "python-tooling-smoke:" in ci_text
    assert "rust-build-unit-smoke:" in ci_text
    assert "differential-tests:" not in ci_text
    assert "benchmark:" not in ci_text
    assert "parity:" not in ci_text
    assert "runs-on: ubuntu-latest" in ci_text
    assert "runs-on: macos-14" not in ci_text
    assert "Swatinem/rust-cache@v2" in ci_text
    assert "tests/test_bench_harness.py" in ci_text
    assert "tests/test_bench_tool.py" in ci_text
    assert "tests/test_ci_workflow_topology.py" in ci_text
    assert "tests/test_harness_conformance.py" in ci_text
    assert "tests/test_harness_layers.py" in ci_text
    assert "tests/test_monty_conformance_runner.py" in ci_text
    assert "Install native linker" in ci_text
    assert "sudo apt-get install -y lld" in ci_text
    assert "ld.lld --version" in ci_text


def test_pre_commit_hooks_are_read_only_by_default() -> None:
    default_python = _default_python_version()
    pre_commit_text = _read(".pre-commit-config.yaml")

    assert "- id: ruff" in pre_commit_text
    assert "repo: https://github.com/astral-sh/ruff-pre-commit" not in pre_commit_text
    assert "uv run ruff check" in pre_commit_text
    assert f"--python {default_python}" not in pre_commit_text
    assert "--fix" not in pre_commit_text
    assert "- id: ruff-format" in pre_commit_text
    assert "uv run ruff format --check" in pre_commit_text
    assert "uv run ty check src" in pre_commit_text
    assert "tools/secret_guard.py --staged" in pre_commit_text
    assert "- id: end-of-file-fixer" not in pre_commit_text
    assert "- id: trailing-whitespace" not in pre_commit_text
    assert "git diff --cached --check" in pre_commit_text


def test_default_ci_python_version_comes_from_single_file() -> None:
    default_python = _default_python_version()

    checked_files = [".pre-commit-config.yaml"] + [
        f".github/workflows/{path.name}" for path in sorted(WORKFLOW_ROOT.glob("*.yml"))
    ]
    for path in checked_files:
        text = _read(path)
        assert f"--python {default_python}" not in text
        assert f"uv python install {default_python}" not in text
        assert f'python-version: "{default_python}"' not in text
        assert f"python-version: '{default_python}'" not in text

    for workflow in ("ci.yml", "formal.yml", "perf-validation.yml", "release.yml"):
        assert 'python-version-file: ".python-version"' in _read(
            f".github/workflows/{workflow}"
        )


def test_repo_githook_delegates_to_pre_commit_authority() -> None:
    hook_text = _read(".githooks/pre-commit")

    assert "pre-commit run --hook-stage pre-commit" in hook_text
    assert "tools/secret_guard.py" not in hook_text


def test_ci_clippy_failures_are_not_swallowed() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    clippy_lines = [
        line.strip()
        for line in ci_text.splitlines()
        if line.strip().startswith("run: cargo clippy")
    ]

    assert clippy_lines == [
        "run: cargo clippy -p molt-backend --features native-backend -- -D warnings"
    ]


def test_ci_warning_check_reuses_primary_build_output() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert "2>&1 | tee logs/ci-cargo-build.log" in ci_text
    assert "WARNING_COUNT=$(grep -c 'warning\\[' logs/ci-cargo-build.log || true)" in (
        ci_text
    )
    assert "WARNING_LINES=$(grep 'warning\\[' logs/ci-cargo-build.log || true)" in (
        ci_text
    )
    assert "WARNING_COUNT=$(cargo build" not in ci_text
    assert "cargo build 2>&1 | grep 'warning\\['" not in ci_text


def test_kani_intrinsic_contracts_avoid_symbolic_std_sort() -> None:
    kani_text = _read("runtime/molt-obj-model/tests/kani_intrinsic_contracts.rs")

    assert "struct BoundedI64List" in kani_text
    assert "struct BoundedBoolList" in kani_text
    assert "Vec<" not in kani_text
    assert "Vec::" not in kani_text
    assert ".collect()" not in kani_text
    assert "DefaultHasher" not in kani_text
    assert "std::hash" not in kani_text
    assert "wrapping_mul" not in kani_text
    assert ".dedup()" not in kani_text
    assert ".sort()" not in kani_text


def test_kani_workflow_has_single_cargo_cache_authority() -> None:
    kani_workflow = _read(".github/workflows/kani.yml")

    assert "swatinem/rust-cache@v2" in kani_workflow
    assert "actions/cache@v4" not in kani_workflow
    assert "Cache cargo registry and target" not in kani_workflow


def test_nightly_contains_correctness_jobs() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")

    assert "schedule:" in nightly_text
    assert "workflow_dispatch:" in nightly_text
    assert "molt-conformance-full:" in nightly_text
    assert "differential-basic-stdlib:" in nightly_text
    assert "tests/harness/run_molt_conformance.py" in nightly_text
    assert "--suite full" in nightly_text
    assert "--build-profile dev" in nightly_text
    assert 'MOLT_DIFF_MEASURE_RSS: "1"' in nightly_text
    assert 'MOLT_DIFF_RLIMIT_GB: "10"' in nightly_text
    assert "tests/differential/basic" in nightly_text
    assert "tests/differential/stdlib" in nightly_text
    assert 'REPRO_ROOT="$PWD/tmp/repro_sweep"' in nightly_text
    assert "mkdir -p /tmp/repro_sweep" not in nightly_text
    assert "MOLT_CACHE=/tmp/repro_sweep" not in nightly_text
    assert "~/.molt/build/" not in nightly_text
    assert "cargo build -p molt-runtime --profile dev-fast" in nightly_text
    assert "cargo build -p molt-runtime --release" not in nightly_text
    assert "A/B Molt caches and build-state roots are intentionally cold" in (
        nightly_text
    )


def test_release_and_perf_workflows_exist_for_hosted_validation() -> None:
    release_text = _read(".github/workflows/release.yml")
    perf_text = _read(".github/workflows/perf-validation.yml")

    assert "push:" in release_text
    assert "tags:" in release_text
    assert "workflow_dispatch:" in release_text
    assert "macos-14" in release_text
    assert "ubuntu-24.04" in release_text
    assert "schedule:" not in perf_text
    assert "MOLT_SESSION_ID: perf-validation" in perf_text
    assert "CARGO_TARGET_DIR: ${{ github.workspace }}/target" in perf_text
    assert "MOLT_CACHE: ${{ github.workspace }}/.molt_cache" in perf_text
    assert "TMPDIR: ${{ github.workspace }}/tmp" in perf_text
    assert "tools/bench.py" in perf_text
    assert "--molt-profile release" in perf_text
    assert "bench/results/" in perf_text


def test_perf_demo_workflow_uses_canonical_env_and_single_uv_sync() -> None:
    perf_demo_text = _read(".github/workflows/perf_demo.yml")
    run_stack_text = _read("bench/scripts/run_stack.sh")

    assert "MOLT_SESSION_ID: perf-demo-${{ github.run_id }}" in perf_demo_text
    assert "CARGO_TARGET_DIR: ${{ github.workspace }}/target" in perf_demo_text
    assert "MOLT_DIFF_CARGO_TARGET_DIR: ${{ github.workspace }}/target" in (
        perf_demo_text
    )
    assert "MOLT_CACHE: ${{ github.workspace }}/.molt_cache" in perf_demo_text
    assert "TMPDIR: ${{ github.workspace }}/tmp" in perf_demo_text
    assert "UV_CACHE_DIR: ${{ github.workspace }}/.uv-cache" in perf_demo_text
    assert 'MOLT_UV_SYNC: "0"' in perf_demo_text
    assert 'if [[ "${MOLT_UV_SYNC:-1}" != "0" ]]' in run_stack_text
    assert 'cargo build --profile "$CARGO_PROFILE" -p molt-worker' in run_stack_text
    assert 'CARGO_ROOT="${CARGO_TARGET_DIR:-$ROOT/target}"' in run_stack_text
    assert 'WORKER_BIN="$CARGO_ROOT/$CARGO_PROFILE/molt-worker"' in run_stack_text


def test_wasm_ci_uses_molt_wasm_host_for_imported_modules() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")

    assert "timeout-minutes: 50" in wasm_text
    assert "cargo build --profile release-fast -p molt-wasm-host" in wasm_text
    assert (
        "$CARGO_TARGET_DIR/release-fast/molt-wasm-host /tmp/test_hello.wasm"
        in wasm_text
    )
    assert (
        "$CARGO_TARGET_DIR/release-fast/molt-wasm-host /tmp/test_comprehension.wasm"
        in wasm_text
    )
    assert (
        "$CARGO_TARGET_DIR/release-fast/molt-wasm-host /tmp/test_sieve.wasm"
        in wasm_text
    )
    assert "wasmtime run /tmp/test_hello.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_comprehension.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_sieve.wasm" not in wasm_text


def test_wasm_ci_uses_canonical_artifact_roots_and_dev_profile() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")

    assert "MOLT_EXT_ROOT: /tmp/molt-ext" in wasm_text
    assert "- '.python-version'" in wasm_text
    assert "CARGO_TARGET_DIR: /tmp/molt-ext/cargo-target" in wasm_text
    assert "MOLT_CACHE: /tmp/molt-ext/molt_cache" in wasm_text
    assert "MOLT_DIFF_ROOT: /tmp/molt-ext/diff" in wasm_text
    assert "MOLT_DIFF_TMPDIR: /tmp/molt-ext/tmp" in wasm_text
    assert "MOLT_WASM_RUNTIME_DIR: /tmp/molt-ext/wasm" in wasm_text
    assert "MOLT_WASM_TEST_CARGO_TARGET_DIR: /tmp/molt-ext/cargo-target" in wasm_text
    assert (
        "MOLT_SESSION_ID: wasm-ci-${{ github.run_id }}-${{ github.run_attempt }}"
        in wasm_text
    )
    assert (
        "cargo build --profile dev-fast -p molt-backend --no-default-features --features wasm-backend"
        in wasm_text
    )
    assert (
        "cargo build --profile dev-fast -p molt-runtime --target wasm32-wasip1"
        not in wasm_text
    )
    assert "cargo build --release -p molt-wasm-host" not in wasm_text
    assert "uv run python3 -m pytest tests/test_wasm_control_flow.py -q" in wasm_text
    assert wasm_text.count("--build-profile dev") >= 5
    assert "/home/runner/.cache/molt" not in wasm_text
