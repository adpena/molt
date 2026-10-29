from __future__ import annotations

import inspect

import molt.cli as cli
from molt.cli import arg_helpers
from molt.cli import build_output_layout

_BUILD_OUTPUT_LAYOUT_NAMES = (
    "_BUILD_OR_DEPLOY_PROFILE_CHOICES",
    "_BUILD_PROFILE_CHOICES",
    "_DEPLOY_PROFILE_CHOICES",
    "_DEPLOY_PROFILE_DEFAULTS",
    "_OUTPUT_BASE_SAFE_RE",
    "_default_build_base",
    "_default_build_base_cached",
    "artifacts_identity",
    "build_scope",
    "_resolve_build_output_layout",
    "_resolve_cache_root",
    "_resolve_cache_root_cached",
    "_resolve_out_dir",
    "_resolve_out_dir_cached",
    "_resolve_output_path",
    "_resolve_output_roots",
    "_resolve_sysroot",
    "_resolve_sysroot_cached",
    "_safe_output_base",
    "_wasm_runtime_root",
    "_wasm_runtime_root_from_env",
)

_BUILD_OUTPUT_LAYOUT_DEFINITIONS = (
    "_OUTPUT_BASE_SAFE_RE =",
    "_DEPLOY_PROFILE_DEFAULTS: dict",
    "_BUILD_PROFILE_CHOICES =",
    "def _safe_output_base(",
    "def _wasm_runtime_root(",
    "def _wasm_runtime_root_from_env(",
    "def _default_build_base_cached(",
    "def _default_build_base(",
    "def artifacts_identity(",
    "def build_scope(",
    "def _resolve_cache_root_cached(",
    "def _resolve_cache_root(",
    "def _resolve_out_dir_cached(",
    "def _resolve_out_dir(",
    "def _resolve_sysroot_cached(",
    "def _resolve_sysroot(",
    "def _resolve_output_roots(",
    "def _resolve_output_path(",
    "def _resolve_build_output_layout(",
)


def test_cli_build_output_layout_authority_is_single_home() -> None:
    for name in _BUILD_OUTPUT_LAYOUT_NAMES:
        assert hasattr(build_output_layout, name)
        assert not hasattr(cli, name)

    cli_source = inspect.getsource(cli)
    for marker in _BUILD_OUTPUT_LAYOUT_DEFINITIONS:
        assert marker not in cli_source


def test_arg_helpers_use_build_output_layout_authority_directly() -> None:
    source = inspect.getsource(arg_helpers)
    assert "cli._BUILD_PROFILE_CHOICES" not in source
    assert "from molt.cli.build_output_layout import _BUILD_PROFILE_CHOICES" in source


def test_build_scope_owns_cached_managed_target_and_control(tmp_path, monkeypatch):
    from molt import checkout_state, dx
    from molt.cli import runtime_paths

    source = tmp_path / "source"
    source.mkdir()
    ext = tmp_path / "artifacts"
    target = dx.cargo_target_dir_for_artifact_root(ext, "reader", source_root=source)
    target.mkdir(parents=True)
    monkeypatch.setenv("MOLT_EXT_ROOT", str(ext))
    monkeypatch.setenv("CARGO_TARGET_DIR", str(target))
    monkeypatch.delenv("MOLT_BUILD_STATE_DIR", raising=False)
    # Prime selectors outside the build: both cached-return paths must acquire
    # a new per-build hold, rather than rely on selector execution/mtime.
    assert runtime_paths._cargo_target_root(source) == target
    control = runtime_paths._build_state_root(source)
    control.mkdir(parents=True, exist_ok=True)
    with build_output_layout.build_scope():
        assert runtime_paths._cargo_target_root(source) == target
        assert runtime_paths._build_state_root(source) == control
        with checkout_state.checkout_state_retirement(target.parent.parent) as allowed:
            assert allowed is False
        with checkout_state.checkout_state_retirement(control) as allowed:
            assert allowed is False
    with checkout_state.checkout_state_retirement(target.parent.parent) as allowed:
        assert allowed is True
    with checkout_state.checkout_state_retirement(control) as allowed:
        assert allowed is True


def test_build_scope_retains_initial_maintenance_choice_as_guard_default_drifts(
    tmp_path, monkeypatch, developer_host_context, unbound_checkout_family
):
    import os
    from molt import dx
    from molt.disk_capacity import DiskCapacityProbe
    from molt.process_environment import ProcessEnvironment
    from molt.source_root import compiler_source_root
    from tools.harness_memory_guard import canonical_harness_env

    monkeypatch.delenv("MOLT_EXT_ROOT", raising=False)
    first, later = tmp_path / "first", tmp_path / "later"
    first.mkdir()
    later.mkdir()
    monkeypatch.setenv(
        "MOLT_EXTERNAL_ARTIFACT_ROOTS", os.pathsep.join((str(first), str(later)))
    )
    monkeypatch.setenv("MOLT_PREFER_EXTERNAL_ARTIFACTS", "1")
    first_ready = True

    def capacity(path, *, required_bytes):
        ready = path != first or first_ready
        return DiskCapacityProbe(
            path, path, required_bytes + 1 if ready else 0, required_bytes
        )

    monkeypatch.setattr(dx, "probe_capacity", capacity)
    captured = ProcessEnvironment(os.environ)
    expected = canonical_harness_env(captured, repo_root=compiler_source_root())
    assert expected["MOLT_EXT_ROOT"] == str(first)
    cwd = os.getcwd()
    with build_output_layout.build_scope() as scope:
        assert scope.maintenance_environment is not None
        assert (
            scope.maintenance_environment["MOLT_EXT_ROOT"] == expected["MOLT_EXT_ROOT"]
        )
        first_ready = False
        payload = canonical_harness_env(captured, repo_root=compiler_source_root())
        assert payload["MOLT_EXT_ROOT"] == str(later)
        monkeypatch.setenv("MOLT_EXT_ROOT", str(later))
        monkeypatch.chdir(tmp_path)
    assert str(scope.cwd) == cwd
    assert scope.maintenance_environment["MOLT_EXT_ROOT"] == expected["MOLT_EXT_ROOT"]
    assert dx.configured_artifact_root(
        scope.maintenance_environment, relative_to=scope.cwd
    )
