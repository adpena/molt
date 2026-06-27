#!/usr/bin/env bash
# Deterministic self-protection gate against the binary-corruption / resolver
# bug-class (and the "agent verified in session-local state" meta-bug).
#
# Builds a corpus of programs (trivial AND non-trivial) in a CLEAN, environment-
# independent way — drops stale runtime staticlibs + symbol caches, builds
# DAEMON-OFF (in-process, current backend) across BOTH stdlib profiles — then
# asserts every emitted Mach-O is structurally VALID (64-bit magic 0xfeedfacf,
# not the 0xfeedface corruption) AND RUNS to the expected output. A single
# corrupt/SIGKILL/wrong-output binary fails the gate.
#
# Why this exists: the per-app intrinsic resolver corruption flipped the Mach-O
# magic byte (cf->ce) for large/non-trivial manifests, producing a kernel-
# rejected binary that passed a naive "builds + symbols present" check. An agent
# verified it green in its own session; a clean-environment rebuild reproduced
# the corruption. This gate makes that class non-shippable.
#
# Usage: tools/verify_native_binary_valid.sh   (run from repo root)
# Exit 0 = all valid; non-zero = at least one corrupt/failed binary.
set -uo pipefail
cd "$(git rev-parse --show-toplevel 2>/dev/null || echo .)"

export MOLT_SESSION_ID="${MOLT_SESSION_ID:-verify-binary-valid}"
eval "$(
  python3 "$PWD/tools/run_context_env.py" \
    --root "$PWD" \
    --session-prefix verify-binary-valid \
    --prefer-external-artifacts \
    --dx \
    --format posix
)"
PY="${MOLT_PY:-.venv/bin/python3}"
CORPUS_DIR="$MOLT_DIFF_TMPDIR/verify_corpus"
mkdir -p "$CORPUS_DIR"

echo "== self-protection: clean-environment native-binary validity gate =="
echo "-- backend daemon disabled for this gate; no process-wide daemon kill --"

# Corpus: trivial -> non-trivial (manifest grows). file|profiles|expected_stdout
#
# Corpus filenames MUST NOT shadow a stdlib module they import (a file literally
# named `json.py` makes `import json` resolve to the script itself — a CPython-
# accurate self-shadow, not a molt bug, but it pollutes the corpus). `usejson`
# imports `json`; `usehashlib` imports `hashlib`.
#
# The `profiles` field scopes each program to the stdlib profile(s) that SUPPORT
# its imports. `hashlib` needs the `stdlib_crypto` intrinsics (`molt_pbkdf2_hmac`,
# `molt_scrypt`, ...) which exist only in the `full` profile staticlib (`micro`
# is "core only" by design and excludes crypto), so `usehashlib` is `full`-only.
# Running it on `full` gates the env-vs-arg profile-divergence fix: an env-only
# `MOLT_STDLIB_PROFILE=full` request (no `--stdlib-profile` flag) must build a
# real full staticlib, not silently fall back to micro and leave the crypto
# intrinsics undefined at link.
printf 'def main()->None:\n    pass\n\nif __name__=="__main__":\n    main()\n' > "$CORPUS_DIR/empty.py"
printf 'class P:\n    x:int\n    def __init__(self,x:int=0)->None:\n        self.x=x\n\ndef main()->None:\n    i=0\n    while i<1000:\n        p=P(i); i+=1\n    print(i)\n\nif __name__=="__main__":\n    main()\n' > "$CORPUS_DIR/cls.py"
printf 'import json\nd={"a":[1,2,3]}\ns=json.dumps(d)\no=json.loads(s)\nprint(o["a"][0], len(s))\n' > "$CORPUS_DIR/usejson.py"
printf 'import hashlib\nh=hashlib.sha256(b"molt")\nprint(h.hexdigest()[:8])\n' > "$CORPUS_DIR/usehashlib.py"

# file|profiles|expected_stdout. `profiles` is a comma-separated subset of
# {micro,full}. Empty stdout (empty.py) is allowed.
declare -a CORPUS=(
  "empty.py|micro,full|"
  "cls.py|micro,full|1000"
  "usejson.py|micro,full|1 16"
  "usehashlib.py|full|e264730d"
)

fail=0
for profile in micro full; do
  echo "-- dropping stale runtime staticlib + symbol caches (profile-agnostic) --"
  find target -name "libmolt_runtime*.a" -delete 2>/dev/null || true
  find target -name "*intrinsic_symbols*" -delete 2>/dev/null || true
  for entry in "${CORPUS[@]}"; do
    IFS='|' read -r entry_file entry_profiles exp <<<"$entry"
    case ",${entry_profiles}," in
      *",${profile},"*) ;;
      *) continue ;;
    esac
    src="$CORPUS_DIR/${entry_file}"
    out="$CORPUS_DIR/bin_${profile}_$(basename "$src" .py)"
    MOLT_STDLIB_PROFILE="$profile" MOLT_BACKEND_DAEMON=0 "$PY" -m molt build \
      --target native --output "$out" "$src" --rebuild --no-cache \
      > "${out}.log" 2>&1
    bexit=$?
    magic="$(xxd -l4 "$out" 2>/dev/null | awk -F: '{print $2}' | tr -s ' ' | cut -c2-10)"
    "$PY" - "$out" "$exp" "$bexit" "$magic" "$profile" <<'PYEOF'
import subprocess, sys
binp, exp, bexit, magic, profile = sys.argv[1], sys.argv[2], int(sys.argv[3]), sys.argv[4], sys.argv[5]
ok = True; reason = []
if bexit != 0: ok=False; reason.append(f"build rc={bexit}")
if not magic.startswith("cffa edfe"):
    ok=False; reason.append(f"MALFORMED Mach-O magic={magic!r} (want cffa edfe; 0xfeedface=corrupt)")
if ok:
    r = subprocess.run([binp], capture_output=True, text=True)
    if r.returncode != 0: ok=False; reason.append(f"run rc={r.returncode} (negative=signal/SIGKILL)")
    elif exp and r.stdout.strip() != exp: ok=False; reason.append(f"out={r.stdout.strip()!r} want={exp!r}")
tag = "PASS" if ok else "FAIL"
print(f"  [{tag}] {profile}/{binp.split('/')[-1]}: magic={magic!r} {' '.join(reason)}")
sys.exit(0 if ok else 1)
PYEOF
    [ $? -ne 0 ] && fail=1
  done
done

if [ "$fail" -ne 0 ]; then
  echo "== GATE FAILED: at least one binary is corrupt/invalid. DO NOT SHIP. =="
  exit 1
fi
echo "== GATE PASSED: all corpus binaries valid 64-bit Mach-O + correct output across micro+full. =="
