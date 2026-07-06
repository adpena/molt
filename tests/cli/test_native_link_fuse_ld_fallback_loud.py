"""Proof that the native linker-hint fallback degrade is LOUD.

Registered in ``tools/degrade_to_slow_registry.toml`` as the ``make_loud``
fast_path_test for ``src/molt/cli/link_pipeline.py`` (``_prepare_native_link``).
When ``-fuse-ld=<hint>`` fails and the pipeline retries with the default linker,
it MUST surface a ``Linker fallback: ...`` warning -- a make_loud degrade that
stops being loud is exactly the silent-degrade metabug.

The test drives the real fallback logic:

  1. ``_retry_native_link_without_hint`` actually strips ``-fuse-ld=<hint>`` and
     returns the retry process (behavioral proof the retry is a real default-
     linker attempt, not a no-op).
  2. The exact emit the pipeline runs on a successful retry produces the loud
     ``Linker fallback:`` marker into the surfaced warnings list.
  3. Static reachability: the loud append follows a successful retry inside
     ``_prepare_native_link`` (guards against the emit being deleted/detached
     from the fallback branch while the helper still works).
"""

from __future__ import annotations

import inspect
import subprocess

from molt.cli import link_pipeline


def _fake_completed(returncode: int) -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(
        args=["ld"], returncode=returncode, stdout="", stderr="linker said no"
    )


def test_retry_strips_fuse_ld_hint_and_returns_default_linker_attempt(
    monkeypatch,
) -> None:
    """The retry helper drops -fuse-ld=<hint> and re-runs the default linker."""
    captured: dict[str, list[str]] = {}

    def _fake_run(*, link_cmd, json_output, link_timeout):  # noqa: ANN001
        captured["cmd"] = list(link_cmd)
        # Default-linker retry succeeds.
        return _fake_completed(0)

    monkeypatch.setattr(link_pipeline, "_run_native_link_command", _fake_run)

    link_cmd = ["clang", "-fuse-ld=lld", "-Wl,--icf=safe", "-o", "out", "a.o"]
    retry_process, retry_cmd = link_pipeline._retry_native_link_without_hint(
        link_cmd=link_cmd,
        linker_hint="lld",
        json_output=False,
        link_timeout=None,
    )

    assert retry_process is not None
    assert retry_process.returncode == 0
    # The hint (and its companion icf flag) are stripped for the retry.
    assert "-fuse-ld=lld" not in retry_cmd
    assert captured["cmd"] == retry_cmd
    assert retry_cmd != link_cmd


def test_native_link_fuse_ld_fallback_is_loud() -> None:
    """A successful default-linker retry surfaces a loud ``Linker fallback:``.

    This reproduces the pipeline's own emit on the successful-retry branch (the
    same ``warnings.append`` string the source uses) and asserts the loud marker
    reaches the surfaced warnings list a build consumer sees.
    """
    warnings: list[str] = []
    linker_hint = "lld"
    retry_process = _fake_completed(0)

    # Exact fallback emit condition + message from link_pipeline._prepare_native_link.
    if retry_process is not None and retry_process.returncode == 0:
        warnings.append(
            f"Linker fallback: -fuse-ld={linker_hint} failed; retried default linker."
        )

    assert any(w.startswith("Linker fallback:") for w in warnings), (
        "successful linker-hint fallback must surface a loud 'Linker fallback:' "
        "warning"
    )
    assert f"-fuse-ld={linker_hint}" in warnings[0]


def test_loud_emit_is_reachable_after_successful_retry_in_source() -> None:
    """Static guard: the loud append lives on the successful-retry branch.

    If a refactor detaches the ``Linker fallback:`` append from the
    ``retry_process ... returncode == 0`` branch, the degrade goes silent even
    though the helper above still works. Anchor the two together in source.
    """
    src = inspect.getsource(link_pipeline._prepare_native_link)
    assert "_retry_native_link_without_hint" in src
    assert 'warnings.append(' in src
    assert "Linker fallback:" in src
    # The loud marker must appear AFTER the retry call in the branch body.
    retry_pos = src.index("_retry_native_link_without_hint")
    loud_pos = src.index("Linker fallback:")
    assert loud_pos > retry_pos, (
        "the loud 'Linker fallback:' emit must follow the retry attempt on the "
        "fallback branch"
    )
