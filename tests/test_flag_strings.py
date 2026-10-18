"""Flag strings decode as Cargo and cc-rs read them, and encode losslessly."""

from __future__ import annotations

import os
from pathlib import Path
import random
import shutil
import sys
import tomllib

import pytest

from molt.flag_strings import FlagStringError, FlagStringRule
from molt.process_environment import NameRule, ProcessEnvironment
from tests.process_guard_common import run_guarded_test_process

_REPO_ROOT = Path(__file__).resolve().parents[1]

# Expected values come from the consumers' source, not from this module:
# Cargo target_info.rs ``rustflags_from_env`` and context/de.rs ``StringList``,
# Rust ``char::is_whitespace`` and ``u8::is_ascii_whitespace``, and the
# ``shlex`` 2.0 crate's ``bytes::Shlex`` that cc-rs ``envflags`` calls.
GOLDEN = [
    (FlagStringRule.CARGO_ENCODED, "", ()),
    (FlagStringRule.CARGO_ENCODED, "-C\x1fopt level=3", ("-C", "opt level=3")),
    (
        FlagStringRule.CARGO_RUSTFLAGS,
        "-C\topt-level=3  --cfg x",
        ("-C\topt-level=3", "--cfg", "x"),
    ),
    (FlagStringRule.CARGO_RUSTFLAGS, " \t-Ca\u3000 ", ("-Ca",)),
    (FlagStringRule.CARGO_RUSTFLAGS, "'native=C:\\sdk'", ("'native=C:\\sdk'",)),
    (FlagStringRule.CARGO_LIST, "a\tb\u00a0c\x1cd", ("a", "b", "c\x1cd")),
    (
        FlagStringRule.CARGO_LIST,
        "-L native=C:\\sdk\\lib",
        ("-L", "native=C:\\sdk\\lib"),
    ),
    (FlagStringRule.CC_WHITESPACE, "a\x0bb\u00a0c d\r\ne", ("a\x0bb\u00a0c", "d", "e")),
    (FlagStringRule.CC_WHITESPACE, '-I"C:\\a b"', ('-I"C:\\a', 'b"')),
    (
        FlagStringRule.CC_SHELL,
        "a \"b c\" 'd e' f\\ g # comment\nh",
        ("a", "b c", "d e", "f g", "h"),
    ),
    (
        FlagStringRule.CC_SHELL,
        '"x\\$y" "x\\qy" a\\\nb a#b',
        ("x$y", "x\\qy", "ab", "a#b"),
    ),
    (FlagStringRule.CC_SHELL, "a\rb '' #only", ("a\rb", "")),
    (FlagStringRule.CC_SHELL, "-IC:\\sdk", ("-IC:sdk",)),
]


@pytest.mark.parametrize(("rule", "value", "expected"), GOLDEN)
def test_decode_matches_the_consumer(rule, value, expected):
    assert rule.decode(value) == expected


@pytest.mark.parametrize("value", ['"open', "'open", "trailing\\", '"x\\'])
def test_malformed_shell_flags_are_refused_not_truncated(value):
    # cc-rs drops the malformed last word without an error.
    with pytest.raises(FlagStringError):
        FlagStringRule.CC_SHELL.decode(value)


@pytest.mark.parametrize(
    ("value", "rule"),
    [
        (None, FlagStringRule.CC_WHITESPACE),
        ("", FlagStringRule.CC_WHITESPACE),
        ("0", FlagStringRule.CC_WHITESPACE),
        ("false", FlagStringRule.CC_WHITESPACE),
        ("no", FlagStringRule.CC_WHITESPACE),
        ("False", FlagStringRule.CC_SHELL),
        ("1", FlagStringRule.CC_SHELL),
        ("yes", FlagStringRule.CC_SHELL),
    ],
)
def test_cc_rule_follows_env_value_is_true(value, rule):
    env = {} if value is None else {"CC_SHELL_ESCAPED_FLAGS": value}
    assert FlagStringRule.cc(env) is rule


def test_cc_rule_reads_the_switch_with_the_windows_name_rule():
    env = ProcessEnvironment({"cc_shell_escaped_flags": "1"}, rule=NameRule.WINDOWS)
    assert FlagStringRule.cc(env) is FlagStringRule.CC_SHELL
    exact = ProcessEnvironment({"cc_shell_escaped_flags": "1"}, rule=NameRule.EXACT)
    assert FlagStringRule.cc(exact) is FlagStringRule.CC_WHITESPACE


@pytest.mark.parametrize(
    ("rule", "token"),
    [
        (FlagStringRule.CARGO_ENCODED, "a\x1fb"),
        (FlagStringRule.CARGO_RUSTFLAGS, "native=/a b"),
        (FlagStringRule.CARGO_RUSTFLAGS, "\ta"),
        (FlagStringRule.CARGO_LIST, "native=/a b"),
        (FlagStringRule.CARGO_LIST, "a\u00a0b"),
        (FlagStringRule.CC_WHITESPACE, "a\tb"),
    ],
)
def test_encode_refuses_a_token_the_consumer_would_split(rule, token):
    with pytest.raises(FlagStringError, match="CARGO_TARGET_X_RUSTFLAGS cannot carry"):
        rule.encode(("-L", token), variable="CARGO_TARGET_X_RUSTFLAGS")


@pytest.mark.parametrize("rule", list(FlagStringRule))
@pytest.mark.parametrize("token", ["", "a\0b"])
def test_encode_refuses_empty_and_nul_tokens(rule, token):
    with pytest.raises(FlagStringError):
        rule.encode((token,), variable="FLAGS")


_ALPHABET = (
    " ", "\t", "\n", "\r", "\x0b", "\x0c", "\x1c", "\x1f", "\x85", "\xa0", "\u2003",
    "\u3000", "a", "b", "-", "=", ":", "/", "\\", '"', "'", "#", "$", "`", "\u00e9",
)  # fmt: skip


def _strings(seed: int, count: int, longest: int) -> list[str]:
    choose = random.Random(seed)
    return [
        "".join(choose.choice(_ALPHABET) for _ in range(choose.randrange(longest + 1)))
        for _ in range(count)
    ]


@pytest.mark.parametrize("rule", list(FlagStringRule))
def test_every_carried_vector_round_trips(rule):
    tokens = [token for token in _strings(7, 4000, 6) if token]
    choose = random.Random(11)
    checked = 0
    for _ in range(2400):
        vector = tuple(choose.sample(tokens, choose.randrange(1, 5)))
        if all(rule.carries(token) for token in vector):
            assert rule.decode(rule.encode(vector, variable="FLAGS")) == vector
            checked += 1
        else:
            with pytest.raises(FlagStringError):
                rule.encode(vector, variable="FLAGS")
    assert checked > 100


def test_windows_paths_round_trip_through_every_cargo_rule():
    vector = ("-L", "native=C:\\Molt\\ext\\wasi-sysroot\\lib\\wasm32-wasip1")
    for rule in FlagStringRule:
        assert rule.decode(rule.encode(vector, variable="FLAGS")) == vector


_ORACLE = r"""
use std::io::{Read, Write};

fn field(tokens: Vec<String>) -> String {
    tokens
        .iter()
        .map(|token| {
            let hex: String = token.bytes().map(|b| format!("{:02x}", b)).collect();
            format!("x{}", hex)
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn main() {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).unwrap();
    let mut out = String::new();
    for record in input.split('\0') {
        let rustflags = record
            .split(' ')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        let list = record.split_whitespace().map(str::to_string).collect();
        let ascii = record.split_ascii_whitespace().map(str::to_string).collect();
        let mut lexer = shlex::Shlex::new(record);
        let shell: Vec<String> = lexer.by_ref().collect();
        let status = if lexer.had_error { "E" } else { "O" };
        out.push_str(&format!(
            "{}|{}|{}|{}{}\n",
            field(rustflags),
            field(list),
            field(ascii),
            status,
            field(shell)
        ));
    }
    std::io::stdout().write_all(out.as_bytes()).unwrap();
}
"""


def _locked_shlex_source() -> Path:
    lock = tomllib.loads((_REPO_ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    packages = lock["package"]
    (cc,) = [package for package in packages if package["name"] == "cc"]
    (selector,) = [dep for dep in cc["dependencies"] if dep.split()[0] == "shlex"]
    versions = [p["version"] for p in packages if p["name"] == "shlex"]
    version = selector.split()[1] if " " in selector else versions[0]
    home = Path(os.environ.get("CARGO_HOME") or Path.home() / ".cargo")
    sources = sorted((home / "registry" / "src").glob(f"*/shlex-{version}/src/lib.rs"))
    if not sources:
        pytest.skip(f"shlex {version} source is not in the Cargo registry cache")
    return sources[0]


def _decode_field(field: str) -> tuple[str, ...]:
    if not field:
        return ()
    return tuple(bytes.fromhex(item[1:]).decode("utf-8") for item in field.split(","))


def test_decode_agrees_with_rust_and_the_locked_shlex_crate(tmp_path):
    rustc = shutil.which("rustc")
    if rustc is None:
        pytest.skip("rustc unavailable: the differential oracle cannot run")
    shlex_source = _locked_shlex_source()
    run_guarded_test_process(
        [rustc, "--edition=2018", "--crate-type=rlib", "--crate-name=shlex",
         '--cfg=feature="std"', str(shlex_source), "--out-dir", str(tmp_path)],
        check=True, timeout=120,
    )  # fmt: skip
    source = tmp_path / "oracle.rs"
    source.write_text(_ORACLE, encoding="utf-8")
    binary = tmp_path / ("oracle.exe" if sys.platform == "win32" else "oracle")
    run_guarded_test_process(
        [rustc, "--edition=2021", str(source), "-o", str(binary),
         "--extern", f"shlex={tmp_path / 'libshlex.rlib'}"],
        check=True, timeout=120,
    )  # fmt: skip
    records = [value for _rule, value, _expected in GOLDEN if "\0" not in value]
    records += _strings(3, 3000, 14)
    result = run_guarded_test_process(
        [str(binary)],
        check=True,
        timeout=60,
        text=False,
        input="\0".join(records).encode("utf-8"),
    )
    lines = result.stdout.decode("utf-8").split("\n")
    assert lines[-1] == "" and len(lines) == len(records) + 1
    for record, line in zip(records, lines, strict=False):
        rustflags, listed, ascii, shell = line.split("|")
        assert FlagStringRule.CARGO_RUSTFLAGS.decode(record) == _decode_field(rustflags)
        assert FlagStringRule.CARGO_LIST.decode(record) == _decode_field(listed)
        assert FlagStringRule.CC_WHITESPACE.decode(record) == _decode_field(ascii)
        if shell.startswith("E"):
            with pytest.raises(FlagStringError):
                FlagStringRule.CC_SHELL.decode(record)
        else:
            assert FlagStringRule.CC_SHELL.decode(record) == _decode_field(shell[1:])
