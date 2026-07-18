from __future__ import annotations

from pathlib import Path
import tomllib


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

    docs_gate = ci_text.split("  docs-gates:", 1)[1].split("\n  classify-changes:", 1)[
        0
    ]
    rustfmt_setup = docs_gate.index("uses: dtolnay/rust-toolchain@1.96.1")
    repository_executor = docs_gate.index("Execute repository policy partitions")
    assert rustfmt_setup < repository_executor
    assert "components: rustfmt, clippy" in docs_gate
    assert "targets: wasm32-wasip1" in docs_gate

    assert "concurrency:" in ci_text
    assert "merge_group:" in ci_text
    assert "github.event_name == 'pull_request'" in ci_text
    assert "format('pr-{0}', github.event.pull_request.number)" in ci_text
    assert "cancel-in-progress: ${{ github.event_name == 'pull_request' }}" in (ci_text)
    assert "docs-gates:" in ci_text
    assert "classify-changes:" in ci_text
    assert "name: Changed Path Classifier" in ci_text
    # The frontend-Python ty type-check is a zero-diagnostic ratchet enforced in
    # CI (pre-commit is not run in Actions), mirroring the pre-commit `ty` hook.
    assert 'argv = ["uv", "run", "ty", "check", "src"]' in _read(
        "tools/proof_plan.toml"
    )
    # Proof spelling lives only in the manifest; docs-gates is executor mechanics.
    assert "--run-family repository_policy --receipt" in ci_text
    assert 'id = "repository.differential.layout"' in _read("tools/proof_plan.toml")
    assert "uv run python3 tools/check_differential_suite_layout.py" not in ci_text
    assert "python-static:" in ci_text
    assert "python-unit:" in ci_text
    assert "native-integration:" in ci_text
    assert "needs: classify-changes" in ci_text
    assert "if: needs.classify-changes.outputs.python_static == 'true'" in ci_text
    assert "rust-build-unit-smoke:" in ci_text
    assert "if: needs.classify-changes.outputs.rust == 'true'" in ci_text
    assert "llvm-backend:" in ci_text
    assert "if: needs.classify-changes.outputs.llvm == 'true'" in ci_text
    assert "      - docs-gates" in ci_text
    assert "differential-tests:" not in ci_text
    assert "benchmark:" not in ci_text
    assert "parity:" not in ci_text
    assert "runs-on: ubuntu-latest" in ci_text
    assert "runs-on: ${{ matrix.runner }}" in ci_text
    assert "matrix: ${{ fromJSON(needs.classify-changes.outputs.matrix) }}" in ci_text
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
    proof_plan_text = _read("tools/proof_plan.toml")
    assert '"-m", "not slow"' in proof_plan_text
    assert "native.integration.bench-cli" in proof_plan_text
    assert "tests/test_bench_tool.py::test_bench_no_cpython_sets_null_baseline" not in (
        ci_text
    )
    assert (
        "tests/test_bench_tool.py::test_bench_runtime_timeout_marks_molt_not_ok"
        not in (ci_text)
    )
    assert "tests/test_bench_harness.py" in proof_plan_text
    assert "tests/test_bench_tool.py" in proof_plan_text
    assert "tests/test_ci_workflow_topology.py" in proof_plan_text
    assert "tests/test_harness_conformance.py" in proof_plan_text
    assert "tests/test_harness_layers.py" in proof_plan_text
    assert "tests/test_monty_conformance_runner.py" in proof_plan_text
    assert "Setup canonical native linker SDK" in ci_text
    assert "proof-receipts/evidence/cargo-test-truth.json" in ci_text
    assert "uses: ./.github/actions/setup-llvm" in ci_text
    assert "sudo apt-get install -y lld" not in ci_text
    assert "timeout_seconds" in proof_plan_text
    # Four jobs summarize hotspots: docs-gates, python-tooling-smoke,
    # rust-build-unit-smoke, and the LLVM backend job.
    assert ci_text.count("Summarize guarded command hotspots") == 4
    assert ci_text.count("python3 tools/profile_hotspots.py --limit 20") == 4


def test_ci_heavy_jobs_are_path_classified() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert 'python3 tools/proof_plan.py --github-output "$GITHUB_OUTPUT"' in (ci_text)
    assert "python_static: ${{ steps.paths.outputs.python_static }}" in ci_text
    assert "python_unit: ${{ steps.paths.outputs.python_unit }}" in ci_text
    assert (
        "native_integration: ${{ steps.paths.outputs.native_integration }}" in ci_text
    )
    assert "rust: ${{ steps.paths.outputs.rust }}" in ci_text
    assert "llvm: ${{ steps.paths.outputs.llvm }}" in ci_text
    assert "python_security: ${{ steps.paths.outputs.python_security }}" in ci_text
    assert "rust_security: ${{ steps.paths.outputs.rust_security }}" in ci_text
    assert (
        "platform_portability: ${{ steps.paths.outputs.platform_portability }}"
        in ci_text
    )
    assert "matrix: ${{ steps.paths.outputs.matrix }}" in ci_text
    assert "topology: ${{ steps.paths.outputs.topology }}" in ci_text
    assert "selected: ${{ steps.paths.outputs.selected }}" in ci_text
    assert ci_text.count("needs: classify-changes") >= 4
    assert "proof-plan-verdict:" in ci_text
    assert "name: Proof Plan Verdict" in ci_text
    assert (
        "--verify-selected '${{ needs.classify-changes.outputs.selected }}'" in ci_text
    )
    assert "--receipt-dir proof-receipts" in ci_text
    assert "actions/download-artifact@v7" in ci_text
    assert "== 'success' && 1 || 0" not in ci_text
    assert ci_text.count("fetch-depth: 0") == 1


def test_llvm_ci_resolves_toolchain_from_manifest_authority() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    perf_text = _read(".github/workflows/perf-gate.yml")
    action_text = _read(".github/actions/setup-llvm/action.yml")

    assert "uses: ./.github/actions/setup-llvm" in ci_text
    assert "uses: ./.github/actions/setup-llvm" in perf_text
    assert "PYTHONPATH=src python3 -m molt.llvm_toolchain" in action_text
    assert '--github-output "$GITHUB_OUTPUT"' in action_text
    assert '--github-env "$GITHUB_ENV"' in action_text
    assert "steps.contract.outputs.apt_packages" in action_text
    assert "steps.contract.outputs.apt_installer_url" in action_text
    assert "steps.contract.outputs.apt_installer_sha256" in action_text
    assert "steps.contract.outputs.wasi_sysroot_url" in action_text
    assert "steps.contract.outputs.wasi_sysroot_sha256" in action_text
    assert "steps.contract.outputs.wasi_sysroot_archive_root" in action_text
    assert 'installer="$RUNNER_TEMP/molt-llvm-apt.sh"' in action_text
    assert "sha256sum --check --strict" in action_text
    assert "wget -qO /tmp" not in action_text
    assert "--verify" in action_text
    assert "--verify-wasm" in action_text
    assert "--wasi-sysroot" in action_text
    assert "Setup canonical WebAssembly linker and WASI sysroot" in ci_text
    assert "profile: wasm" in ci_text
    assert 'wasi: "true"' in ci_text
    assert "grep -oE" not in ci_text
    assert "grep -oE" not in perf_text
    assert "LLVM_SYS_${MAJOR}1_PREFIX" not in ci_text
    assert "LLVM_SYS_${MAJOR}1_PREFIX" not in perf_text


def test_pr_trust_labeler_is_advisory_not_authoritative() -> None:
    labeler_text = _read(".github/workflows/pr_trust_labeler.yml")
    gate_text = _read(".github/workflows/pr_trust_gate.yml")

    assert "Trust gate remains authoritative" in labeler_text
    assert "error?.status === 403" in labeler_text
    assert "core.warning" in labeler_text
    assert "github.rest.issues.addLabels" in labeler_text
    assert "core.setFailed" not in labeler_text
    assert "core.setFailed" in gate_text


def test_kani_runtime_proofs_do_not_compile_full_stdlib_closure() -> None:
    kani_text = _read(".github/workflows/kani.yml")
    runtime_blocks = [
        block
        for block in _named_step_blocks(kani_text)
        if "- name: Run runtime proofs" in block
    ]

    assert len(runtime_blocks) == 1
    runtime_block = runtime_blocks[0]
    assert "cargo kani --tests --no-default-features" in runtime_block
    assert "--all-features" not in runtime_block
    assert "stdlib_full" not in runtime_block


def test_kani_workflow_is_scheduled_manual_and_standalone() -> None:
    kani_text = _read(".github/workflows/kani.yml")

    assert "workflow_dispatch:" in kani_text
    assert "schedule:" in kani_text
    assert "push:" not in kani_text
    assert "pull_request:" not in kani_text
    assert "classify-changes:" not in kani_text
    assert "proof_plan.py" not in kani_text


def test_kani_workflow_gates_verifier_rust_version_honestly() -> None:
    kani_text = _read(".github/workflows/kani.yml")

    assert "Check Kani toolchain compatibility" in kani_text
    assert "id: kani-toolchain" in kani_text
    assert 'workspace_manifest["workspace"]["package"]["rust-version"]' in kani_text
    assert 'runtime" / "molt-obj-model" / "Cargo.toml"' in kani_text
    assert 'runtime" / "molt-runtime" / "Cargo.toml"' in kani_text
    assert "compatible = version_key(kani_rustc) >= version_key(required)" in kani_text
    assert "zero executed proofs is a failure" in kani_text
    assert "outputs.compatible" not in kani_text
    assert "if: always() && steps.kani-toolchain.outcome == 'success'" in kani_text
    assert "Report skipped Kani proofs" not in kani_text
    assert "molt.kani-proof-result.v1" in kani_text
    assert "executed_proof_commands" in kani_text
    assert "if-no-files-found: error" in kani_text
    assert "--ignore-rust-version" not in kani_text


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


def test_executable_proof_workflows_pin_uv_tool_version() -> None:
    for relative in (
        ".github/workflows/ci.yml",
        ".github/workflows/molt-wasm-ci.yml",
        ".github/workflows/security_hardening.yml",
    ):
        text = _read(relative)
        assert text.count("astral-sh/setup-uv@v8.2.0") == text.count(
            'version: "0.11.24"'
        ), relative


def test_executable_receipt_root_is_git_ignored() -> None:
    ignored = {
        line.strip()
        for line in _read(".gitignore").splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    }
    assert "proof-receipts/" in ignored
    for relative in (
        ".github/workflows/ci.yml",
        ".github/workflows/formal.yml",
        ".github/workflows/molt-wasm-ci.yml",
        ".github/workflows/security_hardening.yml",
    ):
        for line in _read(relative).splitlines():
            if "--receipt " in line or "--receipt-dir " in line:
                assert "proof-receipts" in line, (relative, line)


def test_executable_proof_workflows_contain_only_executor_mechanics() -> None:
    allowed_tools = {
        "tools/bootstrap_browser_asset_graph.py",
        "tools/ci_resource_env.py",
        "tools/guarded_exec.py",
        "tools/profile_hotspots.py",
        "tools/proof_plan.py",
    }
    for relative in (
        ".github/workflows/ci.yml",
        ".github/workflows/formal.yml",
        ".github/workflows/molt-wasm-ci.yml",
        ".github/workflows/security_hardening.yml",
    ):
        for line_number, line in enumerate(_read(relative).splitlines(), start=1):
            # Setup/cache inputs may legitimately live under tools/ without
            # executing repository policy. Only Python tool invocations are
            # proof-authority candidates that must route through the plan.
            if "tools/" not in line or ".py" not in line:
                continue
            assert any(tool in line for tool in allowed_tools), (
                relative,
                line_number,
                line,
            )


def test_github_workflows_keep_cargo_target_dirs_cache_stable() -> None:
    unstable_tokens = ("${{ github.run_id }}", "${{ github.run_attempt }}")
    offenders: list[str] = []
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        for line_no, line in enumerate(
            workflow.read_text(encoding="utf-8").splitlines(), start=1
        ):
            if "CARGO_TARGET_DIR" not in line:
                continue
            if any(token in line for token in unstable_tokens):
                rel = workflow.relative_to(REPO_ROOT).as_posix()
                offenders.append(f"{rel}:{line_no}: {line.strip()}")

    assert offenders == []


def test_rust_security_reuses_cached_tool_builds() -> None:
    workflow_text = _read(".github/workflows/security_hardening.yml")
    rust_security = workflow_text.split("  rust-security:", 1)[1]

    assert (
        "CARGO_TARGET_DIR: ${{ github.workspace }}/target/sessions/rust-security"
        in rust_security
    )
    assert (
        "MOLT_SESSION_ID: rust-security-${{ github.run_id }}-${{ github.run_attempt }}"
        in rust_security
    )
    assert "Configure adaptive Rust parallelism" in rust_security
    assert 'python3 tools/ci_resource_env.py --github-env "$GITHUB_ENV"' in (
        rust_security
    )
    assert "uses: Swatinem/rust-cache@v2" in rust_security
    assert 'workspaces: ". -> target/sessions/rust-security"' in rust_security
    assert "cargo install cargo-deny --version 0.20.2 --locked" in rust_security
    assert "cargo install cargo-audit --version 0.22.2 --locked" in rust_security
    assert "rm -rf" not in rust_security
    assert "tmp/security/cargo-audit-advisory-db" in _read("tools/proof_plan.toml")


def test_platform_portability_is_one_generated_cross_os_authority() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))

    assert not (WORKFLOW_ROOT / "proof-queue-portability.yml").exists()
    family = next(
        family
        for family in plan["ci_family"]
        if family["name"] == "platform_portability"
    )
    assert family["executor"] == "github-matrix"
    assert family["workflow"] == ".github/workflows/ci.yml"
    assert family["job"] == "platform-portability"
    assert "fail-fast: false" in ci_text
    assert "runs-on: ${{ matrix.runner }}" in ci_text
    assert "Configure verified ephemeral custody" in ci_text
    assert "MOLT_CI_EPHEMERAL_CUSTODY_ROOT=$custodyRoot" in ci_text
    assert "UV_PROJECT_ENVIRONMENT=$(Join-Path $custodyRoot 'venv')" in ci_text
    assert "$env:RUNNER_TEMP" in ci_text
    assert "${{ runner.temp }}" not in ci_text
    assert "--run-family platform_portability --receipt" in ci_text
    assert '--matrix-cell "${{ matrix.cell }}"' in ci_text
    assert "uv run --active --project . --no-sync python -m pytest" not in ci_text

    cells = {
        cell["id"]: cell
        for cell in plan["matrix_cell"]
        if cell["backend"] == "proof-queue"
    }
    assert {(cell["os"], cell["runner"]) for cell in cells.values()} == {
        ("linux", "ubuntu-latest"),
        ("macos", "macos-14"),
        ("windows", "windows-2022"),
    }
    commands = [
        command
        for command in plan["command"]
        if command["family"] == "platform_portability"
    ]
    assert {command["cell"] for command in commands} == set(cells)
    assert len({tuple(command["argv"]) for command in commands}) == 1


def test_checkouts_drop_persisted_credentials_and_permissions_are_bounded() -> None:
    for workflow in sorted(WORKFLOW_ROOT.glob("*.yml")):
        text = workflow.read_text(encoding="utf-8")
        lines = text.splitlines()
        for index, line in enumerate(lines):
            if "uses: actions/checkout@" not in line:
                continue
            block = "\n".join(lines[index : index + 6])
            assert "persist-credentials: false" in block, workflow

        if workflow.name in {"pr_trust_gate.yml", "pr_trust_labeler.yml"}:
            continue
        assert "\npermissions:\n  contents: read\n" in text, workflow

    release = _read(".github/workflows/release.yml")
    assert release.count("contents: write") == 2
    assert release.count("id-token: write") == 2
    assert release.count("attestations: write") == 2


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
    proof_plan_text = _read("tools/proof_plan.toml")
    assert "rust.clippy.workspace-default" in proof_plan_text
    assert "rust.clippy.feature-surfaces" in proof_plan_text
    assert "--run-family rust --receipt" in ci_text
    assert "continue-on-error" not in ci_text


def test_ci_rust_compile_truth_has_no_redundant_subset_commands() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    commands = {command["id"]: command for command in plan["command"]}
    backend_manifest = tomllib.loads(_read("runtime/molt-backend/Cargo.toml"))

    # Workspace tests compile every package's normal library/bin target before
    # executing test targets, while all-target Clippy covers libs, bins,
    # examples, tests, and benches.  A preceding workspace build/runtime check
    # therefore adds no target or feature coverage and can starve both truths.
    for redundant_id in (
        "rust.check.runtime-default",
        "rust.build.workspace",
        "rust.test.backend-native-feature",
        "rust.clippy.backend-native",
    ):
        assert redundant_id not in commands
    assert "native-backend" in backend_manifest["features"]["default"]
    assert commands["rust.test.default-truth"]["dependencies"] == []
    assert commands["rust.clippy.workspace-default"]["dependencies"] == [
        "rust.test.default-truth"
    ]
    assert commands["rust.clippy.feature-surfaces"]["dependencies"] == [
        "rust.clippy.workspace-default"
    ]
    assert commands["rust.test.default-truth"]["argv"] == [
        "python3",
        "tools/run_cargo_test_truth.py",
    ]
    assert commands["rust.test.default-truth"]["timeout_seconds"] == 1800
    assert commands["rust.clippy.workspace-default"]["argv"] == [
        "cargo",
        "clippy",
        "--locked",
        "--workspace",
        "--all-targets",
        "--",
        "-D",
        "warnings",
    ]
    assert "logs/ci-cargo-build.log" not in ci_text


def test_ci_memory_intensive_steps_use_memory_guard() -> None:
    ci_text = _read(".github/workflows/ci.yml")

    assert "--run-family repository_policy --receipt" in ci_text
    assert "uv run python3 -m pytest -q" not in ci_text
    assert "--run-family rust --receipt" in ci_text
    assert "--run-family llvm --receipt" in ci_text
    proof_executor = _read("tools/proof_plan.py")
    assert "peak_rss_bytes" in proof_executor
    assert "guard_metrics_schema" in proof_executor
    assert "os.killpg" not in proof_executor
    assert "psutil" not in proof_executor
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

    assert "--run-command formal.quint.models --receipt" in formal_workflow
    assert "for model in *.qnt" not in formal_workflow
    assert 'quint verify "$model"' not in formal_workflow
    assert "failed verification (non-blocking)" not in formal_workflow


def test_lean_workflows_share_exact_provisioning_authority() -> None:
    setup_action = _read(".github/actions/setup-lean/action.yml")
    formal_workflow = _read(".github/workflows/formal.yml")
    nightly_workflow = _read(".github/workflows/nightly.yml")

    assert "toolchain=\"$(tr -d '\\r\\n' < formal/lean/lean-toolchain)\"" in (
        setup_action
    )
    probe = 'if ! "$elan" run "$toolchain" lean --version >/dev/null 2>&1; then'
    assert '"$elan" toolchain install "$toolchain"' in setup_action
    assert probe in setup_action
    assert setup_action.index(probe) < setup_action.index(
        '"$elan" toolchain install "$toolchain"'
    )
    assert 'expected_version="${toolchain##*:v}"' in setup_action
    assert '"$exact_version" != "Lean (version $expected_version,"*' in setup_action
    assert '"$selected_version" != "$exact_version"' in setup_action
    assert "key: lean-toolchain-${{ runner.os }}-${{ hashFiles(" in setup_action
    assert formal_workflow.count("uses: ./.github/actions/setup-lean") == 1
    assert nightly_workflow.count("uses: ./.github/actions/setup-lean") == 1
    assert "elan-init.sh" not in formal_workflow
    assert "elan-init.sh" not in nightly_workflow


def test_quint_workflows_pin_patched_node24_toolchain() -> None:
    formal_workflow = _read(".github/workflows/formal.yml")
    nightly_workflow = _read(".github/workflows/nightly.yml")

    assert "actions/setup-node@v6" in formal_workflow
    assert 'node-version: "24.16.0"' in formal_workflow
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

    assert "--run-command formal.lean.build --receipt" in formal_text
    assert "--run-command formal.lean.sorry-baseline --receipt" in formal_text
    assert "--run-command formal.quint.models --receipt" in formal_text
    assert "--run-command formal.correspondence --receipt" in formal_text
    assert "run: lake build" not in formal_text
    assert "run: python3 tools/check_formal_methods.py --quint-only" not in formal_text
    assert (
        "run: python3 tools/check_formal_methods.py --check-correspondence"
        not in formal_text
    )

    assert "--run-family python_security --receipt" in security_text
    assert "--run-family rust_security --receipt" in security_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo install cargo-deny --version 0.20.2 --locked"
    ) in security_text
    assert (
        "python3 tools/guarded_exec.py --prefix MOLT_TEST_SUITE -- "
        "cargo install cargo-audit --version 0.22.2 --locked"
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


def test_security_hardening_is_reusable_and_ci_uses_one_planner() -> None:
    ci_text = _read(".github/workflows/ci.yml")
    security_text = _read(".github/workflows/security_hardening.yml")

    assert "workflow_call:" in security_text
    assert "classify-changes:" not in security_text
    assert "--github-output" not in security_text
    assert "inputs.python_security" in security_text
    assert "inputs.rust_security" in security_text
    assert "uses: ./.github/workflows/security_hardening.yml" in ci_text
    assert ci_text.count('tools/proof_plan.py --github-output "$GITHUB_OUTPUT"') == 1
    assert ci_text.count("tools/proof_plan.py\n          --verify-selected") == 1


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
    assert "bench/scoreboard/logs_*/" in perf_text
    assert "--no-gate" not in perf_text
    assert "--allow-nonauthoritative" not in perf_text
    assert "tools/bench.py" not in perf_text
    assert "bench/results/" not in perf_text
    assert not (WORKFLOW_ROOT / "perf-validation.yml").exists()


def test_perf_demo_workflow_uses_canonical_env_and_single_uv_sync() -> None:
    perf_demo_text = _read(".github/workflows/perf_demo.yml")
    run_stack_text = _read("bench/scripts/run_stack.sh")

    assert "MOLT_SESSION_ID: perf-demo-${{ github.run_id }}" in perf_demo_text
    assert (
        "CARGO_TARGET_DIR: ${{ github.workspace }}/target/sessions/perf-demo"
        in perf_demo_text
    )
    assert (
        "MOLT_DIFF_CARGO_TARGET_DIR: ${{ github.workspace }}/target/sessions/perf-demo"
        in perf_demo_text
    )
    assert "MOLT_CACHE: ${{ github.workspace }}/.molt_cache" in perf_demo_text
    assert "TMPDIR: ${{ github.workspace }}/tmp" in perf_demo_text
    assert "UV_CACHE_DIR: ${{ github.workspace }}/.uv-cache" in perf_demo_text
    assert 'MOLT_UV_SYNC: "0"' in perf_demo_text
    assert 'if [[ "${MOLT_UV_SYNC:-1}" != "0" ]]' in run_stack_text
    assert 'cargo build --profile "$CARGO_PROFILE" -p molt-worker' in run_stack_text
    assert (
        'CARGO_ROOT="${CARGO_TARGET_DIR:-$ROOT/target/sessions/${MOLT_SESSION_ID:-demo-stack}}"'
        in run_stack_text
    )
    assert 'WORKER_BIN="$CARGO_ROOT/$CARGO_PROFILE/molt-worker"' in run_stack_text


def test_wasm_ci_uses_molt_wasm_host_for_imported_modules() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    proof_text = _read("tools/proof_plan.toml")

    assert "--run-family wasm --receipt" in wasm_text
    assert 'id = "wasm.build.host"' in proof_text
    assert 'id = "wasm.run.hello"' in proof_text
    assert 'id = "wasm.run.comprehension"' in proof_text
    assert 'id = "wasm.run.sieve"' in proof_text
    assert "cargo build --profile dev-fast -p molt-wasm-host" not in wasm_text
    assert "wasmtime run /tmp/test_hello.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_comprehension.wasm" not in wasm_text
    assert "wasmtime run /tmp/test_sieve.wasm" not in wasm_text


def test_wasm_ci_uses_canonical_artifact_roots_and_dev_profile() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    wasm_commands = [
        command for command in plan["command"] if command["family"] == "wasm"
    ]

    assert "MOLT_EXT_ROOT: /tmp/molt-ext" in wasm_text
    assert "workflow_call:" in wasm_text
    assert "push:" not in wasm_text
    assert "pull_request:" not in wasm_text
    assert (
        "CARGO_TARGET_DIR: ${{ github.workspace }}/target/sessions/wasm-ci" in wasm_text
    )
    assert (
        "MOLT_DIFF_CARGO_TARGET_DIR: ${{ github.workspace }}/target/sessions/wasm-ci"
        in wasm_text
    )
    assert "MOLT_CACHE: /tmp/molt-ext/molt_cache" in wasm_text
    assert "MOLT_DIFF_ROOT: /tmp/molt-ext/diff" in wasm_text
    assert "MOLT_DIFF_TMPDIR: /tmp/molt-ext/tmp" in wasm_text
    assert "MOLT_WASM_RUNTIME_DIR: /tmp/molt-ext/wasm" in wasm_text
    assert "concurrency:" in wasm_text
    assert "cancel-in-progress: ${{ github.event_name == 'pull_request' }}" in (
        wasm_text
    )
    assert "MOLT_CI_PYTHON" not in wasm_text
    assert (
        "MOLT_WASM_TEST_CARGO_TARGET_DIR: ${{ github.workspace }}/target/sessions/wasm-ci"
        in wasm_text
    )
    assert "enable-cache: true" in wasm_text
    assert "cache-dependency-glob: uv.lock" in wasm_text
    assert (
        "uses: taiki-e/install-action@07b4745e0c39a41822af610387492e3e53aa222b"
        in wasm_text
    )
    assert "tool: wasm-tools@1.253.0" in wasm_text
    assert "fallback: none" in wasm_text
    assert (
        "MOLT_SESSION_ID: wasm-ci-${{ github.run_id }}-${{ github.run_attempt }}"
        in wasm_text
    )
    assert 'MOLT_WASM_TEST_CHILD_RLIMIT_GB: "0"' in wasm_text
    assert 'MOLT_WASM_TEST_KEEPALIVE_SEC: "20"' in wasm_text
    assert 'MOLT_MEMORY_GUARD_TERMINATION_WAIT_SEC: "2"' in wasm_text
    assert 'CARGO_INCREMENTAL: "1"' in wasm_text
    assert 'CARGO_BUILD_JOBS: "1"' not in wasm_text
    assert "Configure adaptive Rust parallelism" in wasm_text
    assert 'python3 tools/ci_resource_env.py --github-env "$GITHUB_ENV"' in wasm_text
    assert "MOLT_WASM_TEST_TIMEOUT_SEC:" not in wasm_text
    assert "MOLT_CARGO_TIMEOUT:" not in wasm_text
    assert "MOLT_BACKEND_DAEMON_SOCKET_DIR" not in wasm_text
    assert "MOLT_BACKEND_DAEMON_CACHE_MB" not in wasm_text
    assert "tools/guarded_exec.py" not in wasm_text
    assert "tools/venv_exec.py" not in wasm_text
    assert "--run-family wasm --receipt" in wasm_text
    assert len(wasm_commands) >= 12
    assert all(command.get("timeout_env") for command in wasm_commands)
    assert all(command["timeout_seconds"] <= 1500 for command in wasm_commands)
    assert any(command["id"] == "wasm.compile.hello" for command in wasm_commands)
    assert any(command["id"] == "wasm.test.control-flow" for command in wasm_commands)
    assert "python3 tools/profile_hotspots.py --limit 20" in wasm_text
    assert "/home/runner/.cache/molt" not in wasm_text


def test_wasm_ci_guarded_steps_have_github_timeout_backstops() -> None:
    wasm_text = _read(".github/workflows/molt-wasm-ci.yml")
    proof_text = _read("tools/proof_plan.py")
    plan = tomllib.loads(_read("tools/proof_plan.toml"))
    wasm_family = next(
        family for family in plan["ci_family"] if family["name"] == "wasm"
    )

    assert f"timeout-minutes: {wasm_family['timeout_minutes']}" in wasm_text
    assert "--timeout" not in wasm_text
    assert "MOLT_CARGO_TIMEOUT:" not in wasm_text
    assert "MOLT_WASM_TEST_TIMEOUT_SEC:" not in wasm_text
    assert '"--timeout",' in proof_text
    assert 'command.data.get("timeout_env", [])' in proof_text
