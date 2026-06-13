# ExceptionRegion Phase 1 model proof (foundation design 45 §1, §8): a bare
# raise-and-immediately-catch loop must run in O(1) memory.
#
# Per caught exception, molt formerly performed 3 inc / 2 dec on the exception's
# owned references → rc=2 leaked per iteration (design 45 §1, the "two
# independently-owned components"):
#
#   - Component A: CreationRef, the `exception_new ValueError(i)` result. Its
#     real last use is the `raise`, but the func_end Swift-ARC last-use extension
#     over-extended it so the per-raise control-flow drain never fired.
#   - Component B: MatchRef, the handler-match reference (`exception_last*` /
#     `exception_active`). Its SSA last use is the re-raise in the no-match ELSE
#     branch that never executes on the caught path, so the single-global-last-use
#     model never released it when the exception was caught. Its correct release
#     point is handler-region exit (`exception_pop`), mirroring CPython's implicit
#     clear of the caught exception when the `except` block completes.
#
# The fix releases A at the raise and B at the enclosing `exception_pop`. With
# both, each iteration's exception and attached payloads are freed before the next
# iteration, so `live` plateaus at the immortal-bootstrap floor regardless of N.
#
# Run under:
#   MOLT_ASSERT_NO_LEAK=1 python3 tools/safe_run.py --rss-mb 64 --timeout 60 -- <binary>
# Correct   -> live <= EXPECTED_LIVE_OBJECTS, RSS plateaus far under 64 MiB.
# Regressed -> exception objects leak per iteration until MOLT_ASSERT_NO_LEAK or
#              the RSS cap trips.
#
# Four shapes are covered so a fix cannot regress one handler form while passing
# another: (1) bare raise/catch plateau; (2) `except ... as e` with a read of the
# bound name; (3) `except T:` with no `as` binding, proving the leak is not just
# implicit `del e`; and (4) a chained re-raise whose __cause__/__context__ must
# remain reachable until both handlers exit.


def raise_catch(n):
    caught = 0
    for i in range(n):
        try:
            raise ValueError(i)
        except ValueError:
            caught += 1
    return caught


def raise_catch_as(n):
    total = 0
    for i in range(n):
        try:
            raise ValueError(i)
        except ValueError as e:
            total += int(str(e))
    return total


def raise_catch_no_as(n):
    count = 0
    for i in range(n):
        try:
            raise KeyError(i)
        except KeyError:
            count += 1
    return count


def raise_catch_chain(n):
    handled = 0
    for i in range(n):
        try:
            try:
                raise ValueError(i)
            except ValueError as inner:
                raise RuntimeError("wrap") from inner
        except RuntimeError as outer:
            if outer.__cause__ is not None:
                handled += 1
    return handled


print(raise_catch(500000))
print(raise_catch_as(200000))
print(raise_catch_no_as(200000))
print(raise_catch_chain(120000))
