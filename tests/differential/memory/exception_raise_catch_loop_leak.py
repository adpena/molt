# ExceptionRegion Phase 1 model proof (foundation design 45 §1, §8): a bare
# raise-and-immediately-catch loop must run in O(1) memory.
#
# Per caught exception, molt formerly performed 3 inc / 2 dec on the exception's
# owned references → rc=2 leaked per iteration (design 45 §1, the "two
# independently-owned components"):
#
#   • Component A — CreationRef: the `exception_new ValueError(i)` result. Its
#     real last use is the `raise`, but the func_end Swift-ARC last-use extension
#     over-extended it so the per-raise control-flow drain never fired.
#   • Component B — MatchRef: the handler-match reference (`exception_last*` /
#     `exception_active`). Its SSA last use is the re-raise in the no-match ELSE
#     branch that never executes on the caught path, so the single-global-last-use
#     model never released it when the exception WAS caught. Its correct release
#     point is handler-region exit (`exception_pop`) — CPython's implicit clear of
#     the caught exception when the `except` block completes.
#
# The fix releases A at the raise (excluded from the func_end extension) and B at
# the enclosing `exception_pop` (bound on every exit path). With both, each
# iteration's ValueError + its arg tuple + message are freed before the next
# iteration, so `live` plateaus at the immortal-bootstrap floor regardless of N.
#
# Run under:
#   MOLT_ASSERT_NO_LEAK=1 python3 tools/safe_run.py --rss-mb 64 --timeout 60 -- <binary>
# Correct   -> live <= EXPECTED_LIVE_OBJECTS, RSS plateaus far under 64 MiB (exit 0).
# Regressed -> ~2 exception objects leaked per iteration; `live` grows past the
#              200_000 ceiling (MOLT_ASSERT_NO_LEAK aborts) and RSS climbs until
#              the --rss-mb cap trips (exit 137).
#
# On stock origin/main this leaks ~2 objects/iteration (≥ 1_000_000 live at the
# count below → assertion trips / RSS climbs); with the ExceptionRegion fix it is
# O(1). The printed summary is a single integer, byte-identical to CPython.


def raise_catch(n: int) -> int:
    caught = 0
    for i in range(n):
        try:
            raise ValueError(i)
        except ValueError:
            caught += 1
    return caught


def main() -> int:
    return raise_catch(500_000)


print(main())
