#!/usr/bin/env python3
"""Report and refresh pinned upstream releases.

Owner rule: every dependency, toolchain and tool runs at its newest stable
release, pinned exactly (version plus digest). This tool compares each pin,
read through its one authority's loader, with its upstream:

  --check        report every pin that trails its upstream's newest stable
                 release; exit 1 when any unheld pin does (network; the
                 scheduled `security.pin-freshness` proof command runs it)
  --update NAME  move one `config/tool_releases.toml` tool to its newest
                 release. Every asset is downloaded and accepted only when
                 its SHA-256 matches the upstream's own record (the GitHub
                 release asset digest, or the release's SHASUMS256.txt) and
                 the archive holds the pinned member; the tool's proof-plan
                 toolchain policy moves with it.

A pin may trail upstream only under a hold that names an open row of the
findings ledger; a hold on a current pin, or on a closed row, is an error.
Pins outside `config/tool_releases.toml` move at their own authority. No
third-party packages; set GITHUB_TOKEN to raise the GitHub API rate limit.
"""

from __future__ import annotations

import argparse
import hashlib
import io
import json
import os
import re
import sys
import tarfile
import tomllib
import urllib.request
import zipfile
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from pathlib import Path
from typing import TextIO

from molt import tool_releases
from molt.binaryen_toolchain import load_binaryen_manifest
from molt.llvm_toolchain import load_llvm_releases
from molt.rust_toolchain import rust_channel

ROOT = Path(__file__).resolve().parents[1]
PROOF_PLAN = Path("tools/proof_plan.toml")
LEDGER = Path("docs/agent/V1_HANDOFF_FINDINGS.md")
USER_AGENT = "molt-pin-freshness"
TIMEOUT_SECONDS = 60

# Pins that trail upstream on purpose, each while an open ledger row moves it.
HOLDS = {
    "llvm": "HF-64",
    "python": "HF-65",
}

FetchJson = Callable[[str], object]
FetchBytes = Callable[[str], bytes]


class PinFreshnessError(RuntimeError):
    pass


def _request(url: str, accept: str) -> urllib.request.Request:
    headers = {"User-Agent": USER_AGENT, "Accept": accept}
    token = os.environ.get("GITHUB_TOKEN", "").strip()
    if token and url.startswith("https://api.github.com/"):
        headers["Authorization"] = f"Bearer {token}"
    return urllib.request.Request(url, headers=headers)


def fetch_bytes(url: str) -> bytes:
    request = _request(url, "*/*")
    with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
        return response.read()


def fetch_json(url: str) -> object:
    request = _request(url, "application/json")
    with urllib.request.urlopen(request, timeout=TIMEOUT_SECONDS) as response:
        return json.load(response)


def version_key(version: str) -> tuple[int, ...]:
    parts = re.findall(r"\d+", version)
    if not parts:
        raise PinFreshnessError(f"version {version!r} has no numeric part")
    return tuple(int(part) for part in parts)


# Upstream resolvers: each returns the newest stable version in the spelling
# the pin uses.


def github_latest(repo: str, tag_prefix: str, fetch: FetchJson) -> str:
    release = fetch(f"https://api.github.com/repos/{repo}/releases/latest")
    tag = release.get("tag_name") if isinstance(release, dict) else None
    if not isinstance(tag, str) or not tag.startswith(tag_prefix):
        raise PinFreshnessError(f"{repo}: unexpected latest tag {tag!r}")
    return tag.removeprefix(tag_prefix)


def node_latest(fetch: FetchJson) -> str:
    index = fetch("https://nodejs.org/dist/index.json")
    if not isinstance(index, list) or not index:
        raise PinFreshnessError("nodejs.org: empty release index")
    return str(index[0]["version"]).removeprefix("v")


def rust_stable(fetch_text: FetchBytes) -> str:
    url = "https://static.rust-lang.org/dist/channel-rust-stable.toml"
    manifest = tomllib.loads(fetch_text(url).decode("utf-8"))
    return str(manifest["pkg"]["rust"]["version"]).split(" ", 1)[0]


def npm_latest(package: str, fetch: FetchJson) -> str:
    quoted = package.replace("/", "%2F")
    return str(fetch(f"https://registry.npmjs.org/{quoted}/latest")["version"])


def crate_latest(name: str, fetch: FetchJson) -> str:
    record = fetch(f"https://crates.io/api/v1/crates/{name}")
    return str(record["crate"]["max_stable_version"])


def python_latest_minor(fetch: FetchJson) -> str:
    return str(fetch("https://endoflife.date/api/python.json")[0]["cycle"])


@dataclass(frozen=True)
class Pin:
    name: str
    authority: str
    current: str
    latest: Callable[[], str]


def _toolchain_policies(root: Path) -> dict[str, str]:
    plan = tomllib.loads((root / PROOF_PLAN).read_text(encoding="utf-8"))
    return {
        str(policy["name"]): str(policy["setup_value"])
        for policy in plan["toolchain_policy"]
        if "setup_value" in policy
    }


# Upstream repositories of `config/tool_releases.toml` tools published as
# GitHub releases, with their tag prefix.
GITHUB_TOOLS = {
    "wasm-tools": ("bytecodealliance/wasm-tools", "v"),
    "lune": ("lune-org/lune", "v"),
    "sccache": ("mozilla/sccache", "v"),
}


def collect_pins(
    root: Path,
    *,
    fetch: FetchJson = fetch_json,
    fetch_text: FetchBytes = fetch_bytes,
) -> list[Pin]:
    tools = tool_releases.load_tool_releases(root)
    llvm = load_llvm_releases(root)
    policies = _toolchain_policies(root)
    supply_chain = tomllib.loads(
        (root / "config/release_supply_chain.toml").read_text(encoding="utf-8")
    )
    lean = (root / "formal/lean/lean-toolchain").read_text(encoding="utf-8").strip()
    plan = str(PROOF_PLAN)
    manifest = tool_releases.TOOL_RELEASES_PATH

    def github(repo: str, prefix: str) -> Callable[[], str]:
        return lambda: github_latest(repo, prefix, fetch)

    pins = [
        Pin(
            "rust",
            "rust-toolchain.toml",
            rust_channel((root / "rust-toolchain.toml").read_bytes()),
            lambda: rust_stable(fetch_text),
        ),
        Pin("python", plan, policies["python"], lambda: python_latest_minor(fetch)),
        Pin("uv", plan, policies["uv"], github("astral-sh/uv", "")),
        Pin("node", manifest, tools["node"].version, lambda: node_latest(fetch)),
        Pin(
            "llvm",
            "config/llvm_toolchain_releases.toml",
            llvm.default_release,
            github("llvm/llvm-project", "llvmorg-"),
        ),
        Pin(
            "wasi-sdk",
            "config/llvm_toolchain_releases.toml",
            llvm.wasi_sdk.archive_version,
            github("WebAssembly/wasi-sdk", "wasi-sdk-"),
        ),
        Pin(
            "binaryen",
            "config/binaryen_releases.toml",
            load_binaryen_manifest(root).release.version,
            github("WebAssembly/binaryen", "version_"),
        ),
        Pin(
            "lean",
            "formal/lean/lean-toolchain",
            lean.rsplit(":v", 1)[-1],
            github("leanprover/lean4", "v"),
        ),
        Pin(
            "elan",
            "config/release_supply_chain.toml",
            str(supply_chain["downloads"]["elan"]["version"]),
            github("leanprover/elan", "v"),
        ),
        Pin(
            "quint",
            plan,
            policies["quint"],
            lambda: npm_latest("@informalsystems/quint", fetch),
        ),
    ]
    for crate in ("cargo-deny", "cargo-audit"):
        pins.append(
            Pin(crate, plan, policies[crate], lambda c=crate: crate_latest(c, fetch))
        )
    for name, (repo, prefix) in GITHUB_TOOLS.items():
        if name in tools:
            pins.append(Pin(name, manifest, tools[name].version, github(repo, prefix)))
    unowned = set(tools) - {pin.name for pin in pins}
    if unowned:
        raise PinFreshnessError(
            f"{manifest} tools without an upstream resolver: {sorted(unowned)!r}"
        )
    return pins


def open_ledger_rows(root: Path) -> set[str]:
    text = (root / LEDGER).read_text(encoding="utf-8")
    open_sections = text.split("\n## Fixed", 1)[0]
    return set(re.findall(r"^\| (HF-\d+) \|", open_sections, re.MULTILINE))


def check(
    pins: Sequence[Pin],
    open_rows: set[str],
    *,
    holds: Mapping[str, str] = HOLDS,
    out: TextIO = sys.stdout,
) -> int:
    failures = 0
    names = {pin.name for pin in pins}
    for name in sorted(set(holds) - names):
        failures += 1
        print(f"ERROR   hold names unknown pin {name!r}", file=out)
    for pin in pins:
        latest = pin.latest()
        stale = version_key(latest) > version_key(pin.current)
        row = holds.get(pin.name)
        if row is not None and row not in open_rows:
            failures += 1
            print(
                f"ERROR   {pin.name}: hold cites {row}, not an open ledger row",
                file=out,
            )
        elif row is not None and not stale:
            failures += 1
            print(
                f"ERROR   {pin.name}: current at {pin.current}; drop its hold", file=out
            )
        elif stale and row is not None:
            print(f"HELD    {pin.name}: {pin.current} -> {latest} ({row})", file=out)
        elif stale:
            failures += 1
            print(
                f"STALE   {pin.name}: {pin.current} -> {latest} ({pin.authority})",
                file=out,
            )
        else:
            print(f"current {pin.name}: {pin.current}", file=out)
    print(f"pin-freshness: {failures} failing of {len(pins)} pins", file=out)
    return 1 if failures else 0


# --update for config/tool_releases.toml.


def _archive_members(filename: str, data: bytes) -> set[str]:
    if filename.endswith(".zip"):
        with zipfile.ZipFile(io.BytesIO(data)) as archive:
            return set(archive.namelist())
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as archive:
        return {member.name.removeprefix("./") for member in archive.getmembers()}


@dataclass(frozen=True)
class UpstreamAsset:
    sha256: str
    size: int | None


def _upstream_assets(
    provenance: tool_releases.ToolProvenance,
    url: str,
    fetch: FetchJson,
    fetch_text: FetchBytes,
) -> tuple[dict[str, UpstreamAsset], int | None]:
    """Return {asset filename: upstream record} and the GitHub release id."""
    if provenance.kind == tool_releases.PROVENANCE_GITHUB_RELEASE:
        release = fetch(url)
        assets = {}
        for asset in release["assets"]:
            digest = str(asset.get("digest") or "")
            if digest.startswith("sha256:"):
                assets[str(asset["name"])] = UpstreamAsset(
                    digest.removeprefix("sha256:"), int(asset["size"])
                )
        return assets, int(release["id"])
    if provenance.kind == tool_releases.PROVENANCE_CHECKSUM_MANIFEST:
        assets = {}
        for line in fetch_text(url).decode("utf-8").splitlines():
            parts = line.split()
            if len(parts) == 2 and re.fullmatch(r"[0-9a-f]{64}", parts[0]):
                assets[parts[1].removeprefix("*")] = UpstreamAsset(parts[0], None)
        return assets, None
    raise PinFreshnessError(f"unsupported provenance kind {provenance.kind!r}")


def plan_tool_update(
    release: tool_releases.ToolRelease,
    new_version: str,
    *,
    fetch: FetchJson = fetch_json,
    fetch_text: FetchBytes = fetch_bytes,
) -> dict[str, object]:
    """Resolve and verify every asset of `release` at `new_version`."""
    old = release.version
    provenance_url = release.provenance.url.replace(old, new_version)
    upstream, release_id = _upstream_assets(
        release.provenance, provenance_url, fetch, fetch_text
    )
    provenance: dict[str, object] = {"url": provenance_url}
    if release_id is not None:
        provenance["release_id"] = release_id
    assets: dict[str, dict[str, object]] = {}
    for key, asset in release.assets.items():
        url = asset.url.replace(old, new_version)
        member = asset.archive_member.replace(old, new_version)
        filename = url.rsplit("/", 1)[-1]
        where = f"{release.name} {new_version}: {filename}"
        record = upstream.get(filename)
        if record is None:
            raise PinFreshnessError(f"{where}: upstream records no SHA-256")
        data = fetch_text(url)
        actual = hashlib.sha256(data).hexdigest()
        if actual != record.sha256:
            raise PinFreshnessError(
                f"{where}: sha256 {actual} != upstream {record.sha256}"
            )
        if record.size is not None and record.size != len(data):
            raise PinFreshnessError(
                f"{where}: {len(data)} bytes != upstream {record.size}"
            )
        if member not in _archive_members(filename, data):
            raise PinFreshnessError(f"{where}: archive has no member {member}")
        assets[key] = {
            "url": url,
            "size": len(data),
            "sha256": actual,
            "archive_member": member,
        }
    return {"version": new_version, "provenance": provenance, "assets": assets}


def _toml_value(value: object) -> str:
    return json.dumps(value) if isinstance(value, str) else str(value)


_TOOL_HEADER = re.compile(
    r"^\[tools\.(?P<tool>[^\].]+)"
    r"(?:\.(?P<table>provenance|assets\.[^\]]+))?\]\s*$"
)


def rewrite_tool(text: str, name: str, update: Mapping[str, object]) -> str:
    """Rewrite one tool's version, provenance and asset fields in place."""
    tables: dict[str | None, Mapping[str, object]] = {
        None: {"version": update["version"]},
        "provenance": update["provenance"],
        **{f"assets.{key}": fields for key, fields in dict(update["assets"]).items()},
    }
    owner: str | None = None
    table: str | None = None
    out = []
    for line in text.splitlines(keepends=True):
        header = _TOOL_HEADER.match(line)
        if header is not None:
            owner, table = header.group("tool"), header.group("table")
        elif line.startswith("["):
            owner = None
        elif owner == name and "=" in line:
            key = line.split("=", 1)[0].strip()
            fields = tables.get(table, {})
            if key in fields:
                line = f"{key} = {_toml_value(fields[key])}\n"
        out.append(line)
    return "".join(out)


def rewrite_policy(text: str, name: str, old: str, new: str) -> str:
    """Move one `[[toolchain_policy]]` block's version literals."""
    blocks = re.split(r"(?m)^(?=\[\[)", text)
    marker = f'[[toolchain_policy]]\nname = "{name}"\n'
    matches = [index for index, block in enumerate(blocks) if block.startswith(marker)]
    if len(matches) != 1:
        raise PinFreshnessError(f"{PROOF_PLAN} has {len(matches)} {name!r} policies")
    block = blocks[matches[0]]
    blocks[matches[0]] = block.replace(f'"{old}"', f'"{new}"').replace(
        f'\\"{old}\\"', f'\\"{new}\\"'
    )
    return "".join(blocks)


def update_tool(
    root: Path,
    name: str,
    *,
    fetch: FetchJson = fetch_json,
    fetch_text: FetchBytes = fetch_bytes,
) -> str:
    pins = collect_pins(root, fetch=fetch, fetch_text=fetch_text)
    pin = next((pin for pin in pins if pin.name == name), None)
    manifest = root / tool_releases.TOOL_RELEASES_PATH
    if pin is None or pin.authority != tool_releases.TOOL_RELEASES_PATH:
        raise PinFreshnessError(f"{name} is not a {manifest.name} tool")
    latest = pin.latest()
    if version_key(latest) <= version_key(pin.current):
        return f"{name}: already at {pin.current}"
    release = tool_releases.load_tool_releases(root)[name]
    update = plan_tool_update(release, latest, fetch=fetch, fetch_text=fetch_text)
    plan_path = root / PROOF_PLAN
    originals = {
        manifest: manifest.read_text(encoding="utf-8"),
        plan_path: plan_path.read_text(encoding="utf-8"),
    }
    manifest.write_text(
        rewrite_tool(originals[manifest], name, update), encoding="utf-8"
    )
    plan_path.write_text(
        rewrite_policy(originals[plan_path], name, pin.current, latest),
        encoding="utf-8",
    )
    try:
        moved = tool_releases.load_tool_releases(root)[name]
        if moved.version != latest:
            raise PinFreshnessError(f"{manifest.name} still pins {moved.version}")
    except (PinFreshnessError, tool_releases.ToolReleaseError):
        for path, original in originals.items():
            path.write_text(original, encoding="utf-8")
        raise
    return (
        f"{name}: {pin.current} -> {latest} ({len(update['assets'])} assets "
        "verified); regenerate with `tools/gen_proof_plan.py`"
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--check", action="store_true")
    mode.add_argument("--update", metavar="NAME")
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    try:
        if args.check:
            return check(collect_pins(args.root), open_ledger_rows(args.root))
        print(update_tool(args.root, args.update))
        return 0
    except (
        PinFreshnessError,
        tool_releases.ToolReleaseError,
        KeyError,
        OSError,
        ValueError,
    ) as exc:
        print(f"pin-freshness: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
