"""The environment-registry gate on a synthetic tree.

The oracle is the tree itself: each file plants one known access form, and
the test states which registry entry must satisfy it and which violation
must fire when the entry is missing. The real repository is not scanned
here; the proof plan runs the gate on it.
"""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT / "tools") not in sys.path:
    sys.path.insert(0, str(ROOT / "tools"))
if str(ROOT / "src") not in sys.path:
    sys.path.insert(0, str(ROOT / "src"))

import check_environment_registry as C  # noqa: E402
from molt.environment_registry import registry_from_payload  # noqa: E402


def _write(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _variable(
    name: str, owner: str, *, audience: str = "user", kind: str = "string"
) -> dict:
    return {
        "name": name,
        "audience": audience,
        "kind": kind,
        "default": "",
        "values": [],
        "owner": owner,
        "summary": f"{name} for the synthetic tree.",
    }


def _payload(**overrides) -> dict:
    payload = {
        "variables": [],
        "stems": [],
        "families": [],
        "prefix_families": [],
        "retired": [],
        "retired_suffixes": [],
    }
    payload.update(overrides)
    return payload


@pytest.fixture
def tree(tmp_path: Path) -> Path:
    _write(
        tmp_path,
        "src/molt/reader.py",
        """
import os

ALPHA_ENV = "MOLT_ALPHA"


def _env_bool(env, names, *, default):
    for name in names:
        if env.get(name) is not None:
            return True
    return default


def read(env):
    direct = os.environ.get(ALPHA_ENV)
    cap = os.environ.get("MOLT_RESOURCE_MAX_MEMORY")
    helped = _env_bool(env, ("MOLT_BETA",), default=False)
    child_env = dict(env)
    child_env["MOLT_GAMMA"] = "1"
    run(env={"MOLT_DELTA": "1"})
    label = "MOLT_SYMBOLIC_NAME"  # a symbol, not an environment access
    composed = f"MOLT_RESOURCE_{direct}"
    return direct, cap, helped, child_env, label, composed


def run(env):
    return env
""",
    )
    _write(
        tmp_path,
        "runtime/x/src/lib.rs",
        """
// MOLT_RUST=1 enables the direct read below.
fn env_setting(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

pub fn read() -> Option<String> {
    let direct = std::env::var("MOLT_RUST").ok();
    let helped = env_setting("MOLT_HELPED");
    let chained = ["MOLT_CHAIN_A", "MOLT_CHAIN_B"]
        .iter()
        .find_map(|name| env_setting(name));
    let label = "MOLT_RUST_SYMBOL";
    direct.or(helped).or(chained).or(Some(label.to_string()))
}
""",
    )
    _write(
        tmp_path,
        ".github/workflows/ci.yml",
        """
env:
  MOLT_CI_X: "1"
jobs:
  run:
    steps:
      - run: echo "${MOLT_CI_X}" && python3 tools/guarded_exec.py --prefix MOLT_STEMX -- true
""",
    )
    _write(tmp_path, "docs/guide.md", "Set `MOLT_ALPHA=1` before you run the tool.\n")
    _write(
        tmp_path,
        "tests/test_cases.py",
        """
def test_it(monkeypatch):
    monkeypatch.setenv("MOLT_TESTONLY", "1")
""",
    )
    _write(
        tmp_path, "tools/proof_plan.toml", 'authority_inputs = ["tests/harness.py"]\n'
    )
    _write(
        tmp_path,
        "tests/harness.py",
        "import os\nHARNESS = os.environ.get('MOLT_HARNESS_ONLY')\n",
    )
    _write(
        tmp_path,
        "tools/guard.py",
        'def run(prefix):\n    return f"{prefix}_TIMEOUT_SEC"\n',
    )
    return tmp_path


def _full_registry() -> object:
    return registry_from_payload(
        _payload(
            variables=[
                _variable("MOLT_ALPHA", "src/molt/reader.py"),
                _variable("MOLT_BETA", "src/molt/reader.py"),
                _variable("MOLT_CHAIN_A", "runtime/x/src/lib.rs"),
                _variable("MOLT_CHAIN_B", "runtime/x/src/lib.rs"),
                _variable("MOLT_CI_X", ".github/workflows/ci.yml", audience="ci"),
                _variable("MOLT_DELTA", "src/molt/reader.py", audience="internal"),
                _variable("MOLT_GAMMA", "src/molt/reader.py", audience="internal"),
                _variable(
                    "MOLT_HARNESS_ONLY", "tests/harness.py", audience="developer"
                ),
                _variable("MOLT_HELPED", "runtime/x/src/lib.rs"),
                _variable("MOLT_RESOURCE_MAX_MEMORY", "src/molt/reader.py"),
                _variable("MOLT_RUST", "runtime/x/src/lib.rs"),
            ],
            stems=[
                {
                    "name": "MOLT_STEMX",
                    "owner": ".github/workflows/ci.yml",
                    "summary": "Stem.",
                }
            ],
            families=[
                {
                    "suffix": "_TIMEOUT_SEC",
                    "root_fallback": "",
                    "audience": "developer",
                    "kind": "float",
                    "default": "",
                    "values": [],
                    "owner": "tools/guard.py",
                    "summary": "Timeout.",
                }
            ],
            retired=[
                {
                    "name": "MOLT_OLD",
                    "replacement": "MOLT_ALPHA",
                    "retired": "2026-10-06",
                    "note": "",
                    "rejected_by": [],
                }
            ],
        )
    )


def _scan(root: Path) -> C.Scan:
    return C.scan_repository(workers=1, root=root)


def _sites(scan: C.Scan, name: str) -> list[C.Site]:
    return [site for site in scan.sites if site.name == name]


def test_python_forms_are_classified(tree: Path) -> None:
    scan = _scan(tree)
    assert {s.access for s in _sites(scan, "MOLT_ALPHA")} >= {
        C.ACCESS_READ,
        C.ACCESS_REFERENCE,
    }
    alpha_reads = [s for s in _sites(scan, "MOLT_ALPHA") if s.access == C.ACCESS_READ]
    assert alpha_reads and alpha_reads[0].form.startswith("call:get:ALPHA_ENV")
    beta = _sites(scan, "MOLT_BETA")
    assert (
        beta and beta[0].access == C.ACCESS_READ and beta[0].form == "helper:_env_bool"
    )
    gamma = _sites(scan, "MOLT_GAMMA")
    assert (
        gamma and gamma[0].access == C.ACCESS_SET and gamma[0].form == "subscript-store"
    )
    delta = _sites(scan, "MOLT_DELTA")
    assert delta and delta[0].access == C.ACCESS_SET and delta[0].form == "dict-key"
    symbol = _sites(scan, "MOLT_SYMBOLIC_NAME")
    assert symbol and symbol[0].access == C.ACCESS_REFERENCE
    prefix = [
        s
        for s in scan.sites
        if s.access == C.ACCESS_PREFIX and s.name == "MOLT_RESOURCE_"
    ]
    assert prefix and prefix[0].form == "f-string"
    suffix = [
        s
        for s in scan.sites
        if s.access == C.ACCESS_SUFFIX and s.name == "_TIMEOUT_SEC"
    ]
    assert suffix and suffix[0].path == "tools/guard.py"


def test_rust_forms_are_classified(tree: Path) -> None:
    scan = _scan(tree)
    rust = [s for s in _sites(scan, "MOLT_RUST") if s.access == C.ACCESS_READ]
    assert rust and rust[0].form == "call:var"
    helped = _sites(scan, "MOLT_HELPED")
    assert (
        helped
        and helped[0].access == C.ACCESS_READ
        and helped[0].form == "helper:env_setting"
    )
    chained = _sites(scan, "MOLT_CHAIN_A")
    assert (
        chained and chained[0].access == C.ACCESS_READ and chained[0].form == "closure"
    )
    symbol = _sites(scan, "MOLT_RUST_SYMBOL")
    assert symbol and symbol[0].access == C.ACCESS_REFERENCE
    comment = [s for s in _sites(scan, "MOLT_RUST") if s.access == C.ACCESS_MENTION]
    assert (
        comment
        and comment[0].access == C.ACCESS_MENTION
        and comment[0].form == C.FORM_ENV_MENTION
    )


def test_text_forms_are_classified(tree: Path) -> None:
    scan = _scan(tree)
    accesses = {(s.access, s.form) for s in _sites(scan, "MOLT_CI_X")}
    assert (C.ACCESS_SET, "env-map") in accesses
    assert (C.ACCESS_READ, "expansion") in accesses
    stem = _sites(scan, "MOLT_STEMX")
    assert stem and stem[0].access == C.ACCESS_STEM
    doc = [s for s in _sites(scan, "MOLT_ALPHA") if s.language == "doc"]
    assert (
        doc
        and doc[0].language == "doc"
        and doc[0].form == C.FORM_ENV_MENTION
        and doc[0].enforced
    )


def test_tests_are_evidence_only_but_harness_modules_are_enforced(tree: Path) -> None:
    scan = _scan(tree)
    testonly = _sites(scan, "MOLT_TESTONLY")
    assert testonly and not testonly[0].enforced
    harness = _sites(scan, "MOLT_HARNESS_ONLY")
    assert harness and harness[0].enforced and harness[0].access == C.ACCESS_READ


def test_complete_registry_is_clean(tree: Path) -> None:
    violations = C.check(_full_registry(), _scan(tree), root=tree)
    assert violations == [], violations


def test_unregistered_definite_access_fails_but_bare_reference_does_not(
    tree: Path,
) -> None:
    registry = registry_from_payload(_payload())
    violations = C.check(registry, _scan(tree), root=tree)
    unregistered = {v.name for v in violations if v.rule == "unregistered"}
    assert {
        "MOLT_ALPHA",
        "MOLT_BETA",
        "MOLT_GAMMA",
        "MOLT_DELTA",
        "MOLT_RUST",
        "MOLT_HELPED",
    } <= unregistered
    assert {
        "MOLT_CHAIN_A",
        "MOLT_CHAIN_B",
        "MOLT_CI_X",
        "MOLT_HARNESS_ONLY",
    } <= unregistered
    assert "MOLT_STEMX" in unregistered
    assert "MOLT_SYMBOLIC_NAME" not in unregistered
    assert "MOLT_RUST_SYMBOL" not in unregistered
    assert "MOLT_TESTONLY" not in unregistered
    mentions = {v.name for v in violations if v.rule == "unregistered-mention"}
    assert mentions == {"MOLT_ALPHA", "MOLT_RUST"}
    prefixes = {v.name for v in violations if v.rule == "unregistered-prefix"}
    assert prefixes == {"MOLT_RESOURCE_"}


def test_retired_name_fails_everywhere_including_tests(tree: Path) -> None:
    violations = C.check(_full_registry(), _scan(tree), root=tree)
    assert violations == []
    _write(tree, "tests/test_more.py", 'import os\nos.environ.get("MOLT_OLD")\n')
    violations = C.check(_full_registry(), _scan(tree), root=tree)
    assert [v.rule for v in violations] == ["retired-name"]
    assert "MOLT_ALPHA" in violations[0].message


def test_rejected_by_allows_only_the_declared_rejector(tree: Path) -> None:
    _write(
        tree,
        "src/molt/reject.py",
        'import os\nif "MOLT_OLD" in os.environ:\n    raise SystemExit("retired")\n',
    )
    registry = _full_registry()
    violations = C.check(registry, _scan(tree), root=tree)
    assert [v.rule for v in violations] == ["retired-name"]
    payload = _payload(
        variables=[],
        retired=[
            {
                "name": "MOLT_OLD",
                "replacement": "MOLT_ALPHA",
                "retired": "2026-10-06",
                "note": "",
                "rejected_by": ["src/molt/reject.py"],
            }
        ],
    )
    allowing = registry_from_payload(payload)
    rules = {v.rule for v in C.check(allowing, _scan(tree), root=tree)}
    assert "retired-name" not in rules
    stale = registry_from_payload(
        _payload(
            retired=[
                {
                    "name": "MOLT_OLD",
                    "replacement": "MOLT_ALPHA",
                    "retired": "2026-10-06",
                    "note": "",
                    "rejected_by": ["src/molt/reader.py"],
                }
            ]
        )
    )
    mismatches = [
        v for v in C.check(stale, _scan(tree), root=tree) if v.rule == "owner-mismatch"
    ]
    assert mismatches and "rejected_by" in mismatches[0].message


def test_registered_name_without_a_reader_fails_unless_internal_or_ci_with_a_setter(
    tree: Path,
) -> None:
    payload = _payload(
        variables=[
            _variable("MOLT_NOBODY", "src/molt/reader.py"),
            _variable("MOLT_GAMMA", "src/molt/reader.py", audience="user"),
            _variable("MOLT_DELTA", "src/molt/reader.py", audience="internal"),
        ]
    )
    violations = C.check(registry_from_payload(payload), _scan(tree), root=tree)
    no_reader = {v.name: v.message for v in violations if v.rule == "no-reader"}
    assert "MOLT_NOBODY" in no_reader and "no site at all" in no_reader["MOLT_NOBODY"]
    assert "MOLT_GAMMA" in no_reader and "only set at" in no_reader["MOLT_GAMMA"]
    assert "MOLT_DELTA" not in no_reader


def test_owner_must_exist_and_hold_a_site(tree: Path) -> None:
    payload = _payload(
        variables=[
            _variable("MOLT_ALPHA", "src/molt/missing.py"),
            _variable("MOLT_RUST", "src/molt/reader.py"),
        ]
    )
    violations = C.check(registry_from_payload(payload), _scan(tree), root=tree)
    rules = {(v.rule, v.name) for v in violations}
    assert ("owner-missing", "MOLT_ALPHA") in rules
    assert ("owner-mismatch", "MOLT_RUST") in rules


def test_family_and_stem_need_composed_evidence(tree: Path) -> None:
    payload = _payload(
        stems=[
            {"name": "MOLT_UNUSED", "owner": "tools/guard.py", "summary": "Unused."}
        ],
        families=[
            {
                "suffix": "_NEVER_SEC",
                "root_fallback": "",
                "audience": "developer",
                "kind": "float",
                "default": "",
                "values": [],
                "owner": "tools/guard.py",
                "summary": "Never composed.",
            }
        ],
    )
    violations = C.check(registry_from_payload(payload), _scan(tree), root=tree)
    assert {v.name for v in violations if v.rule == "no-reader"} == {
        "MOLT_UNUSED",
        "MOLT_<STEM>_NEVER_SEC",
    }


def test_env_shaped_mention_only_matches_environment_phrasing() -> None:
    sites = C.mention_sites(
        "x.py",
        1,
        "the generated name MOLT_SYMBOL = 1 is Python",
        "python",
        enforced=True,
    )
    assert [s.form for s in sites] == ["string"]
    sites = C.mention_sites(
        "x.py", 1, "set MOLT_FLAG=1 to enable it", "python", enforced=True
    )
    assert [s.form for s in sites] == [C.FORM_ENV_MENTION]
    sites = C.mention_sites(
        "x.py", 1, "the MOLT_FLAG environment variable", "python", enforced=True
    )
    assert [s.form for s in sites] == [C.FORM_ENV_MENTION]


def test_rust_lexer_skips_comments_strings_and_lifetimes() -> None:
    tokens, comments = C._rust_lex(
        "fn f<'a>(x: &'a str) -> &'a str { /* \"MOLT_IN_BLOCK\" */ x } // MOLT_LINE\nlet s = \"MOLT_STR\"; let c = '\"';"
    )
    strings = [t.text for t in tokens if t.kind == "string"]
    assert strings == ["MOLT_STR"]
    assert [c[1] for c in comments] == ['/* "MOLT_IN_BLOCK" */', "// MOLT_LINE"]
    assert any(t.kind == "lifetime" and t.text == "'a" for t in tokens)
