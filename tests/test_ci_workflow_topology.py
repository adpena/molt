from __future__ import annotations

from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
WORKFLOW_ROOT = REPO_ROOT / ".github" / "workflows"


def _read(path: str) -> str:
    return (REPO_ROOT / path).read_text(encoding="utf-8")


def _named_step_blocks(workflow_text: str) -> list[str]:
    blocks: list[list[str]] = []
    current: list[str] = []
    for line in workflow_text.splitlines():
        if line.startswith("      - name: "):
            if current:
                blocks.append(current)
            current = [line]
        elif current:
            current.append(line)
    if current:
        blocks.append(current)
    return ["\n".join(block) for block in blocks]


def _default_python_version() -> str:
    version = _read(".python-version").strip()
    components = version.split(".")
    assert len(components) == 2
    assert all(component.isdigit() for component in components)
    return version


def test_ci_push_path_is_cheap_only() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert "concurrency:" in ci_text
    assert "group: ${{ github.workflow }}-${{ github.ref }}" in ci_text
    assert "cancel-in-progress: true" in ci_text
    assert "docs-gates:" in ci_text
    # The frontend-Python ty type-check is a zero-diagnostic ratchet enforced in
    # CI (pre-commit is not run in Actions), mirroring the pre-commit `ty` hook.
    assert "uv run ty check src" in ci_text
    # The differential suite-layout checker (lane/naming hygiene) runs in
    # docs-gates alongside the suite-honesty ratchet — a blocking gate so new
    # lane/naming debt cannot land silently.
    assert "uv run python3 tools/check_differential_suite_layout.py" in ci_text
    assert "python-tooling-smoke:" in ci_text
    assert "rust-build-unit-smoke:" in ci_text
    assert "llvm-backend:" in ci_text
    assert "needs: docs-gates" not in ci_text
    assert "differential-tests:" not in ci_text
    assert "benchmark:" not in ci_text
    assert "parity:" not in ci_text
    assert "runs-on: ubuntu-latest" in ci_text
    assert "runs-on: macos-14" not in ci_text
    assert "Swatinem/rust-cache@v2" in ci_text
    # Three rust-bearing jobs configure adaptive parallelism: python-tooling-smoke,
    # rust-build-unit-smoke, and the LLVM backend job.
    assert ci_text.count("Configure adaptive Rust parallelism") == 3
    assert (
        ci_text.count('python3 tools/ci_resource_env.py --github-env "$GITHUB_ENV"')
        == 3
    )
    assert 'CARGO_BUILD_JOBS: "1"' not in ci_text
    assert "uv sync --frozen --group dev" in ci_text
    assert '-m "not slow"' in ci_text
    assert "Run bench CLI native smoke tests" in ci_text
    assert (
        "tests/test_bench_tool.py::"
        "test_bench_cli_native_smoke_contract_batch_reuses_compiler" in ci_text
    )
    assert "tests/test_bench_tool.py::test_bench_no_cpython_sets_null_baseline" not in (
        ci_text
    )
    assert (
        "tests/test_bench_tool.py::test_bench_runtime_timeout_marks_molt_not_ok"
        not in (ci_text)
    )
    assert "tests/test_bench_harness.py" in ci_text
    assert "tests/test_bench_tool.py" in ci_text
    assert "tests/test_ci_workflow_topology.py" in ci_text
    assert "tests/test_harness_conformance.py" in ci_text
    assert "tests/test_harness_layers.py" in ci_text
    assert "tests/test_monty_conformance_runner.py" in ci_text
    assert "Install native linker" in ci_text
    assert "sudo apt-get install -y lld" in ci_text
    assert "ld.lld --version" in ci_text
    assert 'MOLT_NATIVE_TEST_TIMEOUT_SEC: "900"' in ci_text
    assert ci_text.count("Run bench CLI native smoke tests") == 1
    # Four jobs summarize hotspots: docs-gates, python-tooling-smoke,
    # rust-build-unit-smoke, and the LLVM backend job.
    assert ci_text.count("Summarize guarded command hotspots") == 4
    assert ci_text.count("python3 tools/profile_hotspots.py --limit 20") == 4


def test_github_workflows_opt_into_node24_action_runtime() -> None:
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        if "uses:" not in text:
            continue

        assert 'FORCE_JAVASCRIPT_ACTIONS_TO_NODE24: "true"' in text, workflow
        assert "ACTIONS_ALLOW_USE_UNSECURE_NODE_VERSION" not in text, workflow


def test_github_workflows_do_not_reintroduce_node20_action_pins() -> None:
    node20_action_pins = {
        "actions/checkout@v4",
        "actions/checkout@v5",
        "actions/setup-python@v5",
        "actions/setup-node@v4",
        "actions/cache@v4",
        "actions/upload-artifact@v4",
        "actions/upload-artifact@v6",
        "actions/download-artifact@v4",
        "actions/github-script@v7",
        "actions/attest-build-provenance@v2",
        "astral-sh/setup-uv@v3",
        "astral-sh/setup-uv@v4",
        "astral-sh/setup-uv@v7",
        "astral-sh/setup-uv@v8.1.0",
        "softprops/action-gh-release@v2",
    }

    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        for action_pin in sorted(node20_action_pins):
            assert action_pin not in text, (workflow, action_pin)


def test_github_workflows_use_current_setup_uv_release() -> None:
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        setup_uv_lines = [
            line.strip() for line in text.splitlines() if "astral-sh/setup-uv@" in line
        ]
        if not setup_uv_lines:
            continue

        assert all("astral-sh/setup-uv@v8.2.0" in line for line in setup_uv_lines), (
            workflow,
            setup_uv_lines,
        )


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

    for workflow in ("ci.yml", "formal.yml", "release.yml"):
        assert 'python-version-file: ".python-version"' in _read(
            f".github/workflows/{workflow}"
        )


def test_repo_githook_delegates_to_pre_commit_authority() -> None:
    hook_text = _read(".githooks/pre-commit")

    assert "pre-commit run --hook-stage pre-commit" in hook_text
    assert "tools/secret_guard.py" not in hook_text


def test_ci_clippy_failures_are_not_swallowed() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    backend_clippy_lines = [
        line.strip()
        for line in ci_text.splitlines()
        if "cargo clippy -p molt-backend --features native-backend -- -D warnings"
        in line
    ]
    tir_clippy_lines = [
        line.strip()
        for line in ci_text.splitlines()
        if "cargo clippy -p molt-tir --all-targets --all-features -- -D warnings"
        in line
    ]

    assert backend_clippy_lines == [
        "run: python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo clippy -p molt-backend --features native-backend -- -D warnings"
    ]
    assert tir_clippy_lines == [
        "run: python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo clippy -p molt-tir --all-targets --all-features -- -D warnings"
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


def test_ci_memory_intensive_steps_use_memory_guard() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- \\\n"
        "            uv run python3 -m pytest -q"
    ) in ci_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- cargo build"
        in ci_text
    )
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- cargo test -p molt-backend"
        in ci_text
    )
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- cargo clippy"
        in ci_text
    )
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- \\\n"
        "            env PYTHONPATH=src uv run python3 -c"
    ) in ci_text
    assert "\n          PYTHONPATH=src uv run python3 -c" not in ci_text
    assert "python3 tools/profile_hotspots.py --limit 20" in ci_text


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
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo install --locked kani-verifier"
    ) in kani_workflow
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- cargo kani setup"
    ) in kani_workflow
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE "
        "--cwd runtime/molt-obj-model -- cargo kani --tests"
    ) in kani_workflow
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE "
        "--cwd runtime/molt-runtime -- cargo kani --tests"
    ) in kani_workflow
    assert "cd runtime/molt-obj-model && cargo kani --tests" not in kani_workflow
    assert "cd runtime/molt-runtime && cargo kani --tests" not in kani_workflow


def test_formal_workflow_uses_bounded_blocking_quint_gate() -> None:
    formal_workflow = _read(".github/workflows/formal.yml")

    assert "python3 tools/check_formal_methods.py --quint-only" in formal_workflow
    assert "for model in *.qnt" not in formal_workflow
    assert 'quint verify "$model"' not in formal_workflow
    assert "failed verification (non-blocking)" not in formal_workflow


def test_quint_workflows_pin_patched_node24_toolchain() -> None:
    formal_workflow = _read(".github/workflows/formal.yml")
    nightly_workflow = _read(".github/workflows/nightly.yml")

    assert "actions/setup-node@v6" in formal_workflow
    assert "node-version: '24.16.0'" in formal_workflow
    assert "check-latest: true" in formal_workflow
    assert 'MOLT_QUINT_NPM_PACKAGE: "@informalsystems/quint@0.32.0"' in (
        formal_workflow
    )
    assert 'MOLT_QUINT_RUST_EVALUATOR_VERSION: "v0.6.0"' in formal_workflow
    assert "Install Quint Rust evaluator" in formal_workflow
    assert "sha256sum --check" in formal_workflow

    assert nightly_workflow.count('npm install -g "$MOLT_QUINT_NPM_PACKAGE"') == 2
    assert nightly_workflow.count("actions/setup-node@v6") >= 2
    assert nightly_workflow.count("node-version: '24.16.0'") >= 2
    assert nightly_workflow.count("check-latest: true") >= 2
    assert 'MOLT_QUINT_NPM_PACKAGE: "@informalsystems/quint@0.32.0"' in (
        nightly_workflow
    )
    assert nightly_workflow.count("Install Quint Rust evaluator") >= 1
    assert nightly_workflow.count("sha256sum --check") >= 1


def test_nightly_contains_correctness_jobs() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")

    assert "schedule:" in nightly_text
    assert "workflow_dispatch:" in nightly_text
    assert "molt-conformance-full:" in nightly_text
    assert "differential-basic-stdlib:" in nightly_text
    assert "tests/harness/run_molt_conformance.py" in nightly_text
    assert "tools/guarded_exec.py --prefix MOLT_CONFORMANCE" in nightly_text
    assert "tools/guarded_exec.py --prefix MOLT_DIFF" in nightly_text
    assert "tools/guarded_exec.py --prefix MOLT_REGRTEST" in nightly_text
    assert "tools/guarded_exec.py --prefix MOLT_TEST_SUITE" in nightly_text
    assert "--suite full" in nightly_text
    assert "--build-profile dev" in nightly_text
    assert 'MOLT_DIFF_MEASURE_RSS: "1"' in nightly_text
    assert "MOLT_DIFF_RLIMIT_GB" not in nightly_text
    assert "tests/differential/basic" in nightly_text
    assert "tests/differential/stdlib" in nightly_text
    assert 'REPRO_ROOT="$PWD/tmp/repro_sweep"' in nightly_text
    assert "mkdir -p /tmp/repro_sweep" not in nightly_text
    assert "MOLT_CACHE=/tmp/repro_sweep" not in nightly_text
    assert "~/.molt/build/" not in nightly_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo build -p molt-runtime --profile dev-fast"
    ) in nightly_text
    assert "cargo build -p molt-runtime --release" not in nightly_text
    assert "A/B Molt caches and build-state roots are intentionally cold" in (
        nightly_text
    )


def test_hosted_workflow_heavy_commands_enter_memory_guard() -> None:
    nightly_text = _read(".github/workflows/nightly.yml")
    formal_text = _read(".github/workflows/formal.yml")
    security_text = _read(".github/workflows/security_hardening.yml")
    release_text = _read(".github/workflows/release.yml")

    guarded_runtime_build = (
        "run: python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo build -p molt-runtime --profile dev-fast"
    )
    assert nightly_text.count(guarded_runtime_build) == 3
    assert "run: cargo build -p molt-runtime --profile dev-fast" not in nightly_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "uv run python3 tools/ci_gate.py --tier 3 --verbose --json"
    ) in nightly_text
    assert (
        "run: uv run python3 tools/ci_gate.py --tier 3 --verbose --json"
        not in nightly_text
    )
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "quint verify formal/quint/molt_build_determinism.qnt"
    ) in nightly_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "quint verify formal/quint/molt_runtime_determinism.qnt"
    ) in nightly_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "quint verify formal/quint/molt_midend_pipeline.qnt"
    ) in nightly_text
    assert "run: cargo install cargo-deny --locked" not in nightly_text
    assert "run: cargo deny check" not in nightly_text
    assert "          quint verify formal/quint/" not in nightly_text

    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE --cwd formal/lean -- "
        "lake build"
    ) in formal_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "python3 tools/check_formal_methods.py --quint-only"
    ) in formal_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "python3 tools/check_formal_methods.py --check-correspondence"
    ) in formal_text
    assert "run: lake build" not in formal_text
    assert "run: python3 tools/check_formal_methods.py --quint-only" not in formal_text
    assert (
        "run: python3 tools/check_formal_methods.py --check-correspondence"
        not in formal_text
    )

    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "uv run pip-audit --ignore-vuln CVE-2025-69872"
    ) in security_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- cargo deny check"
        in security_text
    )
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- cargo audit"
        in security_text
    )
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo install cargo-deny --locked"
    ) in security_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo install cargo-audit --locked"
    ) in security_text
    assert "run: uv run pip-audit --ignore-vuln CVE-2025-69872" not in security_text
    assert "run: cargo deny check" not in security_text
    assert "          cargo install cargo-deny --locked" not in security_text
    assert "          cargo install cargo-audit --locked" not in security_text

    assert (
        '"$PYTHON_BIN" tools/guarded_exec.py --prefix MOLT_RELEASE -- '
        "cargo build -p molt-worker --release"
    ) in release_text
    assert '"$PYTHON_BIN" tools/guarded_exec.py --prefix MOLT_RELEASE -- \\' in (
        release_text
    )
    assert "run: cargo build -p molt-worker --release" not in release_text


def test_release_and_perf_workflows_exist_for_hosted_validation() -> None:
    release_text = _read(".github/workflows/release.yml")
    perf_text = _read(".github/workflows/perf-gate.yml")

    assert "push:" in release_text
    assert "tags:" in release_text
    assert "workflow_dispatch:" in release_text
    assert "macos-14" in release_text
    assert "ubuntu-24.04" in release_text
    assert "schedule:" in perf_text
    assert "MOLT_SESSION_ID: perfscore-${{ matrix.backend }}" in perf_text
    assert (
        "CARGO_TARGET_DIR: ${{ github.workspace }}/target/sessions/perfscore-${{ matrix.backend }}"
        in perf_text
    )
    assert "MOLT_CACHE: ${{ github.workspace }}/.molt_cache" in perf_text
    assert "TMPDIR: ${{ github.workspace }}/tmp" in perf_text
    assert "tools/guarded_exec.py --prefix MOLT_BENCH" in perf_text
    assert "tools/perf_scoreboard.py" in perf_text
    assert "backend: [native, llvm]" in perf_text
    assert '--backend "${{ matrix.backend }}"' in perf_text
    assert "--profile release-fast" in perf_text
    assert "--samples 5" in perf_text
    assert "--warmup 2" in perf_text
    assert "--repeat 5" in perf_text
    assert "--classify" in perf_text
    assert "--require-quiescent" in perf_text
    assert "--no-gate" not in perf_text
    assert "--allow-nonauthoritative" not in perf_text
    assert "tools/bench.py" not in perf_text
    assert "bench/results/" not in perf_text
    assert not (WORKFLOW_ROOT / "perf-validation.yml").exists()


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
    assert "cargo build --profile dev-fast -p molt-wasm-host" in wasm_text
    assert "$CARGO_TARGET_DIR/dev-fast/molt-wasm-host /tmp/test_hello.wasm" in wasm_text
    assert (
        "$CARGO_TARGET_DIR/dev-fast/molt-wasm-host /tmp/test_comprehension.wasm"
        in wasm_text
    )
    assert "$CARGO_TARGET_DIR/dev-fast/molt-wasm-host /tmp/test_sieve.wasm" in wasm_text
    assert "wasmtime run /tmp/test_hello.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_comprehension.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_sieve.wasm" not in wasm_text


def test_wasm_ci_uses_canonical_artifact_roots_and_dev_profile() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")

    assert "MOLT_EXT_ROOT: /tmp/molt-ext" in wasm_text
    assert "- '.python-version'" in wasm_text
    assert "- 'tools/venv_exec.py'" in wasm_text
    assert "CARGO_TARGET_DIR: ${{ github.workspace }}/target" in wasm_text
    assert "MOLT_DIFF_CARGO_TARGET_DIR: ${{ github.workspace }}/target" in wasm_text
    assert "MOLT_CACHE: /tmp/molt-ext/molt_cache" in wasm_text
    assert "MOLT_DIFF_ROOT: /tmp/molt-ext/diff" in wasm_text
    assert "MOLT_DIFF_TMPDIR: /tmp/molt-ext/tmp" in wasm_text
    assert "MOLT_WASM_RUNTIME_DIR: /tmp/molt-ext/wasm" in wasm_text
    assert "concurrency:" in wasm_text
    assert "cancel-in-progress: true" in wasm_text
    assert "MOLT_CI_PYTHON" not in wasm_text
    assert (
        "MOLT_WASM_TEST_CARGO_TARGET_DIR: ${{ github.workspace }}/target" in wasm_text
    )
    assert "enable-cache: true" in wasm_text
    assert "cache-dependency-glob: uv.lock" in wasm_text
    assert (
        "MOLT_SESSION_ID: wasm-ci-${{ github.run_id }}-${{ github.run_attempt }}"
        in wasm_text
    )
    assert 'MOLT_WASM_TEST_CHILD_RLIMIT_GB: "0"' in wasm_text
    assert 'MOLT_WASM_TEST_TIMEOUT_SEC: "600"' in wasm_text
    assert 'MOLT_WASM_TEST_KEEPALIVE_SEC: "20"' in wasm_text
    assert 'MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC: "2"' in wasm_text
    assert wasm_text.count('MOLT_BACKEND_DAEMON: "0"') == 5
    assert "MOLT_BACKEND_DAEMON_SOCKET_DIR" not in wasm_text
    assert "MOLT_BACKEND_DAEMON_CACHE_MB" not in wasm_text
    parity_step = next(
        block
        for block in _named_step_blocks(wasm_text)
        if block.startswith("      - name: Run WASM control flow parity tests")
    )
    assert "MOLT_BACKEND_DAEMON" not in parity_step
    assert 'MOLT_WASM_TEST_CHILD_RLIMIT_GB: "0"' in parity_step
    assert (
        "cargo build --profile dev-fast -p molt-backend --no-default-features --features wasm-backend"
        in wasm_text
    )
    assert (
        "cargo build --profile dev-fast -p molt-runtime --target wasm32-wasip1"
        not in wasm_text
    )
    assert "cargo build --release -p molt-wasm-host" not in wasm_text
    assert "cargo build --profile release-fast -p molt-wasm-host" not in wasm_text
    assert "python3 tools/guarded_exec.py --prefix MOLT_WASM_TEST" in wasm_text
    assert (
        "-- python3 tools/venv_exec.py python3 -m pytest tests/test_wasm_control_flow.py -q"
        in wasm_text
    )
    assert "uv run python3 -m molt.cli build" not in wasm_text
    assert "uv run python3 -m pytest tests/test_wasm_control_flow.py -q" not in (
        wasm_text
    )
    assert "python3 tools/venv_exec.py python3 -m molt.cli build" in wasm_text
    assert (
        wasm_text.count("python3 tools/guarded_exec.py --prefix MOLT_WASM_TEST") >= 10
    )
    assert "=== Guarded Command Hotspots ===" in wasm_text
    assert "python3 tools/profile_hotspots.py --limit 20" in wasm_text
    assert wasm_text.count("--build-profile dev") >= 5
    assert "/home/runner/.cache/molt" not in wasm_text


def test_wasm_ci_guarded_steps_have_github_timeout_backstops() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    guarded_steps = [
        block
        for block in _named_step_blocks(wasm_text)
        if "tools/guarded_exec.py --prefix MOLT_WASM_TEST" in block
    ]

    assert len(guarded_steps) >= 10
    missing = [
        block.splitlines()[0].removeprefix("      - name: ")
        for block in guarded_steps
        if "timeout-minutes:" not in block
    ]
    assert missing == []
    for block in guarded_steps:
        timeout_line = next(
            line.strip()
            for line in block.splitlines()
            if line.strip().startswith("timeout-minutes:")
        )
        timeout_minutes = int(timeout_line.split(":", 1)[1].strip())
        assert 1 <= timeout_minutes <= 20, block
