# Generator/Async Generator API Coverage (Python 3.12+)

This matrix tracks generator and async generator API coverage in differential tests.
It focuses on public protocol methods/attributes plus inspect/sys APIs.

Generated on 2026-01-23.

Legend:
- basic: covered by tests/differential/basic
- planned: covered only by tests/differential/planned
- missing: no mapped tests

| API | Status | Evidence |
| --- | --- | --- |
| generator `__iter__`/`__next__` + StopIteration.value | basic | `tests/differential/basic/generator_protocol.py`, `tests/differential/basic/generator_stopiteration_value.py` |
| generator `send` | basic | `tests/differential/basic/generator_protocol.py`, `tests/differential/basic/generator_send_non_none_initial.py`, `tests/differential/basic/generator_send_after_close.py` |
| generator `throw` | basic | `tests/differential/basic/generator_protocol.py`, `tests/differential/basic/generator_throw_signature.py`, `tests/differential/basic/generator_throw_generator_exit.py`, `tests/differential/basic/generator_throw_stopiteration.py` |
| generator `close` | basic | `tests/differential/basic/generator_close_multiple_yields.py`, `tests/differential/basic/generator_close_return_semantics.py`, `tests/differential/basic/generator_close_throw_edge.py`, `tests/differential/basic/generator_close_yield_in_finally.py`, `tests/differential/basic/generator_close_yield_then_raise.py` |
| generator `gi_code`/`gi_frame`/`gi_running`/`gi_yieldfrom` | basic | `tests/differential/basic/generator_introspection_attrs.py` |
| `inspect.getgeneratorstate` | basic | `tests/differential/basic/generator_state.py`, `tests/differential/basic/generator_state_transitions.py` |
| `inspect.getgeneratorlocals` | basic | `tests/differential/basic/generator_introspection_attrs.py` |
| `yield from` delegation (`throw`/`close`) | basic | `tests/differential/basic/yield_from_delegation.py`, `tests/differential/basic/generator_protocol.py` |
| async generator `__aiter__`/`__anext__` | basic | `tests/differential/basic/async_for_iter.py`, `tests/differential/basic/async_generator_protocol.py` |
| async generator `asend`/`athrow`/`aclose` | basic | `tests/differential/basic/async_generator_protocol.py`, `tests/differential/basic/async_generator_post_stop_edges.py`, `tests/differential/basic/async_generator_completion_more.py` |
| async generator `ag_code`/`ag_frame`/`ag_running`/`ag_await` | basic | `tests/differential/basic/async_generator_introspection.py` |
| `inspect.getasyncgenstate` | basic | `tests/differential/basic/inspect_getasyncgenstate.py`, `tests/differential/basic/async_generator_introspection.py` |
| `inspect.getasyncgenlocals` | basic | `tests/differential/basic/async_generator_introspection.py` |
| async generator reentrancy errors | basic | `tests/differential/basic/async_generator_reentrancy.py` |
| `sys.get_asyncgen_hooks`/`sys.set_asyncgen_hooks` | basic | `tests/differential/basic/asyncgen_hooks_api.py` |
