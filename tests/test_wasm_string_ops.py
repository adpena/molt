import os
import shutil
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest

from tests.wasm_harness import write_wasm_runner

STRING_HELPERS = textwrap.dedent(
    """\
    const boxStr = (value) => boxPtr({ type: 'str', value });
    const listFromStrings = (items) => {
      return boxPtr({ type: 'list', items: items.map((item) => boxStr(item)) });
    };
    const stringCount = (hay, needle) => {
      if (needle.length === 0) return hay.length + 1;
      let count = 0;
      let idx = 0;
      while (idx <= hay.length - needle.length) {
        const found = hay.indexOf(needle, idx);
        if (found < 0) break;
        count += 1;
        idx = found + needle.length;
      }
      return count;
    };
    """
)

STRING_IMPORT_OVERRIDES = textwrap.dedent(
    """\
    string_find: (hayBits, needleBits) => {
      const hay = getStr(hayBits);
      const needle = getStr(needleBits);
      return boxInt(hay.indexOf(needle));
    },
    string_startswith: (hayBits, needleBits) => {
      const hay = getStr(hayBits);
      const needle = getStr(needleBits);
      return boxBool(hay.startsWith(needle));
    },
    string_endswith: (hayBits, needleBits) => {
      const hay = getStr(hayBits);
      const needle = getStr(needleBits);
      return boxBool(hay.endsWith(needle));
    },
    string_count: (hayBits, needleBits) => {
      const hay = getStr(hayBits);
      const needle = getStr(needleBits);
      return boxInt(stringCount(hay, needle));
    },
    string_join: (sepBits, listBits) => {
      const sep = getStr(sepBits);
      const list = getList(listBits);
      if (!list) return boxNone();
      const parts = list.items.map((item) => getStr(item));
      return boxStr(parts.join(sep));
    },
    string_split: (hayBits, needleBits) => {
      const hay = getStr(hayBits);
      const needle = getStr(needleBits);
      if (needle === '') return boxNone();
      return listFromStrings(hay.split(needle));
    },
    string_replace: (hayBits, needleBits, replBits, countBits) => {
      const hay = getStr(hayBits);
      const needle = getStr(needleBits);
      const repl = getStr(replBits);
      const count = Number(unboxIntLike(countBits));
      if (count === 0) return boxStr(hay);
      if (needle === '') {
        const limit = count < 0 ? hay.length + 1 : Math.min(count, hay.length + 1);
        if (limit === 0) return boxStr(hay);
        let out = '';
        let inserted = 0;
        if (inserted < limit) {
          out += repl;
          inserted += 1;
        }
        for (const ch of hay) {
          out += ch;
          if (inserted < limit) {
            out += repl;
            inserted += 1;
          }
        }
        return boxStr(out);
      }
      if (count < 0) return boxStr(hay.split(needle).join(repl));
      let out = '';
      let idx = 0;
      let replaced = 0;
      while (replaced < count) {
        const found = hay.indexOf(needle, idx);
        if (found < 0) break;
        out += hay.slice(idx, found);
        out += repl;
        idx = found + needle.length;
        replaced += 1;
      }
      out += hay.slice(idx);
      return boxStr(out);
    },
    """
)


def test_wasm_string_ops_parity(tmp_path: Path) -> None:
    if shutil.which("node") is None:
        pytest.skip("node is required for wasm parity test")
    if shutil.which("cargo") is None:
        pytest.skip("cargo is required for wasm parity test")

    root = Path(__file__).resolve().parents[1]
    src = tmp_path / "string_ops.py"
    src.write_text(
        textwrap.dedent(
            """\
            s = 'alpha,beta,gamma'
            print(s.find('beta'))
            print(s.find('beta', 2))
            print(s.startswith('alpha'))
            print(s.startswith('alpha', 0, 5))
            print(s.endswith('gamma'))
            print(s.endswith('gamma', 0, len(s)))
            parts = s.split(',')
            print(len(parts))
            print(parts[1])
            print(','.join(parts))
            print('ha'.replace('a', 'o'))
            print('mississippi'.count('iss'))
            print('mississippi'.count('iss', 1, 6))
            """
        )
    )

    output_wasm = tmp_path / "output.wasm"

    runner = write_wasm_runner(
        tmp_path,
        "run_wasm_string_ops.js",
        extra_js=STRING_HELPERS,
        import_overrides=STRING_IMPORT_OVERRIDES,
    )

    env = os.environ.copy()
    env["PYTHONPATH"] = str(root / "src")
    build = subprocess.run(
        [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            str(src),
            "--target",
            "wasm",
            "--out-dir",
            str(tmp_path),
        ],
        cwd=root,
        env=env,
        capture_output=True,
        text=True,
    )
    assert build.returncode == 0, build.stderr

    run = subprocess.run(
        ["node", str(runner), str(output_wasm)],
        cwd=root,
        capture_output=True,
        text=True,
    )
    assert run.returncode == 0, run.stderr
    assert (
        run.stdout.strip()
        == "6\n6\nTrue\nTrue\nTrue\nTrue\n3\nbeta\nalpha,beta,gamma\nho\n2\n1"
    )
