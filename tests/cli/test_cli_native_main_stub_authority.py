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


def test_native_main_stub_uses_warning_free_windows_env_probe() -> None:
    rendered = native_main_stub._render_native_main_stub(
        resolved_capability_policy=CapabilityManifest().resolve(),
    )

    assert "_dupenv_s(&value, &value_len, name)" in rendered
    assert 'getenv("MOLT_DEBUG_MAIN_EXCEPTION")' not in rendered


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

    assert "molt_exception_report_uncaught(exc)" in rendered
    assert "molt_raise(exc)" not in rendered
    assert "molt_exception_last()" not in rendered
    acquire = rendered.index("exc = molt_exception_last_pending()")
    report = rendered.index("molt_exception_report_uncaught(exc)", acquire)
    release = rendered.index("molt_dec_ref_obj(exc)", report)
    shutdown = rendered.index("molt_runtime_exit(exit_code)", release)
    fallback = rendered.index("_Exit(1)", shutdown)
    assert acquire < report < release < shutdown < fallback


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
def test_rendered_native_main_reports_owned_raised_exception(tmp_path: Path) -> None:
    tools = verified_llvm_tools()
    rendered = native_main_stub._render_native_main_stub(
        resolved_capability_policy=CapabilityManifest().resolve(),
    )
    # Distinct raised and handled owners make the old observer visibly wrong.
    # Compile the actual entrypoint, including its shutdown-return fallbacks.
    runtime = r"""
static int mode, acquired, legacy, reported, released, exited;
static unsigned long long pending;
static void require(int condition) {
    if (!condition) { fputs("owner-contract-failed\n", stderr); _Exit(91); }
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
unsigned long long molt_runtime_init(void) { return 1; }
void molt_runtime_ensure_gil(void) {}
void molt_set_argv(int argc, const char** argv) {
    require(argc == 2); mode = atoi(argv[1]); pending = mode != 0;
}
void molt_main(void) {}
unsigned long long molt_exception_pending(void) { return pending; }
unsigned long long molt_exception_last_pending(void) {
    require(pending && acquired == 0); ++acquired; return 101;
}
unsigned long long molt_exception_last(void) {
    ++legacy; pending = 0; return 202;
}
unsigned long long molt_exception_report_uncaught(unsigned long long owner) {
    require(owner == 101 && pending && acquired == 1 && reported == 0);
    ++reported; return 23;
}
void molt_dec_ref_obj(unsigned long long owner) {
    require(owner == 101 && reported == 1 && released == 0); ++released;
}
unsigned long long molt_runtime_exit(unsigned long long code) {
    require(legacy == 0 && exited == 0);
    require(mode ? (acquired == 1 && reported == 1 && released == 1 && code == 23)
                 : (acquired == 0 && reported == 0 && released == 0 && code == 0));
    ++exited;
    printf("%d %d %d %d %d\n", acquired, legacy, reported, released, exited);
    fflush(stdout);
    if (mode == 2) return 0;
    _Exit((int)code);
}
"""
    for old_observer in (False, True):
        source = tmp_path / f"main-{old_observer}.c"
        binary = source.with_suffix("")
        entrypoint = rendered
        if old_observer:
            entrypoint = entrypoint.replace(
                "molt_exception_last_pending()", "molt_exception_last()"
            )
        source.write_text(entrypoint + runtime, encoding="utf-8")
        compiled = _COMMANDS.run(
            [
                tools.clang,
                "-std=c11",
                "-D_POSIX_C_SOURCE=200809L",
                str(source),
                "-o",
                str(binary),
            ],
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=30,
        )
        assert compiled.returncode == 0, compiled.stdout + compiled.stderr
        cases = (
            ((1, 91, ""),)
            if old_observer
            else (
                (0, 0, "0 0 0 0 1\n"),
                (1, 23, "1 0 1 1 1\n"),
                (2, 1, "1 0 1 1 1\n"),
            )
        )
        for mode, exit_code, output in cases:
            result = _COMMANDS.run(
                [str(binary), str(mode)],
                capture_output=True,
                text=True,
                encoding="utf-8",
                timeout=10,
            )
            assert result.returncode == exit_code, result.stdout + result.stderr
            assert result.stdout == output
            assert result.stderr == ("owner-contract-failed\n" if old_observer else "")
