from __future__ import annotations

import inspect

import molt.cli as cli
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
        trusted=False,
        capabilities_list=None,
    )

    assert "_dupenv_s(&value, &value_len, name)" in rendered
    assert 'getenv("MOLT_DEBUG_MAIN_EXCEPTION")' not in rendered


def test_native_main_stub_reports_then_exits_through_runtime_custody() -> None:
    rendered = native_main_stub._render_native_main_stub(
        trusted=False,
        capabilities_list=None,
    )

    assert "molt_exception_report_uncaught(exc)" in rendered
    assert "molt_raise(exc)" not in rendered
    report = rendered.index("molt_exception_report_uncaught(exc)")
    frame_pop = rendered.index("molt_frame_pop()", report)
    release = rendered.index("molt_dec_ref_obj(exc)", frame_pop)
    shutdown = rendered.index("molt_runtime_exit(exit_code)", release)
    fallback = rendered.index("_Exit(1)", shutdown)
    assert report < frame_pop < release < shutdown < fallback
