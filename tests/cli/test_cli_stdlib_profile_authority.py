"""Pin the single config authority for ``stdlib_profile``.

``stdlib_profile`` is the user-facing runtime stdlib intent. ``auto`` means the
backend selects the smallest concrete runtime tier whose Cargo feature ceiling
contains the reached intrinsic feature set; named tiers are explicit ceilings.
It used to be resolved at several independent sites that each carried their own
literal ``"micro"`` default. Those defaults could desync: the module-graph
closure readers read
``MOLT_STDLIB_PROFILE`` to decide which modules enter the dependency closure,
while the runtime-staticlib selector consumes the resolved value to decide which
prebuilt archive to link. When the two disagree (env-only ``full`` pulling
crypto modules into the closure while a ``micro`` staticlib is linked) the build
fails at link time on undefined full-profile intrinsics.

These tests pin the consolidated state:

* one default constant, one legal-values tuple, one resolver;
* the documented precedence (flag > env > config > deploy-profile > default)
  with provenance;
* every fallback in the build pipeline references the single constant, so a new
  independent ``"micro"`` literal default cannot be reintroduced without failing
  this gate.
"""

from __future__ import annotations

import inspect
import re
from pathlib import Path

from molt.cli.config_resolution import (
    AUTO_STDLIB_PROFILE,
    DEFAULT_RUNTIME_STDLIB_PROFILE,
    DEFAULT_STDLIB_PROFILE,
    MOLT_STDLIB_PROFILE_ENV,
    RUNTIME_STDLIB_PROFILE_TIERS,
    STDLIB_PROFILE_CHOICES,
    resolve_stdlib_profile,
)

_CLI_DIR = Path(__file__).resolve().parents[2] / "src" / "molt" / "cli"


def test_single_default_and_choices() -> None:
    assert DEFAULT_STDLIB_PROFILE == AUTO_STDLIB_PROFILE
    assert DEFAULT_RUNTIME_STDLIB_PROFILE == "micro"
    assert DEFAULT_STDLIB_PROFILE in STDLIB_PROFILE_CHOICES
    assert STDLIB_PROFILE_CHOICES == (
        AUTO_STDLIB_PROFILE,
        *RUNTIME_STDLIB_PROFILE_TIERS,
    )
    assert MOLT_STDLIB_PROFILE_ENV == "MOLT_STDLIB_PROFILE"


def test_resolver_default_when_nothing_set() -> None:
    value, source = resolve_stdlib_profile(flag=None, build_cfg={}, env={})
    assert value == DEFAULT_STDLIB_PROFILE
    assert source == "default"


def test_resolver_precedence_flag_over_everything() -> None:
    value, source = resolve_stdlib_profile(
        flag="full",
        build_cfg={"stdlib_profile": "micro"},
        deploy_defaults={"stdlib_profile": "micro"},
        env={MOLT_STDLIB_PROFILE_ENV: "micro"},
    )
    assert (value, source) == ("full", "flag")


def test_resolver_precedence_env_over_config_and_deploy() -> None:
    value, source = resolve_stdlib_profile(
        flag=None,
        build_cfg={"stdlib_profile": "micro"},
        deploy_defaults={"stdlib_profile": "micro"},
        env={MOLT_STDLIB_PROFILE_ENV: "full"},
    )
    assert (value, source) == ("full", "env")


def test_resolver_precedence_config_over_deploy_and_default() -> None:
    # Both the hyphen and underscore spellings are honored.
    for key in ("stdlib-profile", "stdlib_profile"):
        value, source = resolve_stdlib_profile(
            flag=None,
            build_cfg={key: "full"},
            deploy_defaults={"stdlib_profile": "micro"},
            env={},
        )
        assert (value, source) == ("full", "config"), key


def test_resolver_precedence_deploy_over_default() -> None:
    value, source = resolve_stdlib_profile(
        flag=None,
        build_cfg={},
        deploy_defaults={"stdlib_profile": "full"},
        env={},
    )
    assert (value, source) == ("full", "deploy-profile")


def test_resolver_ignores_invalid_values_at_each_layer() -> None:
    # An invalid flag falls through to the env layer.
    value, source = resolve_stdlib_profile(
        flag="bogus", build_cfg={}, env={MOLT_STDLIB_PROFILE_ENV: "full"}
    )
    assert (value, source) == ("full", "env")
    # An invalid env value falls through to config/default.
    value, source = resolve_stdlib_profile(
        flag=None,
        build_cfg={"stdlib_profile": "full"},
        env={MOLT_STDLIB_PROFILE_ENV: "bogus"},
    )
    assert (value, source) == ("full", "config")
    # An invalid config value falls through to the default.
    value, source = resolve_stdlib_profile(
        flag=None, build_cfg={"stdlib_profile": "bogus"}, env={}
    )
    assert (value, source) == (DEFAULT_STDLIB_PROFILE, "default")


def test_arg_env_default_agree_when_all_request_the_same_profile() -> None:
    """The three input lanes must converge on one value, never desync."""

    for requested in STDLIB_PROFILE_CHOICES:
        by_flag, _ = resolve_stdlib_profile(flag=requested, build_cfg={}, env={})
        by_env, _ = resolve_stdlib_profile(
            flag=None, build_cfg={}, env={MOLT_STDLIB_PROFILE_ENV: requested}
        )
        by_cfg, _ = resolve_stdlib_profile(
            flag=None, build_cfg={"stdlib_profile": requested}, env={}
        )
        assert by_flag == by_env == by_cfg == requested


def test_auto_intent_keeps_micro_core_until_runtime_tier_selection() -> None:
    """The graph reader receives user intent, while artifacts use concrete tiers."""

    from molt.cli import (
        backend_cache_setup,
        backend_compile,
        backend_output_pipeline,
        link_pipeline,
        module_graph,
        module_stdlib_policy,
        runtime_build,
        runtime_intrinsic_symbols,
        runtime_paths,
    )

    # Closure reader A: module_stdlib_policy core-module selection.
    assert module_stdlib_policy._core_stdlib_module_names_for_profile(
        None
    ) == module_stdlib_policy._core_stdlib_module_names_for_profile(
        DEFAULT_STDLIB_PROFILE
    )
    # Raw artifact helpers operate on concrete runtime tiers; absent a selected
    # tier, their safe concrete fallback is micro, not the user-facing auto
    # intent.
    assert module_stdlib_policy.DEFAULT_STDLIB_PROFILE is DEFAULT_STDLIB_PROFILE
    assert module_graph.DEFAULT_STDLIB_PROFILE is DEFAULT_STDLIB_PROFILE
    assert runtime_paths._normalize_runtime_stdlib_profile(None) == "micro"
    assert module_stdlib_policy._core_stdlib_module_names_for_profile(
        "auto"
    ) == module_stdlib_policy._core_stdlib_module_names_for_profile("micro")
    lower_artifact_functions = [
        backend_cache_setup._build_cache_variant,
        backend_cache_setup._prepare_backend_cache_setup,
        backend_compile._prepare_backend_setup,
        backend_compile._prepare_backend_runtime_context,
        backend_output_pipeline._emit_backend_pipeline_outputs,
        link_pipeline._prepare_native_link,
        runtime_build._initialize_runtime_artifact_state,
        runtime_build._maybe_start_native_runtime_lib_ready_async,
        runtime_build._ensure_runtime_lib_ready,
        runtime_build._ensure_native_runtime_lib_ready_before_link,
        runtime_build._ensure_runtime_lib,
        runtime_build._ensure_runtime_wasm_artifact,
        runtime_build._ensure_runtime_wasm,
        runtime_intrinsic_symbols._stage_runtime_intrinsic_symbols_for_native_codegen,
    ]
    for func in lower_artifact_functions:
        default = inspect.signature(func).parameters["stdlib_profile"].default
        assert default == DEFAULT_RUNTIME_STDLIB_PROFILE, func.__name__


def test_build_reexports_resolved_env_before_module_graph(monkeypatch) -> None:
    """``build()`` called with ``stdlib_profile=None`` must resolve through the
    authority and re-export the resolved value to ``MOLT_STDLIB_PROFILE`` BEFORE
    the module-graph closure reads it, so the closure reader and the
    runtime-staticlib selector observe the same profile.

    We capture the env at ``_prepare_build_inputs`` — the first build step, run
    before module-graph construction — and short-circuit there to avoid the
    heavy compile."""

    import os

    import molt.cli as cli

    for env_value, expected in (("full", "full"), (None, DEFAULT_STDLIB_PROFILE)):
        if env_value is None:
            monkeypatch.delenv(MOLT_STDLIB_PROFILE_ENV, raising=False)
        else:
            monkeypatch.setenv(MOLT_STDLIB_PROFILE_ENV, env_value)

        captured: dict[str, object] = {}

        def fake_prepare(*args, **kwargs):
            # The env the module-graph closure reader will observe.
            captured["env"] = os.environ.get(MOLT_STDLIB_PROFILE_ENV)
            return None, 0  # (no inputs, sentinel error) -> build() returns early

        monkeypatch.setattr(cli._build_inputs, "_prepare_build_inputs", fake_prepare)
        cli.build("examples/hello.py", stdlib_profile=None)

        assert captured.get("env") == expected


def test_no_independent_micro_literal_default_in_cli() -> None:
    """Source gate: no module may reintroduce an independent ``"micro"`` literal
    as a stdlib_profile default or env fallback. Every fallback must reference
    DEFAULT_STDLIB_PROFILE so the resolver stays the one authority.

    ``config_resolution.py`` owns user-facing defaults. ``runtime_paths.py`` is
    allowed one concrete-artifact fallback because raw archive helpers cannot
    name the user-facing ``auto`` intent."""

    # A kwarg default: ``stdlib_profile: ... = "micro"`` or
    # ``stdlib_profile="micro"`` — but NOT an equality comparison ``== "micro"``
    # (those are legitimate branches on the resolved value).
    literal_kwarg_default = re.compile(
        r'stdlib_profile(?:\s*:[^\n=]*)?\s*(?<!=)=(?!=)\s*"micro"'
    )
    # A ``... or "micro"`` fallback.
    literal_or = re.compile(r'stdlib_profile\s+or\s+"micro"')
    # ``params.get("stdlib_profile", "micro")`` dict-default fallback.
    literal_get_default = re.compile(r'"stdlib_profile"\s*,\s*"micro"')
    # ``os.environ.get("MOLT_STDLIB_PROFILE", "micro")`` env fallback.
    literal_env_get = re.compile(r'"MOLT_STDLIB_PROFILE"\s*,\s*"micro"')
    patterns = (
        literal_kwarg_default,
        literal_or,
        literal_get_default,
        literal_env_get,
    )
    offenders: list[str] = []
    allowed = {"config_resolution.py", "runtime_paths.py"}
    for path in sorted(_CLI_DIR.glob("*.py")):
        if path.name in allowed:
            continue
        text = path.read_text(encoding="utf-8")
        if any(p.search(text) for p in patterns):
            offenders.append(path.name)
    assert not offenders, (
        "These CLI modules reintroduced an independent 'micro' literal for "
        "stdlib_profile; reference DEFAULT_STDLIB_PROFILE instead: "
        + ", ".join(offenders)
    )
