from __future__ import annotations

from pathlib import Path

from molt.capability_manifest import (
    CapabilityManifest,
    PackageCapabilities,
    load_manifest,
)
from molt.capability_policy import (
    CapabilityPolicy,
    allowed_capabilities_for_package,
    allowed_effects_for_package,
    parse_capability_input,
    parse_capability_policy,
    resolve_capability_policy,
)
from molt._host_capabilities_generated import (
    EXPLICIT_CAPABILITY_TIER,
    MAXIMUM_BUILTIN_CAPABILITY_TIER,
    capabilities_for_tier,
)


def test_manifest_and_cli_share_package_policy_types_and_resolution() -> None:
    data = {
        "capabilities": {
            "allow": ["fs", "future.transport.quic"],
            "deny": ["fs.write"],
            "effects": ["read", "network"],
            "packages": {
                "reader": {
                    "allow": ["fs.read"],
                    "effects": ["read"],
                }
            },
        }
    }

    spec = parse_capability_input(data)
    assert spec.errors == ()
    assert spec.capabilities == ("fs.read", "future.transport.quic")
    assert isinstance(spec.policy, CapabilityPolicy)
    assert isinstance(spec.policy.packages["reader"], PackageCapabilities)

    policy, errors = parse_capability_policy(data)
    assert errors == []
    assert policy == spec.policy
    manifest = CapabilityManifest(
        allow=policy.allow,
        deny=policy.deny,
        effects=policy.effects,
        packages=policy.packages,
    )
    assert manifest.effective_capabilities() == {
        "fs.read",
        "future.transport.quic",
    }


def test_manifest_file_and_cli_file_adapter_share_policy_parser(
    tmp_path: Path,
) -> None:
    path = tmp_path / "capabilities.toml"
    path.write_text(
        """
[manifest]
version = "2.0"

[capabilities]
allow = ["fs"]
deny = ["fs.write"]
effects = ["read"]

[capabilities.packages.reader]
allow = ["fs.read"]
effects = ["read"]
""".strip()
        + "\n",
        encoding="utf-8",
    )

    manifest = load_manifest(path)
    spec = parse_capability_input(str(path))
    assert spec.errors == ()
    assert spec.capabilities == ("fs.read",)
    assert spec.policy is not None
    assert spec.policy.allow == manifest.allow
    assert spec.policy.deny == manifest.deny
    assert spec.policy.effects == manifest.effects
    assert spec.policy.packages == manifest.packages


def test_absent_effect_policy_stays_unconstrained_across_manifest_and_cli(
    tmp_path: Path,
) -> None:
    path = tmp_path / "capabilities.toml"
    path.write_text(
        '[manifest]\nversion = "2.0"\n\n[capabilities]\nallow = []\n',
        encoding="utf-8",
    )
    manifest = load_manifest(path)
    spec = parse_capability_input(str(path))
    assert manifest.effects is None
    assert spec.policy is not None and spec.policy.effects is None
    assert allowed_effects_for_package(manifest, None) is None
    assert manifest.to_env_vars() == {
        "MOLT_CAPABILITIES": "",
        "MOLT_CAPABILITY_TIER": EXPLICIT_CAPABILITY_TIER,
        "MOLT_CAPABILITY_POLICY_DIGEST": manifest.resolved_policy_digest(),
    }


def test_manifest_decode_and_integrity_check_share_one_byte_snapshot(
    tmp_path: Path, monkeypatch
) -> None:
    from molt.capability_manifest import sign_manifest

    path = tmp_path / "capabilities.json"
    unsigned = '{"allow":["fs.read"]}'
    path.write_text(unsigned, encoding="utf-8")
    signature = sign_manifest(path)
    snapshot_a = (
        '{"allow":["fs.read"],"signature":' + repr(signature).replace("'", '"') + "}"
    ).encode()
    snapshot_b = b'{"allow":["net"],"signature":"sha256:' + b"0" * 64 + b'"}'
    path.write_bytes(snapshot_a)

    original_read_bytes = Path.read_bytes
    reads = 0

    def mutate_after_snapshot(candidate: Path) -> bytes:
        nonlocal reads
        payload = original_read_bytes(candidate)
        if candidate == path:
            reads += 1
            path.write_bytes(snapshot_b)
        return payload

    monkeypatch.setattr(Path, "read_bytes", mutate_after_snapshot)
    manifest = load_manifest(path)
    assert reads == 1
    assert manifest.effective_capabilities() == {"fs.read"}


def test_run_cli_and_manifest_grants_form_one_explicit_runtime_policy(
    tmp_path: Path,
) -> None:
    from molt.cli.script_commands import _apply_run_capability_policy

    path = tmp_path / "capabilities.json"
    path.write_text('{"allow":["fs.read"]}', encoding="utf-8")
    env: dict[str, str] = {}
    error = _apply_run_capability_policy(
        env,
        capabilities=["net"],
        capability_manifest=str(path),
        require_signed_manifest=False,
    )
    assert error is None
    assert env["MOLT_CAPABILITY_TIER"] == EXPLICIT_CAPABILITY_TIER
    assert env["MOLT_CAPABILITIES"] == (
        "fs.read,net.asyncio,net.bind,net.connect,net.listen,net.poll,net.resolve,"
        "net.socket,net.socketpair,ssl.connect,ssl.listen,ssl.read,ssl.write,"
        "websocket.connect,websocket.listen"
    )


def test_trusted_tier_composes_with_explicit_grants_and_policy_overrides() -> None:
    from molt.cli.script_commands import _apply_run_capability_policy

    env = {"MOLT_CAPABILITY_TIER": MAXIMUM_BUILTIN_CAPABILITY_TIER}
    error = _apply_run_capability_policy(
        env,
        capabilities=["future.transport.quic"],
        capability_manifest=None,
        require_signed_manifest=False,
        audit_log="jsonl:stderr",
        io_mode="virtual",
    )

    assert error is None
    assert env["MOLT_CAPABILITY_TIER"] == MAXIMUM_BUILTIN_CAPABILITY_TIER
    full_grants = capabilities_for_tier(MAXIMUM_BUILTIN_CAPABILITY_TIER)
    assert full_grants is not None
    assert set(full_grants) < set(env["MOLT_CAPABILITIES"].split(","))
    assert "future.transport.quic" in env["MOLT_CAPABILITIES"].split(",")
    assert env["MOLT_AUDIT_ENABLED"] == "1"
    assert env["MOLT_AUDIT_SINK"] == "jsonl"
    assert env["MOLT_IO_MODE"] == "virtual"
    assert env["MOLT_CAPABILITY_POLICY_DIGEST"].startswith("sha256:")


def test_run_policy_digest_changes_with_cli_runtime_override() -> None:
    from molt.cli.script_commands import _apply_run_capability_policy

    stderr_env: dict[str, str] = {}
    stdout_env: dict[str, str] = {}
    for env, audit_log in (
        (stderr_env, "jsonl:stderr"),
        (stdout_env, "jsonl:stdout"),
    ):
        error = _apply_run_capability_policy(
            env,
            capabilities=["fs.read"],
            capability_manifest=None,
            require_signed_manifest=False,
            audit_log=audit_log,
        )
        assert error is None

    assert (
        stderr_env["MOLT_CAPABILITY_POLICY_DIGEST"]
        != stdout_env["MOLT_CAPABILITY_POLICY_DIGEST"]
    )


def test_omitted_package_allow_inherits_but_empty_allow_denies_all() -> None:
    policy, errors = parse_capability_policy(
        {
            "allow": ["fs"],
            "packages": {
                "inherited": {"deny": ["fs.write"]},
                "isolated": {"allow": []},
            },
        }
    )
    assert errors == []
    assert policy is not None
    resolution = resolve_capability_policy(policy)
    assert resolution.errors == ()
    assert allowed_capabilities_for_package(
        resolution.capabilities, policy, "inherited"
    ) == {"fs.read"}
    assert (
        allowed_capabilities_for_package(resolution.capabilities, policy, "isolated")
        == set()
    )


def test_effect_scoping_intersects_global_and_package_policy() -> None:
    policy, errors = parse_capability_policy(
        {
            "allow": [],
            "effects": ["read", "write"],
            "packages": {
                "reader": {"effects": ["read"]},
                "unscoped": {},
            },
        }
    )
    assert errors == []
    assert policy is not None
    assert allowed_effects_for_package(policy, "reader") == {"read"}
    assert allowed_effects_for_package(policy, "unscoped") == {"read", "write"}


def test_invalid_tokens_and_package_escalation_fail_closed() -> None:
    spec = parse_capability_input(
        {
            "allow": ["fs.read", "INVALID TOKEN"],
            "packages": {"escape": {"allow": ["fs.write"]}},
        }
    )
    assert spec.capabilities is None
    assert "invalid capability token in allow: INVALID TOKEN" in spec.errors
    assert (
        "packages.escape.allow includes capabilities not in global allowlist: fs.write"
        in spec.errors
    )


def test_package_policy_merge_preserves_first_use_order() -> None:
    left = PackageCapabilities(
        name="pkg",
        allow=["fs.read"],
        deny=["net"],
        effects=["read"],
    )
    right = PackageCapabilities(
        name="pkg",
        allow=["fs.read", "env.read"],
        deny=["net", "time.wall"],
        effects=["read", "env"],
    )
    assert left.merged(right) == PackageCapabilities(
        name="pkg",
        allow=["fs.read", "env.read"],
        deny=["net", "time.wall"],
        effects=["read", "env"],
    )


def test_resolved_policy_digest_binds_effects_packages_resources_and_mounts() -> None:
    from molt.capability_manifest import IoConfig, ResourceLimits, VirtualMount

    base = CapabilityManifest(
        allow=["fs"],
        deny=["fs.write"],
        effects=["read", "write"],
        packages={
            "reader": PackageCapabilities(
                name="reader",
                allow=["fs.read"],
                effects=["read"],
            )
        },
        resources=ResourceLimits(max_memory=1024),
        io=IoConfig(
            mode="virtual",
            virtual_mounts=[VirtualMount(path="/tmp", type="memory", max_size=512)],
        ),
    )
    same_semantics = CapabilityManifest(
        allow=["fs.write", "fs.read"],
        deny=["fs.write"],
        effects=["write", "read"],
        packages={
            "reader": PackageCapabilities(
                name="reader",
                allow=["fs.read"],
                effects=["read"],
            )
        },
        resources=ResourceLimits(max_memory=1024),
        io=IoConfig(
            mode="virtual",
            virtual_mounts=[VirtualMount(path="/tmp", type="memory", max_size=512)],
        ),
    )
    assert base.resolved_policy_digest() == same_semantics.resolved_policy_digest()

    changed = CapabilityManifest(
        allow=["fs"],
        deny=["fs.write"],
        effects=["read", "write"],
        packages=base.packages,
        resources=ResourceLimits(max_memory=2048),
        io=base.io,
    )
    assert base.resolved_policy_digest() != changed.resolved_policy_digest()
