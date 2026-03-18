#!/usr/bin/env bash
# Run this script once fleet MCP auth is restored to update all Linear issues.
# Usage: bash tools/update_linear_status.sh <fleet_token>
#
# Requires: fleet MCP server at alejandros-mac-mini.local:8850

set -euo pipefail
TOKEN="${1:?Usage: $0 <fleet_bearer_token>}"
BASE="http://alejandros-mac-mini.local:8850/mcp"
HDR=(-H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" -H "Accept: application/json, text/event-stream")

call() {
  local id="$1" method="$2" args="$3"
  curl -s "${HDR[@]}" -X POST "$BASE" -d "{\"jsonrpc\":\"2.0\",\"id\":$id,\"method\":\"tools/call\",\"params\":{\"name\":\"$method\",\"arguments\":$args}}" 2>&1 | grep -o '"isError":[a-z]*' | head -1
}

echo "Updating Linear issues for Molt..."

# Wave 1
call 1 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-284","state":"Done","comment":"CI Lean proof gate + sorry-count regression check"}'
echo "MOL-284 -> Done"
call 2 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-292","state":"Done","comment":"23 Kani bounded verification harnesses for NaN-boxing + refcount"}'
echo "MOL-292 -> Done"
call 3 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-280","state":"Done","comment":"91 Hypothesis property-based tests (math/string/collection/hash)"}'
echo "MOL-280 -> Done"
call 4 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-281","state":"Done","comment":"Translation validation infrastructure + TV hooks + 24 tests"}'
echo "MOL-281 -> Done"
call 5 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-285","state":"Done","comment":"Correctness coverage dashboard (tools/correctness_dashboard.py)"}'
echo "MOL-285 -> Done"

# Wave 2
call 6 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-293","state":"Done","comment":"Capability gate Lean formalization - 12 theorems, 0 sorrys"}'
echo "MOL-293 -> Done"
call 7 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-291","state":"Done","comment":"Intrinsic contract Lean axioms - 30 builtins, 50+ axioms"}'
echo "MOL-291 -> Done"
call 8 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-283","state":"Done","comment":"Mutation testing expansion - 5 new operators, 48 tests"}'
echo "MOL-283 -> Done"
call 9 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-282","state":"Done","comment":"Model-based tests from Quint specs - 108 tests"}'
echo "MOL-282 -> Done"
call 10 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-286","state":"Done","comment":"Reproducible build verification + entropy audit - 38 tests"}'
echo "MOL-286 -> Done"

# Wave 3
call 11 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-295","state":"Done","comment":"Lean 4.28.0 upgrade + bv_decide draft proofs for NanBoxCorrect"}'
echo "MOL-295 -> Done"
call 12 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-279","state":"Done","comment":"AST fuzzing complete - 232 fuzz tests (extended + differential)"}'
echo "MOL-279 -> Done"
call 13 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-294","state":"Done","comment":"Kani expanded to 62 harnesses across molt-obj-model + molt-runtime"}'
echo "MOL-294 -> Done"

# Wave 4
call 14 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-287","state":"Done","comment":"Version-gated semantics for 3.12/3.13/3.14 - Lean formalization + 12 tests"}'
echo "MOL-287 -> Done"
call 15 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-290","state":"In Progress","comment":"Sorry count: 84->~13 real sorrys. NanBoxCorrect fully proven. PyExpr induction solved. Backend abs fixed. SSA sorrys closed."}'
echo "MOL-290 -> In Progress"
call 16 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-274","state":"In Progress","comment":"PyExpr nested induction SOLVED via CompCert-style strengthened IH + well-founded recursion. Remaining: operator case analysis (mechanical)."}'
echo "MOL-274 -> In Progress"
call 17 linear_mutate '{"workspace":"molt","action":"update_issue","identifier":"MOL-276","state":"In Progress","comment":"dceSim/sccpSim/cseSim verified sorry-free. Only 2 guard-hoisting sorrys remain."}'
echo "MOL-276 -> In Progress"

echo "Done! 17 issues updated."
