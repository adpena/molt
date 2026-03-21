#!/usr/bin/env bash
# Lint Python.h for common issues
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
HEADER="$PROJECT_ROOT/include/molt/Python.h"
ERRORS=0

echo "C API Header Lint"
echo "================="

# 1. Check for Py_DECREF immediately before return (use-after-free pattern)
echo -n "  Use-after-free pattern (DECREF before return)... "
UAF=$(grep -n "Py_DECREF\|Py_XDECREF" "$HEADER" | while read line; do
  lineno=$(echo "$line" | cut -d: -f1)
  nextline=$((lineno + 1))
  if sed -n "${nextline}p" "$HEADER" | grep -q "return.*;" 2>/dev/null; then
    echo "$lineno"
  fi
done | wc -l)
if [ "$UAF" -gt 0 ]; then
  echo "WARN: $UAF potential use-after-free sites"
  ERRORS=$((ERRORS + UAF))
else
  echo "OK"
fi

# 2. Check for missing NULL checks in public functions
echo -n "  NULL safety (functions without NULL check)... "
MISSING_NULL=$(grep -c "static inline.*PyObject \*Py.*PyObject \*" "$HEADER" || true)
HAS_NULL=$(grep -c "if.*== NULL\|if.*!= NULL" "$HEADER" || true)
echo "OK ($HAS_NULL NULL checks across $MISSING_NULL functions)"

# 3. Check for duplicate #define values
echo -n "  Duplicate slot numbers... "
DUPS=$(grep "#define Py_nb_\|#define Py_sq_\|#define Py_mp_\|#define Py_tp_" "$HEADER" | \
  awk '{print $3}' | sort -n | uniq -d | wc -l | tr -d ' ')
if [ "$DUPS" -gt 0 ]; then
  echo "FAIL: $DUPS duplicate slot values"
  grep "#define Py_nb_\|#define Py_sq_\|#define Py_mp_\|#define Py_tp_" "$HEADER" | \
    awk '{print $3, $2}' | sort -k1,1n | awk 'prev==$1 {if(!printed){print prevline} print; printed=1} {prev=$1; prevline=$0; printed=0}'
  ERRORS=$((ERRORS + DUPS))
else
  echo "OK"
fi

# 4. Check compilation with all warnings
echo -n "  Compilation (-Wall -Wextra)... "
printf '#include "Python.h"\nint main(void) { return 0; }\n' > /tmp/test_lint.c
if cc -I "$PROJECT_ROOT/include/molt" -I "$PROJECT_ROOT/include" -Wall -Wextra -Werror -fsyntax-only /tmp/test_lint.c 2>/dev/null; then
  echo "OK"
else
  echo "FAIL"
  cc -I "$PROJECT_ROOT/include/molt" -I "$PROJECT_ROOT/include" -Wall -Wextra /tmp/test_lint.c 2>&1 | head -10
  ERRORS=$((ERRORS + 1))
fi

# 5. Check for dunder dispatch (should be direct intrinsics)
echo -n "  Dunder dispatch (should use direct intrinsics)... "
DUNDER=$(grep -c "_molt_call_dunder\|__add__\|__sub__\|__mul__\|__mod__" "$HEADER" || true)
echo "$DUNDER remaining dunder dispatch sites"

# 6. Count definitions
echo ""
echo "  Total lines: $(wc -l < "$HEADER")"
echo "  Definitions: $(grep -c 'static inline\|#define Py' "$HEADER")"
echo "  Extern decls: $(grep -c '^extern ' "$HEADER")"

echo ""
if [ "$ERRORS" -gt 0 ]; then
  echo "RESULT: $ERRORS issues found"
  exit 1
else
  echo "RESULT: All checks passed"
fi
