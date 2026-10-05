from pathlib import Path
import re
import sys
import functools
import json
import os

import pytest

from molt import stdlib_intrinsic_policy
from molt.cli import module_stdlib_policy as cli_module_stdlib_policy
from molt.cli import module_graph_cache
from molt.compiler_analysis.python_imports import UnresolvedStaticImportError
from molt.target_python import (
    SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS,
    _DEFAULT_TARGET_PYTHON_VERSION,
    _parse_target_python_version,
)


@pytest.fixture
def intrinsic_source_cache(tmp_path, monkeypatch):
    identity = {"compiler": "compiler-A"}
    monkeypatch.setattr(
        module_graph_cache, "_build_state_root", lambda _root: tmp_path / "cache"
    )
    monkeypatch.setattr(
        module_graph_cache,
        "_frontend_semantic_tooling_fingerprint",
        lambda: identity["compiler"],
    )
    provider = functools.partial(
        module_graph_cache._stdlib_intrinsic_source_facts,
        tmp_path,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    return provider, identity


def test_intrinsic_source_cache_reuses_analysis_but_resolves_facade_children_live(
    tmp_path, monkeypatch, intrinsic_source_cache
):
    provider, _identity = intrinsic_source_cache
    owner = _write_module(
        tmp_path,
        "owner.py",
        "from _intrinsics import require_intrinsic\nValue = require_intrinsic('molt_x')\n",
    )
    facade = _write_module(
        tmp_path, "_facade.py", "from owner import Value\n__all__ = ('Value',)\n"
    )
    graph = {"owner": owner, "_facade": facade}

    def classify():
        return stdlib_intrinsic_policy.classify_stdlib_module_statuses(
            graph, target_python=_DEFAULT_TARGET_PYTHON_VERSION, facts_provider=provider
        )

    first = classify()
    assert first.statuses["_facade"] == stdlib_intrinsic_policy.STATUS_INTRINSIC_SUPPORT
    monkeypatch.setattr(
        stdlib_intrinsic_policy,
        "stdlib_module_intrinsic_facts",
        lambda *_a, **_kw: pytest.fail("unchanged source was analyzed again"),
    )
    assert classify() == first
    # This real child is a new graph fact; a cached resolved closure would
    # incorrectly keep accepting the facade through its intrinsic owner alone.
    graph["owner.Value"] = tmp_path / "Value.so"
    changed = classify()
    assert changed.statuses["_facade"] == stdlib_intrinsic_policy.STATUS_PYTHON_ONLY
    assert changed.import_evidence["_facade"].facade.owners == frozenset(
        {"owner", "owner.Value"}
    )
    del graph["owner.Value"]
    assert classify() == first


def test_intrinsic_source_cache_checks_bytes_with_restored_mtime(
    tmp_path, intrinsic_source_cache
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(tmp_path, "owner.py", "raise ImportError\n")
    stat = path.stat()
    assert provider("owner", path).status == stdlib_intrinsic_policy.STATUS_POLICY_GATE
    path.write_text("value = 12345678\n ", encoding="utf-8")
    assert path.stat().st_size == stat.st_size
    os.utime(path, ns=(stat.st_atime_ns, stat.st_mtime_ns))
    assert provider("owner", path).status == stdlib_intrinsic_policy.STATUS_PYTHON_ONLY


def test_intrinsic_source_cache_keeps_unresolved_relative_obligations(
    tmp_path, monkeypatch, intrinsic_source_cache
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(
        tmp_path, "owner.py", "__package__ = get_package()\nfrom . import child\n"
    )
    first = provider("pkg.owner", path)
    assert "pkg.child" not in first.import_evidence.proven_modules
    [(line, request, plan)] = first.import_evidence.unresolved_sites
    assert (line, request.level, request.fromlist) == (2, 1, ("child",))
    assert plan.requires_runtime
    monkeypatch.setattr(
        stdlib_intrinsic_policy,
        "stdlib_module_intrinsic_facts",
        lambda *_a, **_kw: pytest.fail("cache miss"),
    )
    assert provider("pkg.owner", path) == first


def test_intrinsic_source_cache_separates_module_target_and_compiler(
    tmp_path, monkeypatch, intrinsic_source_cache
):
    provider, identity = intrinsic_source_cache
    path = _write_module(tmp_path, "owner.py", "from . import child\n")
    assert provider("one.owner", path).import_evidence.proven_modules == frozenset(
        {"one", "one.child"}
    )
    assert provider("two.owner", path).import_evidence.proven_modules == frozenset(
        {"two", "two.child"}
    )
    produce = stdlib_intrinsic_policy.stdlib_module_intrinsic_facts
    observations = []

    def observe(*args, **kwargs):
        observations.append(kwargs["target_python"].tag)
        return produce(*args, **kwargs)

    monkeypatch.setattr(
        stdlib_intrinsic_policy, "stdlib_module_intrinsic_facts", observe
    )
    provider("one.owner", path)
    assert observations == []
    identity["compiler"] = "compiler-B"
    provider("one.owner", path)
    assert len(observations) == 1
    target = _parse_target_python_version("3.13")
    if sys.version_info[:2] < target.feature_version:
        with pytest.raises(UnresolvedStaticImportError, match="requires a Python 3.13"):
            provider("one.owner", path, target_python=target)
    else:
        provider("one.owner", path, target_python=target)
    assert observations[-1] == target.tag


@pytest.mark.parametrize("corruption", ["status", "modules", "unresolved", "facade"])
def test_intrinsic_source_cache_recomputes_malformed_payload(
    tmp_path, intrinsic_source_cache, corruption
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(tmp_path, "owner.py", "VALUE = 1\n")
    first = provider("owner", path)
    [cache] = list((tmp_path / "cache").rglob("*.intrinsic.json"))
    payload = json.loads(cache.read_text(encoding="utf-8"))
    payload["facts"][corruption] = {"poison": True}
    cache.write_text(json.dumps(payload), encoding="utf-8")
    assert provider("owner", path) == first


def test_intrinsic_source_cache_binds_publication_to_captured_bytes(
    tmp_path, monkeypatch, intrinsic_source_cache
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(tmp_path, "owner.py", "raise ImportError\n")
    produce = stdlib_intrinsic_policy.stdlib_module_intrinsic_facts

    def replace_during_analysis(*args, **kwargs):
        path.write_text("VALUE = 1\n", encoding="utf-8")
        return produce(*args, **kwargs)

    monkeypatch.setattr(
        stdlib_intrinsic_policy,
        "stdlib_module_intrinsic_facts",
        replace_during_analysis,
    )
    assert provider("owner", path).status == stdlib_intrinsic_policy.STATUS_POLICY_GATE
    monkeypatch.setattr(
        stdlib_intrinsic_policy, "stdlib_module_intrinsic_facts", produce
    )
    assert provider("owner", path).status == stdlib_intrinsic_policy.STATUS_PYTHON_ONLY


@pytest.mark.parametrize(
    "content",
    [
        b"\xef\xbb\xbfraise ImportError\r\n",
        b"# coding: latin-1\n# caf\xe9\nraise ImportError\n",
    ],
)
def test_intrinsic_source_decoding_is_identical_with_or_without_cache(
    tmp_path, intrinsic_source_cache, content
):
    provider, _identity = intrinsic_source_cache
    path = tmp_path / "owner.py"
    path.write_bytes(content)
    direct = stdlib_intrinsic_policy.stdlib_module_intrinsic_facts(
        "owner", path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert direct.status == stdlib_intrinsic_policy.STATUS_POLICY_GATE
    assert provider("owner", path) == direct
    assert provider("owner", path) == direct


def test_intrinsic_source_cache_publication_failure_is_visible_and_nonfatal(
    tmp_path, monkeypatch, intrinsic_source_cache, capsys
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(tmp_path, "owner.py", "raise ImportError\n")

    def cannot_publish(*_args, **_kwargs):
        raise OSError("read-only cache")

    monkeypatch.setattr(
        module_graph_cache, "_write_artifact_sync_payload", cannot_publish
    )
    counts = {}
    assert (
        provider("owner", path, operation_counts=counts).status
        == stdlib_intrinsic_policy.STATUS_POLICY_GATE
    )
    assert counts == {
        "intrinsic_source_requests": 1,
        "intrinsic_source_misses": 1,
        "intrinsic_source_publication_failures": 1,
    }
    assert "cannot cache intrinsic source facts" in capsys.readouterr().err
    assert list((tmp_path / "cache").rglob("*.intrinsic.json")) == []


def test_intrinsic_source_cache_reports_hit_and_rejected_payload(
    tmp_path, intrinsic_source_cache
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(tmp_path, "owner.py", "raise ImportError\n")
    counts = {}
    provider("owner", path, operation_counts=counts)
    provider("owner", path, operation_counts=counts)
    [cache] = list((tmp_path / "cache").rglob("*.intrinsic.json"))
    payload = json.loads(cache.read_text(encoding="utf-8"))
    payload["facts"]["status"] = stdlib_intrinsic_policy.STATUS_INTRINSIC_SUPPORT
    cache.write_text(json.dumps(payload), encoding="utf-8")
    assert (
        provider("owner", path, operation_counts=counts).status
        == stdlib_intrinsic_policy.STATUS_POLICY_GATE
    )
    assert counts == {
        "intrinsic_source_requests": 3,
        "intrinsic_source_misses": 2,
        "intrinsic_source_hits": 1,
        "intrinsic_source_rejected": 1,
    }


@pytest.mark.parametrize("version", SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS)
def test_runtime_seeded_builtins_keeps_real_intrinsic_policy_evidence(
    version: str,
) -> None:
    root = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    path = root / "builtins.py"
    # These are actual facade operations, not bootstrap self-binding markers.
    assert {
        "molt_compile_builtin",
        "molt_input_builtin",
        "molt_function_set_builtin",
    } == stdlib_intrinsic_policy.module_required_intrinsic_names(path)
    target = _parse_target_python_version(version)
    if sys.version_info[:2] < target.feature_version:
        # Version support is a real frontend capability, not a policy bypass.
        with pytest.raises(
            UnresolvedStaticImportError,
            match=rf"requires a Python {re.escape(version)}\+ frontend",
        ):
            cli_module_stdlib_policy._enforce_intrinsic_stdlib(
                {"builtins": path}, root, json_output=False, target_python=target
            )
        return
    assert (
        cli_module_stdlib_policy._enforce_intrinsic_stdlib(
            {"builtins": path},
            root,
            json_output=False,
            target_python=target,
        )
        is None
    )


def test_builtin_spelling_does_not_exempt_python_only_source(tmp_path: Path) -> None:
    path = _write_module(tmp_path, "builtins.py", "VALUE = 1\n")
    assert stdlib_intrinsic_policy.stdlib_module_intrinsic_status(path) == "python-only"


def _write_module(tmp_path: Path, name: str, source: str) -> Path:
    path = tmp_path / name
    path.write_text(source, encoding="utf-8")
    return path


def test_marker_literal_does_not_count_as_intrinsic_usage(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "marker_only.py",
        '_MOLT_INTRINSIC_MARKER = "molt_capabilities_has"\n',
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "python-only"
    )


def test_require_intrinsic_call_is_intrinsic_backed(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "intrinsic_backed.py",
        (
            "from _intrinsics import require_intrinsic as _require_intrinsic\n"
            '_require_intrinsic("molt_capabilities_has", globals())\n'
        ),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "intrinsic-backed"
    )


def test_probe_only_module_status(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "probe_only.py",
        (
            "from _intrinsics import require_intrinsic as _require_intrinsic\n"
            '_require_intrinsic("molt_stdlib_probe", globals())\n'
        ),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module) == "probe-only"
    )


def test_fail_closed_import_policy_gate_is_not_python_only(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "policy_gate.py",
        (
            '"""namespace reservation"""\n'
            "raise ImportError('not supported; use the explicit adapter')\n"
        ),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "policy-gate"
    )


def test_policy_gate_classifier_rejects_executable_python_body(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "not_policy_gate.py",
        ('"""not a pure gate"""\nVALUE = 1\nraise ImportError(\'not supported\')\n'),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "python-only"
    )


def test_syntax_error_with_intrinsic_marker_is_python_only(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "invalid.py",
        (
            '_MOLT_INTRINSIC_MARKER = "molt_capabilities_has"\n'
            "def broken(:\n"
            "    return 1\n"
        ),
    )
    assert (
        cli_module_stdlib_policy._stdlib_module_intrinsic_status(module)
        == "python-only"
    )


def test_same_package_wrapper_importing_intrinsic_root_is_not_python_only(
    tmp_path: Path,
) -> None:
    stdlib_root = tmp_path / "stdlib"
    package = stdlib_root / "pkg"
    package.mkdir(parents=True)
    root = package / "__init__.py"
    wrapper = package / "widgets.py"
    root.write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_WIDGET_BIND = _require_intrinsic("molt_tk_widget_bind_callback_register")\n',
        encoding="utf-8",
    )
    wrapper.write_text(
        "from . import _WIDGET_BIND\nclass Widget:\n    pass\n",
        encoding="utf-8",
    )

    assert (
        cli_module_stdlib_policy._enforce_intrinsic_stdlib(
            {"pkg": root, "pkg.widgets": wrapper},
            stdlib_root,
            json_output=False,
            target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        )
        is None
    )


def test_private_support_module_loaded_by_intrinsic_owner_is_not_python_only(
    tmp_path: Path,
) -> None:
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    owner = stdlib_root / "_pyio.py"
    support = stdlib_root / "_pyio_text.py"
    owner.write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_READY = _require_intrinsic("molt_import_smoke_runtime_ready")\n'
        "def _load_text_io_classes():\n"
        "    import _pyio_text as text_module\n"
        "    return text_module\n",
        encoding="utf-8",
    )
    support.write_text(
        "class TextIOBase:\n    pass\n",
        encoding="utf-8",
    )

    assert (
        cli_module_stdlib_policy._enforce_intrinsic_stdlib(
            {"_pyio": owner, "_pyio_text": support},
            stdlib_root,
            json_output=False,
            target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        )
        is None
    )


def test_real_weakref_facade_uses_shared_intrinsic_classification() -> None:
    root = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    graph = {
        name: root / f"{name}.py" for name in ("weakref", "_weakref", "_weakrefset")
    }
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        graph, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert classification.statuses == {
        "weakref": stdlib_intrinsic_policy.STATUS_INTRINSIC,
        "_weakref": stdlib_intrinsic_policy.STATUS_INTRINSIC,
        "_weakrefset": stdlib_intrinsic_policy.STATUS_INTRINSIC_SUPPORT,
    }
    assert not stdlib_intrinsic_policy.module_required_intrinsic_names(
        graph["_weakrefset"]
    )
    assert classification.facades_payload()[0]["owners"] == ["weakref"]
    assert (
        cli_module_stdlib_policy._enforce_intrinsic_stdlib(
            graph,
            root,
            json_output=False,
            target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        )
        is None
    )


def test_real_io_wrapper_projects_its_native_provider_without_marker_loads() -> None:
    root = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    graph = {name: root / f"{name}.py" for name in ("io", "_io")}
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        graph, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert classification.statuses == {
        "io": stdlib_intrinsic_policy.STATUS_INTRINSIC,
        "_io": stdlib_intrinsic_policy.STATUS_INTRINSIC,
    }
    assert classification.import_evidence["io"].facade is None
    assert "_io" in classification.import_evidence["io"].proven_modules
    without_provider = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"io": graph["io"]}, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert without_provider.statuses["io"] == stdlib_intrinsic_policy.STATUS_PYTHON_ONLY
    assert not stdlib_intrinsic_policy.module_required_intrinsic_names(graph["io"])
    assert (
        cli_module_stdlib_policy._enforce_intrinsic_stdlib(
            graph, root, json_output=False, target_python=_DEFAULT_TARGET_PYTHON_VERSION
        )
        is None
    )


@pytest.mark.parametrize("owner_source", [None, "class WeakSet: pass\n"])
def test_facade_with_missing_or_python_owner_still_fails_cli_enforcement(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], owner_source: str | None
) -> None:
    facade = _write_module(
        tmp_path, "_facade.py", "from owner import WeakSet\n__all__ = ['WeakSet']\n"
    )
    graph = {"_facade": facade}
    if owner_source is not None:
        graph["owner"] = _write_module(tmp_path, "owner.py", owner_source)
    assert (
        cli_module_stdlib_policy._enforce_intrinsic_stdlib(
            graph,
            tmp_path,
            json_output=False,
            target_python=_DEFAULT_TARGET_PYTHON_VERSION,
        )
        == 2
    )
    assert "_facade" in capsys.readouterr().err


@pytest.mark.parametrize(
    "prefix", ["import os\n", "items.attribute\n", "def load():\n    "]
)
def test_intrinsic_status_never_consumes_unsealed_relative_candidates(tmp_path, prefix):
    owner = _write_module(
        tmp_path,
        "provider.py",
        "from _intrinsics import require_intrinsic\nValue = require_intrinsic('molt_x')\n",
    )
    wrapper = _write_module(
        tmp_path, "wrapper.py", prefix + "from .provider import Value\n"
    )
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"pkg.wrapper": wrapper, "pkg.provider": owner},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    evidence = classification.import_evidence["pkg.wrapper"]
    assert "pkg.provider" not in evidence.proven_modules
    assert evidence.unresolved_sites
    assert (
        classification.statuses["pkg.wrapper"]
        == stdlib_intrinsic_policy.STATUS_PYTHON_ONLY
    )


def test_relative_facade_requires_metadata_proof_for_every_owner(tmp_path):
    provider = _write_module(
        tmp_path,
        "provider.py",
        "from _intrinsics import require_intrinsic\nValue = require_intrinsic('molt_x')\n",
    )
    facade = _write_module(
        tmp_path,
        "facade.py",
        "from .provider import Value\nfrom .provider import Other\n",
    )
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"pkg.facade": facade, "pkg.provider": provider},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    evidence = classification.import_evidence["pkg.facade"]
    assert evidence.facade is not None
    assert not evidence.facade.resolved
    assert evidence.facade.bindings[0].owner_module == "pkg.provider"
    assert evidence.facade.bindings[1].owner_module is None
    assert (
        classification.statuses["pkg.facade"]
        == stdlib_intrinsic_policy.STATUS_PYTHON_ONLY
    )


def test_asyncio_reporting_belongs_to_the_runtime_backed_event_loop_owner():
    root = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    events = root / "asyncio" / "events.py"
    owner = root / "asyncio" / "__init__.py"
    assert not (root / "asyncio" / "_debug.py").exists()
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"asyncio.events": events, "asyncio": owner},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert (
        classification.statuses["asyncio.events"]
        == stdlib_intrinsic_policy.STATUS_INTRINSIC
    )
    assert "asyncio" in classification.import_evidence["asyncio.events"].proven_modules
    assert (
        "molt_event_loop_new"
        in stdlib_intrinsic_policy.module_required_intrinsic_names(owner)
    )
    assert (
        "molt_event_loop_get_exception_handler"
        in stdlib_intrinsic_policy.module_required_intrinsic_names(owner)
    )


def test_python_stream_protocol_does_not_manufacture_an_intrinsic_provider(tmp_path):
    source = _write_module(
        tmp_path,
        "reporting.py",
        "import sys\ndef report(message):\n    sys.stderr.write(message)\n    sys.stderr.flush()\n",
    )
    root = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"pkg.reporting": source, "sys": root / "sys.py"},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert (
        classification.statuses["pkg.reporting"]
        == stdlib_intrinsic_policy.STATUS_PYTHON_ONLY
    )


@pytest.mark.parametrize("provider", ["real", "missing", "python-only"])
def test_real_tk_widgets_require_their_semantic_callable_provider(tmp_path, provider):
    root = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    graph = {
        "tkinter": root / "tkinter" / "__init__.py",
        "tkinter.widgets": root / "tkinter" / "widgets.py",
        "tkinter.constants": root / "tkinter" / "constants.py",
        "_tkinter": root / "_tkinter.py",
    }
    if provider == "real":
        graph["tkinter._support"] = root / "tkinter" / "_support.py"
    elif provider == "python-only":
        graph["tkinter._support"] = _write_module(
            tmp_path, "_support.py", "def _require_tk_callable(name): return None\n"
        )
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        graph,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    evidence = classification.import_evidence["tkinter.widgets"]
    assert "tkinter._support" in evidence.proven_modules
    assert evidence.unresolved_sites  # relative operands still require runtime custody
    assert "tkinter" not in evidence.proven_modules
    assert classification.statuses["tkinter.widgets"] == (
        stdlib_intrinsic_policy.STATUS_INTRINSIC
        if provider == "real"
        else stdlib_intrinsic_policy.STATUS_PYTHON_ONLY
    )
