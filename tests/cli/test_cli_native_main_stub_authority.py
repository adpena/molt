from __future__ import annotations

import inspect

import molt.cli as cli
from molt.capability_manifest import CapabilityManifest, resolve_runtime_policy_from_env
from molt.cli import native_main_stub

_NATIVE_MAIN_STUB_NAMES = (
    "_native_main_stub_snippets",
    "_render_native_main_stub",
)


def test_cli_native_main_stub_authority_is_single_home() -> None:
    for name in _NATIVE_MAIN_STUB_NAMES:
        assert getattr(cli, name) is getattr(native_main_stub, name)

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
    report = rendered.index("molt_exception_report_uncaught(exc)")
    release = rendered.index("molt_dec_ref_obj(exc)", report)
    shutdown = rendered.index("molt_runtime_exit(exit_code)", release)
    fallback = rendered.index("_Exit(1)", shutdown)
    assert report < release < shutdown < fallback


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
