"""Cross-language drift and fail-closed teeth for host capabilities."""

from __future__ import annotations

import importlib
import importlib.util
import json
import re
from pathlib import Path
from types import ModuleType

import pytest

ROOT = Path(__file__).resolve().parents[1]


def _gen():
    return importlib.import_module("tools.gen_host_capabilities")


def _generated() -> ModuleType:
    path = _gen().OUT_PYTHON
    spec = importlib.util.spec_from_file_location("_molt_host_capabilities_test", path)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def test_generated_outputs_are_byte_exact() -> None:
    gen = _gen()
    for path, expected in gen.render_all(gen.load_schema()).items():
        assert path.read_bytes() == expected.encode("utf-8"), (
            f"{path.relative_to(ROOT)} is stale; run "
            "`python tools/gen_host_capabilities.py`"
        )


def test_explicit_policy_tier_is_ambientless_and_tiers_are_monotone() -> None:
    schema = _gen().load_schema()
    generated = _generated()
    assert generated.EXPLICIT_CAPABILITY_TIER == "none"
    assert generated.capabilities_for_tier("none") == ()
    assert generated.capabilities_for_tier("not-a-tier") is None
    tiers = {tier.name: set(tier.effective_grants) for tier in schema.tiers}
    assert tiers["none"] < tiers["safe"] < tiers["standard"] < tiers["full"]
    assert set(generated.CapabilityId) == {
        generated.CapabilityId(item) for item in tiers["full"]
    }


def test_typed_operation_requirements_and_target_gates_are_generated() -> None:
    generated = _generated()
    assert generated.capabilities_for_operation(
        generated.OperationId.THREAD_SPAWN_SHARED
    ) == (
        generated.CapabilityId.THREAD_SPAWN,
        generated.CapabilityId.THREAD_SHARED,
    )
    assert generated.capabilities_for_operation(
        generated.OperationId.SSL_HANDSHAKE_SERVER
    ) == (generated.CapabilityId.SSL_LISTEN,)
    assert generated.capabilities_for_operation(
        generated.OperationId.RANDOM_ENTROPY
    ) == (generated.CapabilityId.RANDOM,)
    assert generated.OPERATION_PLATFORMS[generated.OperationId.SELECT_EPOLL] == (
        "linux",
    )
    assert generated.OPERATION_TARGETS[generated.OperationId.SELECT_EPOLL] == (
        "native",
    )
    assert generated.operation_supports_target(
        generated.OperationId.SELECT_EPOLL,
        target="native",
        platform="linux",
        architecture="x86_64",
        python_version="3.12",
    )
    assert not generated.operation_supports_target(
        generated.OperationId.SELECT_EPOLL,
        target="browser",
        platform="wasi",
        architecture="wasm32",
        python_version="3.12",
    )
    assert generated.operation_supports_target(
        generated.OperationId.SELECT_POLL,
        target="browser",
        platform="wasi",
        architecture="wasm32",
        python_version="3.14",
    )
    assert not generated.operation_supports_target(
        generated.OperationId.SELECT_POLL,
        target="unknown",
        platform="linux",
        architecture="x86_64",
        python_version="3.12",
    )
    assert not generated.operation_supports_target(
        generated.OperationId.SELECT_POLL,
        target="native",
        platform="linux",
        architecture="x86_64",
        python_version="3.15",
    )
    assert generated.OperationId.SELECT_POLL not in generated.OPERATION_PLATFORMS


def test_cpython_fallback_and_generated_tiers_resolve_identically(monkeypatch) -> None:
    from molt import capabilities as runtime_capabilities

    env = {
        "MOLT_CAPABILITY_TIER": "none",
        "MOLT_CAPABILITIES": "fs.read,future.transport.quic",
    }
    monkeypatch.setattr(
        runtime_capabilities,
        "_env_get",
        lambda key, default="": env.get(key, default),
    )
    assert runtime_capabilities.capabilities() == {
        "fs.read",
        "future.transport.quic",
    }
    env["MOLT_CAPABILITY_TIER"] = "not-a-tier"
    assert runtime_capabilities.capabilities() == {
        "fs.read",
        "future.transport.quic",
    }


def test_profile_and_runtime_consumers_have_no_handwritten_grant_tables() -> None:
    policy = (ROOT / "src/molt/capability_policy.py").read_text(encoding="utf-8")
    runtime = (
        ROOT / "runtime/molt-runtime/src/async_rt/channels/capabilities.rs"
    ).read_text(encoding="utf-8")
    browser = (ROOT / "wasm/browser_host.js").read_text(encoding="utf-8")
    wasi = (ROOT / "wasm/run_wasm.js").read_text(encoding="utf-8")
    assert "from molt._host_capabilities_generated import CAPABILITY_PROFILES" in policy
    assert "const TIER_SAFE" not in runtime
    assert "grants_for_tier" in runtime
    assert "EXPLICIT_CAPABILITY_TIER" in browser
    assert ": 'full'" not in browser
    assert "wasiEnvMap.set('MOLT_EXECUTION_TARGET', 'browser')" in browser
    assert "wasmEnv.MOLT_EXECUTION_TARGET = 'wasi'" in wasi


def test_all_runtime_entropy_sources_use_generated_operation_authority() -> None:
    paths = {
        "os": ROOT / "runtime/molt-runtime/src/builtins/io_path.rs",
        "hash": ROOT / "runtime/molt-runtime/src/object/ops_hash.rs",
        "crypto_host": ROOT / "runtime/molt-runtime/src/crypto_bridge.rs",
        "math_host": ROOT / "runtime/molt-runtime/src/math_bridge.rs",
        "secrets": ROOT / "runtime/molt-runtime-crypto/src/secrets.rs",
        "random": ROOT / "runtime/molt-runtime-math/src/random_mod.rs",
    }
    sources = {name: path.read_text(encoding="utf-8") for name, path in paths.items()}

    assert "OperationId::RandomEntropy" in sources["os"]
    assert "OperationId::RandomEntropy" in sources["hash"]
    assert "OperationId::RandomEntropy" in sources["crypto_host"]
    assert "OperationId::RandomEntropy" in sources["math_host"]
    assert "require_random_entropy(_py)" in sources["secrets"]
    assert "require_random_entropy(_py)" in sources["random"]
    random_new = sources["random"].split('pub extern "C" fn molt_random_new', 1)[1]
    random_new = random_new.split('pub extern "C" fn molt_random_seed', 1)[0]
    assert "Fallback: single-word 0 seed" not in random_new


def test_literal_runtime_audit_relations_are_registry_owned() -> None:
    generated = _generated()
    pattern = re.compile(
        r'audit_capability_decision\(\s*"(?P<operation>[^"]+)"\s*,'
        r'\s*"(?P<capability>[^"]+)"',
        re.DOTALL,
    )
    observed: dict[str, set[str]] = {}
    for path in (ROOT / "runtime/molt-runtime/src").rglob("*.rs"):
        for match in pattern.finditer(path.read_text(encoding="utf-8")):
            observed.setdefault(match["operation"], set()).add(match["capability"])

    registered = {
        operation.value: {capability.value for capability in capabilities}
        for operation, capabilities in generated.OPERATION_CAPABILITIES.items()
    }
    assert set(observed) <= set(registered)
    for operation, capabilities in observed.items():
        assert capabilities == registered[operation]


def test_generated_split_worker_is_default_deny() -> None:
    from molt.cli.wasm import _generate_split_worker_js
    from molt.capability_manifest import CapabilityManifest

    source = _generate_split_worker_js(
        resolved_capability_policy=CapabilityManifest().resolve(),
        shared_memory_initial_pages=1,
        shared_table_initial=1,
        shared_table_base=None,
    )
    assert '"MOLT_CAPABILITY_TIER=none"' in source
    assert "MOLT_TRUSTED" not in source


def test_generated_split_worker_embeds_complete_resolved_policy() -> None:
    from molt.capability_manifest import AuditConfig, CapabilityManifest, ResourceLimits
    from molt.cli.wasm import _generate_split_worker_js

    policy = CapabilityManifest(
        allow=["net.connect"],
        resources=ResourceLimits(
            max_memory=1_048_576,
            max_duration=1.25,
            max_allocations=4_096,
            max_recursion_depth=256,
            max_pow_result=8_192,
        ),
        audit=AuditConfig(enabled=True, sink="jsonl", output="stdout"),
    ).resolve(tier="safe")
    expected_env = [
        f"{name}={value}"
        for name, value in sorted(
            {**policy.to_env_vars(), "MOLT_EXECUTION_TARGET": "cloudflare"}.items()
        )
    ]

    source = _generate_split_worker_js(
        resolved_capability_policy=policy,
        shared_memory_initial_pages=1,
        shared_table_initial=1,
        shared_table_base=None,
    )

    assert json.dumps(expected_env, ensure_ascii=True) in source
    assert '"MOLT_CAPABILITY_TIER=safe"' in source
    assert '"MOLT_EXECUTION_TARGET=cloudflare"' in source
    assert '"MOLT_CAPABILITIES=' in source
    assert "net.connect" in source
    assert '"MOLT_AUDIT_ENABLED=1"' in source
    assert '"MOLT_AUDIT_SINK=jsonl"' in source
    assert '"MOLT_AUDIT_OUTPUT=stdout"' in source
    assert '"MOLT_RESOURCE_MAX_MEMORY=1048576"' in source
    assert '"MOLT_RESOURCE_MAX_DURATION_MS=1250"' in source
    assert '"MOLT_RESOURCE_MAX_ALLOCATIONS=4096"' in source
    assert '"MOLT_RESOURCE_MAX_RECURSION_DEPTH=256"' in source
    assert '"MOLT_RESOURCE_MAX_POW_RESULT=8192"' in source
    assert f'"MOLT_CAPABILITY_POLICY_DIGEST={policy.digest()}"' in source


def test_schema_rejects_forward_tier_inheritance_and_ambient_explicit_tier(
    tmp_path: Path,
) -> None:
    gen = _gen()
    source = gen.SOURCE.read_text(encoding="utf-8")
    forward = tmp_path / "forward.toml"
    forward.write_text(
        source.replace('inherits = ["none"]', 'inherits = ["full"]', 1),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="inherit only earlier tiers"):
        gen.load_schema(forward)

    ambient = tmp_path / "ambient.toml"
    ambient.write_text(
        source.replace(
            'explicit_policy_tier = "none"', 'explicit_policy_tier = "safe"'
        ),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="must grant no ambient"):
        gen.load_schema(ambient)


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("targets", "unknown"),
        ("platforms", "plan9"),
        ("architectures", "mips"),
        ("python_versions", "3.15"),
    ],
)
def test_schema_rejects_unknown_target_vocabulary(
    tmp_path: Path, field: str, value: str
) -> None:
    gen = _gen()
    source = gen.SOURCE.read_text(encoding="utf-8")
    invalid = tmp_path / "invalid-vocabulary.toml"
    invalid.write_text(
        source.replace(
            'capabilities = ["net.socket"]',
            f'capabilities = ["net.socket"]\n{field} = ["{value}"]',
            1,
        ),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="contains unsupported values"):
        gen.load_schema(invalid)
