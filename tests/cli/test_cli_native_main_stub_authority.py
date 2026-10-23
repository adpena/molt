from __future__ import annotations

import inspect
from pathlib import Path
import sys

import pytest

from tests.llvm_sdk_test_support import verified_llvm_tools
from tools.command_execution import CommandExecutor

import molt.cli as cli
from molt.capability_manifest import CapabilityManifest, resolve_runtime_policy_from_env
from molt.cli import native_main_stub

_COMMANDS = CommandExecutor.for_file(__file__)

_NATIVE_MAIN_STUB_NAMES = (
    "_native_main_stub_snippets",
    "_render_native_main_stub",
)


def test_cli_native_main_stub_authority_is_single_home() -> None:
    for name in _NATIVE_MAIN_STUB_NAMES:
        assert hasattr(native_main_stub, name), name
        assert not hasattr(cli, name), name

    cli_source = inspect.getsource(cli)
    for name in _NATIVE_MAIN_STUB_NAMES:
        assert f"def {name}(" not in cli_source


def test_native_main_stub_leaves_the_exit_debug_switch_to_the_runtime() -> None:
    rendered = native_main_stub._render_native_main_stub(
        resolved_capability_policy=CapabilityManifest().resolve(),
    )

    # `molt_main_exit_status` reads MOLT_DEBUG_MAIN_EXCEPTION as a RuntimeFlag.
    assert "MOLT_DEBUG_MAIN_EXCEPTION" not in rendered


def test_native_main_stub_attribute_result_preserves_boxed_width_on_windows() -> None:
    rendered = native_main_stub._render_native_main_stub(
        resolved_capability_policy=CapabilityManifest().resolve(),
    )

    assert (
        "extern unsigned long long molt_get_attr_generic("
        "void* obj, const char* attr, unsigned long long len);"
    ) in rendered
    assert "extern long molt_get_attr_generic(" not in rendered


def test_native_main_stub_explicit_grants_disable_ambient_tier() -> None:
    rendered = native_main_stub._render_native_main_stub(
        resolved_capability_policy=CapabilityManifest(allow=["fs.read"]).resolve(),
    )

    assert '_putenv_s("MOLT_CAPABILITY_TIER", "none")' in rendered
    assert 'setenv("MOLT_CAPABILITY_TIER", "none", 1)' in rendered
    assert '_putenv_s("MOLT_CAPABILITIES", "fs.read")' in rendered


def test_native_main_stub_maximum_tier_freezes_to_exact_finite_grants() -> None:
    rendered = native_main_stub._render_native_main_stub(
        resolved_capability_policy=resolve_runtime_policy_from_env(
            {"MOLT_CAPABILITY_TIER": "full"}
        ),
    )

    assert '_putenv_s("MOLT_CAPABILITY_TIER", "full")' in rendered
    assert 'setenv("MOLT_CAPABILITY_TIER", "full", 1)' in rendered
    assert "net.connect" in rendered
    assert "MOLT_CAPABILITY_POLICY_DIGEST" in rendered
    assert "MOLT_EXECUTION_TARGET" in rendered
    assert "native" in rendered
    assert "MOLT_TRUSTED" not in rendered


def test_native_main_stub_reports_then_exits_through_runtime_custody() -> None:
    rendered = native_main_stub._render_native_main_stub(
        resolved_capability_policy=CapabilityManifest().resolve(),
    )

    # The runtime owns the pending exception, its report and the status; the
    # stub only hands that status to the runtime's exit.
    for retired in (
        "molt_exception_report_uncaught",
        "molt_exception_last_pending()",
        "molt_exception_last()",
        "molt_raise(",
        "molt_dec_ref_obj(",
    ):
        assert retired not in rendered
    shutdown = rendered.index("molt_runtime_exit(molt_main_exit_status());")
    assert rendered.index("_Exit(1)", shutdown) > shutdown


def test_native_main_stub_leaves_python_frame_custody_to_generated_code() -> None:
    rendered = native_main_stub._render_native_main_stub(
        resolved_capability_policy=CapabilityManifest().resolve(),
    )

    for symbol in (
        "molt_frame_push",
        "molt_frame_pop",
        "molt_trace_enter",
        "molt_trace_exit",
    ):
        assert symbol not in rendered


@pytest.mark.slow
@pytest.mark.skipif(
    sys.platform == "win32", reason="POSIX native main execution control"
)
def test_rendered_native_main_hands_the_runtime_status_to_its_exit(
    tmp_path: Path,
) -> None:
    tools = verified_llvm_tools()
    rendered = native_main_stub._render_native_main_stub(
        resolved_capability_policy=CapabilityManifest().resolve(),
    )
    # Compile the actual entrypoint, including its refused-exit fallback.
    runtime = r"""
static int mode, ran, reported, exited;
static void require(int condition) {
    if (!condition) { fputs("exit-contract-failed\n", stderr); _Exit(91); }
}
const unsigned char molt_module_registry_blob[] = {0};
unsigned long long molt_app_resolve_callable(const char* name, unsigned long long len) {
    (void)name; (void)len; return 0;
}
unsigned long long molt_set_app_callable_resolver(unsigned long long fn) {
    (void)fn; return 0;
}
unsigned long long molt_module_registry_install(const unsigned char* blob) {
    (void)blob; return 0;
}
void molt_register_compiled_sources(
    const char* const* paths, unsigned long long count
) {
    (void)paths; (void)count;
}
unsigned long long molt_runtime_init(void) { return 1; }
void molt_runtime_ensure_gil(void) {}
void molt_set_argv(int argc, const char** argv) {
    require(argc == 2); mode = atoi(argv[1]);
}
void molt_main(void) { require(ran == 0); ++ran; }
unsigned long long molt_main_exit_status(void) {
    require(ran == 1 && reported == 0); ++reported; return mode ? 23 : 0;
}
unsigned long long molt_runtime_exit(unsigned long long code) {
    require(reported == 1 && exited == 0 && code == (mode ? 23u : 0u));
    ++exited;
    printf("%d %d %d\n", ran, reported, exited);
    fflush(stdout);
    if (mode == 2) return 0;
    _Exit((int)code);
}
"""
    source = tmp_path / "main.c"
    binary = source.with_suffix("")
    source.write_text(rendered + runtime, encoding="utf-8")
    compiled = _COMMANDS.run(
        [
            tools.clang,
            "-std=c11",
            "-D_POSIX_C_SOURCE=200809L",
            str(source),
            "-o",
            str(binary),
        ],
        env=dict(tools.environment),
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=30,
    )
    assert compiled.returncode == 0, compiled.stdout + compiled.stderr
    # Mode 2: the runtime refuses the exit, and the stub terminates nonzero.
    for mode, exit_code in ((0, 0), (1, 23), (2, 1)):
        result = _COMMANDS.run(
            [str(binary), str(mode)],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=10,
        )
        assert result.returncode == exit_code, result.stdout + result.stderr
        assert result.stdout == "1 1 1\n"
        assert result.stderr == ""
