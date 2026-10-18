from pathlib import Path
import re
import sys
import functools
import os

import pytest

from molt import stdlib_intrinsic_policy
from molt.cli import module_graph_cache
from molt.cli.cache_fingerprints import _source_tree_fingerprint_transaction
from molt.cli.source_record_store import SourceRecordStore
from molt.compiler_analysis.python_imports import UnresolvedStaticImportError
from molt.target_python import (
    SUPPORTED_TARGET_PYTHON_SHORT_VERSIONS,
    _DEFAULT_TARGET_PYTHON_VERSION,
    _parse_target_python_version,
)
from tests.source_record_test_support import (
    delete_source_record,
    replace_source_record,
    source_records,
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
    analyzed = []
    original = stdlib_intrinsic_policy.stdlib_module_intrinsic_facts

    def record(module, *args, **kwargs):
        analyzed.append(module)
        return original(module, *args, **kwargs)

    monkeypatch.setattr(
        stdlib_intrinsic_policy, "stdlib_module_intrinsic_facts", record
    )
    assert classify() == first
    assert not analyzed
    # This real child is a new graph fact; a cached resolved closure would
    # incorrectly keep accepting the facade through its intrinsic owner alone.
    graph["owner.Value"] = tmp_path / "Value.so"
    changed = classify()
    # Importing a new child writes the parent's namespace. Its changed premise
    # must invalidate the owning fact even though the source bytes are stable.
    assert analyzed == ["owner"]
    assert changed.statuses["_facade"] == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    assert changed.import_evidence["_facade"].facade.owners == frozenset(
        {"owner", "owner.Value"}
    )
    del graph["owner.Value"]
    assert classify() == first
    # Intrinsic metadata retains one current premise; returning to an older
    # graph must rebuild its projection rather than reuse the changed one.
    assert analyzed == ["owner", "owner"]


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
    assert (
        provider("owner", path).status == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    )


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


@pytest.mark.parametrize(
    "corruption",
    [
        "status",
        "sites",
        "private_imports",
        "used",
        "unread",
        "discarded",
        "facade",
    ],
)
def test_intrinsic_source_cache_recomputes_malformed_payload(
    tmp_path, intrinsic_source_cache, corruption
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(tmp_path, "owner.py", "VALUE = 1\n")
    first = provider("owner", path)
    [(key, payload)] = source_records(tmp_path, "intrinsic-facts").items()
    kind = "intrinsic-facts"
    if corruption == "sites":
        kind = "import-scan"
        payload = source_records(tmp_path, kind)[key]
        payload["source_sites"] = {"poison": True}
    else:
        payload["facts"][corruption] = {"poison": True}
    replace_source_record(tmp_path, kind, key, payload)
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
    assert (
        provider("owner", path).status == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    )


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
        return False

    monkeypatch.setattr(SourceRecordStore, "put", cannot_publish)
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
    assert source_records(tmp_path, "intrinsic-facts") == {}


def test_intrinsic_source_cache_reports_hit_and_rejected_payload(
    tmp_path, intrinsic_source_cache
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(tmp_path, "owner.py", "raise ImportError\n")
    counts = {}
    provider("owner", path, operation_counts=counts)
    provider("owner", path, operation_counts=counts)
    [(key, payload)] = source_records(tmp_path, "intrinsic-facts").items()
    payload["facts"]["status"] = stdlib_intrinsic_policy.STATUS_INTRINSIC_SUPPORT
    replace_source_record(tmp_path, "intrinsic-facts", key, payload)
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
            stdlib_intrinsic_policy.classify_stdlib_module_statuses(
                {"builtins": path}, target_python=target
            )
        return
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"builtins": path}, target_python=target
    )
    assert classification.statuses == {
        "builtins": stdlib_intrinsic_policy.STATUS_INTRINSIC
    }
    assert classification.used_intrinsics[
        "builtins"
    ] == stdlib_intrinsic_policy.module_required_intrinsic_names(path)


def _status(path: Path) -> str:
    return stdlib_intrinsic_policy.stdlib_module_intrinsic_facts(
        path.stem, path, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    ).status


def test_builtin_spelling_does_not_exempt_compiled_python_source(
    tmp_path: Path,
) -> None:
    path = _write_module(tmp_path, "builtins.py", "VALUE = 1\n")
    assert _status(path) == "python-compiled"


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
    assert _status(module) == "python-compiled"


def test_read_intrinsic_is_intrinsic_backed(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "intrinsic_backed.py",
        (
            "from _intrinsics import require_intrinsic as _require_intrinsic\n"
            '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")\n'
            "def has(name):\n"
            "    return _MOLT_CAPABILITIES_HAS(name)\n"
        ),
    )
    assert _status(module) == "intrinsic-backed"


@pytest.mark.parametrize(
    "requirement",
    [
        '_require_intrinsic("molt_capabilities_has", globals())\n',
        '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")\n',
        'def ensure():\n    _require_intrinsic("molt_capabilities_has")\n',
    ],
    ids=["discarded", "unread-binding", "discarded-in-function"],
)
def test_unread_requirement_is_not_intrinsic_backing(
    tmp_path: Path, requirement: str
) -> None:
    module = _write_module(
        tmp_path,
        "anchored.py",
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        + requirement,
    )
    facts = stdlib_intrinsic_policy.stdlib_module_intrinsic_facts(
        "anchored", module, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert facts.status == "python-compiled"
    assert facts.intrinsic_use.used == frozenset()
    unread = [binding.intrinsic for binding in facts.intrinsic_use.unread_bindings]
    unread += [name for name, _line in facts.intrinsic_use.discarded]
    assert unread == ["molt_capabilities_has"]


def test_generated_stub_has_its_own_status(tmp_path: Path) -> None:
    marker = stdlib_intrinsic_policy.STDLIB_STUB_MARKER
    module = _write_module(
        tmp_path,
        "missing.py",
        f"def __getattr__(attr):\n    raise RuntimeError('stdlib module \"missing\" {marker}')\n",
    )
    assert _status(module) == "stub"


def test_fail_closed_import_policy_gate_is_not_compiled_python(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "policy_gate.py",
        (
            '"""namespace reservation"""\n'
            "raise ImportError('not supported; use the explicit adapter')\n"
        ),
    )
    assert _status(module) == "policy-gate"


def test_policy_gate_classifier_rejects_executable_python_body(tmp_path: Path) -> None:
    module = _write_module(
        tmp_path,
        "not_policy_gate.py",
        ('"""not a pure gate"""\nVALUE = 1\nraise ImportError(\'not supported\')\n'),
    )
    assert _status(module) == "python-compiled"


def test_syntax_error_with_intrinsic_marker_is_an_analysis_failure(
    tmp_path: Path,
) -> None:
    module = _write_module(
        tmp_path,
        "invalid.py",
        (
            '_MOLT_INTRINSIC_MARKER = "molt_capabilities_has"\n'
            "def broken(:\n"
            "    return 1\n"
        ),
    )
    with pytest.raises(UnresolvedStaticImportError, match="cannot parse source"):
        _status(module)


def test_same_package_wrapper_reads_the_package_intrinsic_binding(
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

    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"pkg": root, "pkg.widgets": wrapper},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    # The package never loads its binding; the sibling's import is the read.
    assert classification.statuses == {
        "pkg": stdlib_intrinsic_policy.STATUS_INTRINSIC,
        "pkg.widgets": stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED,
    }
    assert classification.used_intrinsics["pkg"] == {
        "molt_tk_widget_bind_callback_register"
    }
    assert dict(classification.unused_bindings) == {}
    # Without the importing sibling, nothing reads the binding.
    alone = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"pkg": root}, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert alone.statuses["pkg"] == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    assert [binding.name for binding in alone.unused_bindings["pkg"]] == [
        "_WIDGET_BIND"
    ]


def test_private_support_module_loaded_by_intrinsic_owner_is_support(
    tmp_path: Path,
) -> None:
    stdlib_root = tmp_path / "stdlib"
    stdlib_root.mkdir()
    owner = stdlib_root / "_pyio.py"
    support = stdlib_root / "_pyio_text.py"
    owner.write_text(
        "from _intrinsics import require_intrinsic as _require_intrinsic\n"
        '_MOLT_CAPABILITIES_HAS = _require_intrinsic("molt_capabilities_has")\n'
        "def _has(name):\n"
        "    return _MOLT_CAPABILITIES_HAS(name)\n"
        "def _load_text_io_classes():\n"
        "    import _pyio_text as text_module\n"
        "    return text_module\n",
        encoding="utf-8",
    )
    support.write_text(
        "class TextIOBase:\n    pass\n",
        encoding="utf-8",
    )

    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"_pyio": owner, "_pyio_text": support},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    assert classification.statuses == {
        "_pyio": stdlib_intrinsic_policy.STATUS_INTRINSIC,
        "_pyio_text": stdlib_intrinsic_policy.STATUS_INTRINSIC_SUPPORT,
    }


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


def test_real_io_wrapper_keeps_python_ownership_without_marker_loads() -> None:
    root = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    graph = {name: root / f"{name}.py" for name in ("io", "_io")}
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        graph, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert classification.statuses == {
        "io": stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED,
        "_io": stdlib_intrinsic_policy.STATUS_INTRINSIC,
    }
    assert classification.import_evidence["io"].facade is None
    assert "_io" in classification.import_evidence["io"].proven_modules
    without_provider = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"io": graph["io"]}, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert (
        without_provider.statuses["io"]
        == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    )
    assert not stdlib_intrinsic_policy.module_required_intrinsic_names(graph["io"])


@pytest.mark.parametrize("owner_source", [None, "class WeakSet: pass\n"])
def test_facade_with_missing_or_python_owner_is_not_intrinsic_support(
    tmp_path: Path, owner_source: str | None
) -> None:
    facade = _write_module(
        tmp_path, "_facade.py", "from owner import WeakSet\n__all__ = ['WeakSet']\n"
    )
    graph = {"_facade": facade}
    if owner_source is not None:
        graph["owner"] = _write_module(tmp_path, "owner.py", owner_source)
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        graph, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    assert (
        classification.statuses["_facade"]
        == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    )
    assert classification.facades_payload()[0]["reason"] is None


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
    writer = _write_module(
        tmp_path,
        "writer.py",
        "import pkg.wrapper as target\ndef mutate(value): target.__package__ = value\n",
    )
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"pkg.wrapper": wrapper, "pkg.provider": owner, "writer": writer},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    evidence = classification.import_evidence["pkg.wrapper"]
    assert "pkg.provider" not in evidence.proven_modules
    assert evidence.unresolved_sites
    assert (
        classification.statuses["pkg.wrapper"]
        == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    )


def test_relative_import_requires_metadata_proof_at_every_site(tmp_path):
    provider = _write_module(
        tmp_path,
        "provider.py",
        "from _intrinsics import require_intrinsic\nValue = require_intrinsic('molt_x')\n",
    )
    facade = _write_module(
        tmp_path,
        "facade.py",
        "from .provider import Value\n__package__ = choose_package()\nfrom .provider import Other\n",
    )
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        {"pkg.facade": facade, "pkg.provider": provider},
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    evidence = classification.import_evidence["pkg.facade"]
    # The explicit mutation makes this executable Python rather than a pure
    # facade, but does not erase the earlier proven import owner.
    assert evidence.facade is None
    first, second = evidence.sites
    assert first.owner_plan.modules == ("pkg.provider",)
    assert not first.unresolved
    assert second.unresolved and second.owner_plan.requires_runtime
    assert (
        classification.statuses["pkg.facade"]
        == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    )


def test_asyncio_reporting_keeps_python_ownership_with_native_event_loop():
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
        == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
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
        == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    )


@pytest.mark.parametrize("provider", ["real", "missing", "python-compiled"])
def test_real_tk_widgets_keep_python_ownership_across_callable_providers(
    tmp_path, provider
):
    root = Path(__file__).resolve().parents[2] / "src" / "molt" / "stdlib"
    graph = {
        "tkinter": root / "tkinter" / "__init__.py",
        "tkinter.widgets": root / "tkinter" / "widgets.py",
        "tkinter.constants": root / "tkinter" / "constants.py",
        "_tkinter": root / "_tkinter.py",
    }
    if provider == "real":
        graph["tkinter._support"] = root / "tkinter" / "_support.py"
    elif provider == "python-compiled":
        graph["tkinter._support"] = _write_module(
            tmp_path, "_support.py", "def _require_tk_callable(name): return None\n"
        )
    classification = stdlib_intrinsic_policy.classify_stdlib_module_statuses(
        graph,
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )
    evidence = classification.import_evidence["tkinter.widgets"]
    assert "tkinter._support" in evidence.proven_modules
    # The admitted graph has no foreign package-metadata writer. Literal
    # relative imports therefore retain their proven owners across callbacks.
    assert not evidence.unresolved_sites
    assert "tkinter" in evidence.proven_modules
    assert (
        classification.statuses["tkinter.widgets"]
        == stdlib_intrinsic_policy.STATUS_PYTHON_COMPILED
    )
    if provider == "real":
        assert (
            classification.statuses["tkinter._support"]
            == stdlib_intrinsic_policy.STATUS_INTRINSIC
        )


def test_warm_intrinsic_source_facts_runs_workers_only_for_a_large_set(
    tmp_path, monkeypatch
):
    import molt.host_capacity

    # Spawned workers inherit the environment, not this process's patches.
    monkeypatch.setenv("MOLT_BUILD_STATE_DIR", str(tmp_path / "state"))
    monkeypatch.setattr(molt.host_capacity, "worker_count", lambda **_: 2)
    real_pool = module_graph_cache.ProcessPoolExecutor

    def modules(prefix: str, count: int) -> dict[str, Path]:
        return {
            f"{prefix}{index}": _write_module(
                tmp_path, f"{prefix}{index}.py", f"VALUE = {index}\n"
            )
            for index in range(count)
        }

    per_worker = module_graph_cache._INTRINSIC_FACTS_MODULES_PER_WORKER
    monkeypatch.setattr(
        module_graph_cache,
        "ProcessPoolExecutor",
        lambda **_: pytest.fail("a set one worker covers must stay serial"),
    )
    module_graph_cache.warm_stdlib_intrinsic_source_facts(
        tmp_path,
        modules("small", 2 * per_worker - 1),
        target_python=_DEFAULT_TARGET_PYTHON_VERSION,
    )

    monkeypatch.setattr(module_graph_cache, "ProcessPoolExecutor", real_pool)
    large = modules("large", 2 * per_worker)
    module_graph_cache.warm_stdlib_intrinsic_source_facts(
        tmp_path, large, target_python=_DEFAULT_TARGET_PYTHON_VERSION
    )
    counts: dict[str, int] = {}
    with _source_tree_fingerprint_transaction():
        for name, path in large.items():
            module_graph_cache._stdlib_intrinsic_source_facts(
                tmp_path,
                name,
                path,
                target_python=_DEFAULT_TARGET_PYTHON_VERSION,
                operation_counts=counts,
            )
    assert counts == {
        "intrinsic_source_requests": 2 * per_worker,
        "intrinsic_source_hits": 2 * per_worker,
    }


def test_intrinsic_source_cache_roundtrips_owner_plans(
    tmp_path, intrinsic_source_cache
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(
        tmp_path,
        "consumer.py",
        "try: from pkg import Value\nexcept ImportError: pass\n"
        "try:\n    __import__(unknown_name)\nexcept ImportError:\n    pass\n",
    )
    first = provider("consumer", path)
    second = provider("consumer", path)
    assert second == first
    statement, opaque = second.import_evidence.sites
    assert statement.line == 1
    assert statement.owner_plan.modules == ("pkg",)
    assert statement.plan.modules == ("pkg", "pkg.Value")
    assert opaque.request is None and opaque.unresolved
    assert opaque.line == 4
    [payload] = source_records(tmp_path, "intrinsic-facts").values()
    assert payload["identity"]["schema"] == "molt.stdlib-intrinsic-source.v8"
    assert "modules" not in payload["facts"] and "unresolved" not in payload["facts"]
    assert "blockers" not in payload["facts"]


@pytest.mark.parametrize(
    "corruption",
    [
        "missing-owner-plan",
        "request-level-bool",
        "runtime-int",
        "unknown-error",
        "retired-handlers",
        "retired-owner",
        "ordinary-owner-mismatch",
        "opaque-no-obligation",
    ],
)
def test_intrinsic_source_cache_rejects_malformed_site_facts(
    tmp_path, intrinsic_source_cache, corruption
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(
        tmp_path,
        "consumer.py",
        "try:\n    import pkg\nexcept ImportError:\n    pass\n"
        "__import__(unknown_name)\n",
    )
    first = provider("consumer", path)
    [key] = source_records(tmp_path, "intrinsic-facts")
    payload = source_records(tmp_path, "import-scan")[key]
    site = payload["source_sites"][0]
    if corruption == "missing-owner-plan":
        del site["owner_plan"]
    elif corruption == "request-level-bool":
        site["request"]["level"] = True
    elif corruption == "runtime-int":
        site["plan"]["runtime"] = 0
    elif corruption == "unknown-error":
        site["plan"]["errors"] = ["invented"]
    elif corruption == "retired-handlers":
        site["handlers"] = []
    elif corruption == "retired-owner":
        site["owner"] = None
    elif corruption == "ordinary-owner-mismatch":
        site["owner_plan"]["modules"] = ["other"]
    elif corruption == "opaque-no-obligation":
        site = payload["source_sites"][1]
        site["plan"]["runtime"] = False
    replace_source_record(tmp_path, "import-scan", key, payload)
    assert provider("consumer", path) == first


def test_intrinsic_source_cache_invalidates_request_after_same_size_edit(
    tmp_path, intrinsic_source_cache
):
    import os

    provider, _identity = intrinsic_source_cache
    path = _write_module(
        tmp_path,
        "consumer.py",
        "try:\n    import pkg\nexcept ImportError:\n    pass\n",
    )
    first = provider("consumer", path)
    timestamp = path.stat()
    before = path.read_bytes()
    replacement = b"try:\n    import new\nexcept ImportError:\n    pass\n"
    replacement += b" " * (len(before) - len(replacement))
    assert len(replacement) == len(before)
    path.write_bytes(replacement)
    os.utime(path, ns=(timestamp.st_atime_ns, timestamp.st_mtime_ns))
    second = provider("consumer", path)
    assert first.import_evidence.sites[0].plan.modules == ("pkg",)
    assert second.import_evidence.sites[0].plan.modules == ("new",)


def test_intrinsic_source_cache_retains_call_request_metadata(
    tmp_path, intrinsic_source_cache
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(
        tmp_path,
        "consumer.py",
        "__import__('child', {'__package__': 'pkg'}, fromlist=('Value',), level=1)\n",
    )
    first = provider("consumer", path)
    second = provider("consumer", path)
    assert first == second
    [site] = second.import_evidence.sites
    assert site.request.kind == "dunder_import"
    assert site.request.globals_were_supplied
    assert site.request.globals_state.package.value == "pkg"
    assert site.request.level == 1 and site.request.fromlist == ("Value",)
    assert site.owner_plan.modules == ("pkg.child",)
    assert site.plan.modules == ("pkg.child", "pkg.child.Value")


@pytest.mark.parametrize(
    "corruption",
    [
        "retired-effects",
        "demand-phase",
        "demand-width",
        "export-status",
        "absent-with-names",
        "known-with-reason",
        "absent-with-reason",
        "source-span",
        "missing-source-record",
    ],
)
def test_shared_source_cache_rejects_malformed_execution_and_export_facts(
    tmp_path, intrinsic_source_cache, corruption
):
    provider, _identity = intrinsic_source_cache
    path = _write_module(
        tmp_path,
        "consumer.py",
        "__all__ = ['value']\ndef f():\n    _MOLT_OP()\ntry:\n    f()\nexcept Exception:\n    pass\n",
    )
    first = provider("consumer", path)
    [(key, metadata)] = source_records(tmp_path, "intrinsic-facts").items()
    assert "sites" not in metadata["facts"]
    payload = source_records(tmp_path, "import-scan")[key]
    if corruption == "retired-effects":
        payload["source_effects"] = []
    elif corruption == "demand-phase":
        payload["source_demands"][0][1] = "invented"
    elif corruption == "demand-width":
        payload["source_demands"][0].append("stale")
    elif corruption == "export-status":
        payload["all_exports"]["status"] = "invented"
    elif corruption == "absent-with-names":
        payload["all_exports"]["status"] = "absent"
        payload["all_exports"]["names"] = ["forged"]
    elif corruption in {"known-with-reason", "absent-with-reason"}:
        payload["all_exports"] = {
            "status": corruption.split("-", 1)[0],
            "names": [],
            "reason": "not actually proved",
        }
    elif corruption == "source-span":
        payload["source_demands"][0][0][0] = -1
    if corruption == "missing-source-record":
        delete_source_record(tmp_path, "import-scan", key)
    else:
        replace_source_record(tmp_path, "import-scan", key, payload)
    assert provider("consumer", path) == first
