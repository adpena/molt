# Asyncio Coverage Matrix (CPython 3.12+; tracked 3.12-3.14)

This matrix maps asyncio APIs referenced by CPython test_asyncio modules (3.12-3.14)
to Molt differential coverage. Status reflects whether we have at least one
differential test referencing the API in the basic or stdlib lanes.

Generated on 2026-01-22 from:
- third_party/cpython-3.12/Lib/test/test_asyncio
- third_party/cpython-3.13/Lib/test/test_asyncio
- third_party/cpython-3.14/Lib/test/test_asyncio

## Async PEP coverage (3.12+)

Status here reflects differential tests (basic/stdlib) plus direct API coverage
in the table below.

| PEP | Topic | Status | Evidence |
| --- | --- | --- | --- |
| 3156 | asyncio core | partial | Core loop/task/future APIs covered; event loop policies, transports, and protocols missing (see table). |
| 492 | async/await | basic | `tests/differential/basic/async_*`, `tests/differential/stdlib/asyncio_task_basic.py` |
| 525 | async generators | basic | `tests/differential/basic/async_generator_*` |
| 530 | async comprehensions | basic | `tests/differential/basic/async_comprehensions.py`, `tests/differential/basic/async_comprehensions_nested.py` |
| 567 | contextvars | basic | `tests/differential/stdlib/asyncio_task_context_explicit.py`, `tests/differential/stdlib/asyncio_future_callback_context.py`, `tests/differential/stdlib/asyncio_loop_call_soon_context.py`, `tests/differential/stdlib/asyncio_taskgroup_context_name.py` |
| 654 | ExceptionGroup/TaskGroup | basic | `tests/differential/basic/exceptiongroup_basic.py`, `tests/differential/basic/asyncio_taskgroup_*` |
| 703 | free-threading (asyncio interactions) | missing | CPython 3.14 adds `test_free_threading.py`; no Molt coverage yet. |

Legend:
- basic: covered by tests/differential/basic (core/builtins lane)
- stdlib: covered by tests/differential/stdlib (stdlib module lane)
- missing: not referenced in any differential test

Child watcher APIs were removed in CPython 3.12+, so Molt intentionally does not
expose them. Differential coverage asserts absence via the asyncio API surface
tests.

| API | CPython test modules | Molt differential coverage | Status |
| --- | --- | --- | --- |
| `ALL_COMPLETED` | test_tasks.py | basic: asyncio_wait_return_all.py | basic |
| `AbstractEventLoop` | test_events.py, test_free_threading.py, test_unix_events.py | basic: asyncio_loop_exception_handler_debug.py, asyncio_task_factory_naming.py | basic |
| `AbstractEventLoopPolicy` | test_events.py, test_runners.py | basic: asyncio_api_surface_core.py | basic |
| `Barrier` | test_locks.py | basic: asyncio_barrier_basic.py | basic |
| `BaseEventLoop` | test_pep492.py, test_runners.py | basic: asyncio_api_surface_core.py | basic |
| `BaseProtocol` | test_events.py, test_protocols.py, test_unix_events.py | basic: asyncio_api_surface_core.py | basic |
| `BoundedSemaphore` | test_locks.py, test_pep492.py | basic: asyncio_bounded_semaphore_overrelease.py | basic |
| `BrokenBarrierError` | test_locks.py | basic: asyncio_api_surface_core.py | basic |
| `BufferedProtocol` | test_buffered_proto.py, test_protocols.py, test_selector_events.py, test_ssl.py, test_sslproto.py | basic: asyncio_api_surface_core.py | basic |
| `CancelledError` | test_eager_task_factory.py, test_events.py, test_futures.py, test_locks.py, test_queues.py, test_runners.py, test_server.py, test_staggered.py, test_subprocess.py, test_taskgroups.py, test_tasks.py, test_timeouts.py, test_unix_events.py, test_waitfor.py, test_windows_events.py | basic: asyncio_cancel_sleep.py, asyncio_event_cancelled_waiter.py, asyncio_gather_cancel_order.py, asyncio_gather_cancel_sibling.py, asyncio_gather_return_exceptions_cancelled.py, asyncio_shield_cancel.py, asyncio_subprocess_cancel_io.py, asyncio_task_cancel_chain.py, asyncio_task_cancel_message.py, asyncio_taskgroup_cancel_on_error.py, asyncio_taskgroup_cancel_propagation.py, asyncio_wait_for_cancel_propagation.py | basic |
| `Condition` | test_locks.py, test_pep492.py | basic: asyncio_condition_basic.py, asyncio_condition_fairness.py, asyncio_condition_wait_for.py | basic |
| `DatagramProtocol` | test_base_events.py, test_events.py, test_proactor_events.py, test_protocols.py, test_selector_events.py, test_sendfile.py | basic: asyncio_api_surface_core.py | basic |
| `DatagramTransport` | test_proactor_events.py, test_selector_events.py, test_transports.py | basic: asyncio_api_surface_core.py | basic |
| `DefaultEventLoopPolicy` | test_events.py, test_ssl.py, test_unix_events.py | basic: asyncio_api_surface_core.py | basic |
| `Event` | test_events.py, test_locks.py, test_queues.py, test_runners.py, test_ssl.py, test_staggered.py, test_taskgroups.py | basic: asyncio_event_basic.py, asyncio_event_cancelled_waiter.py, asyncio_lock_basic.py, asyncio_queue_basic.py, asyncio_run_pending_task_cleanup.py, asyncio_semaphore_basic.py | basic |
| `EventLoop` | test_eager_task_factory.py, test_runners.py, test_taskgroups.py, test_tasks.py | basic: asyncio_api_surface_core.py | basic |
| `FIRST_COMPLETED` | test_tasks.py | basic: asyncio_wait_basic.py, asyncio_wait_first_completed.py | basic |
| `FIRST_EXCEPTION` | test_tasks.py | basic: asyncio_wait_first_exception.py, asyncio_wait_first_exception_timeout.py | basic |
| `Future` | test_eager_task_factory.py, test_events.py, test_free_threading.py, test_graph.py, test_server.py, test_ssl.py, test_streams.py, test_subprocess.py, test_tasks.py | basic: asyncio_contextvars_callback_propagation.py, asyncio_ensure_future_passthrough.py, asyncio_future_basic.py, asyncio_future_callbacks.py, asyncio_future_cancel_callback.py, asyncio_future_exception_result.py, asyncio_gather_task_future_mix.py, asyncio_loop_read_write_callbacks.py, asyncio_task_future_repr.py | basic |
| `Handle` | test_base_events.py, test_events.py, test_unix_events.py | basic: asyncio_api_surface_core.py | basic |
| `IncompleteReadError` | test_streams.py | basic: asyncio_streams_eof_errors.py | basic |
| `InvalidStateError` | test_futures.py, test_tasks.py | basic: asyncio_api_surface_core.py | basic |
| `LifoQueue` | test_queues.py | basic: asyncio_queue_priority_lifo.py | basic |
| `LimitOverrunError` | test_streams.py | basic: asyncio_api_surface_core.py | basic |
| `Lock` | test_locks.py, test_pep492.py | basic: asyncio_lock_async_with.py, asyncio_lock_basic.py | basic |
| `PriorityQueue` | test_queues.py | basic: asyncio_queue_priority_lifo.py | basic |
| `ProactorEventLoop` | test_buffered_proto.py, test_events.py, test_proactor_events.py, test_runners.py, test_sendfile.py, test_server.py, test_sock_lowlevel.py, test_sslproto.py, test_subprocess.py, test_taskgroups.py, test_windows_events.py | basic: asyncio_api_surface_core.py | basic |
| `Protocol` | test_base_events.py, test_events.py, test_proactor_events.py, test_protocols.py, test_selector_events.py, test_sendfile.py, test_sock_lowlevel.py, test_ssl.py, test_sslproto.py, test_unix_events.py, test_windows_events.py | basic: asyncio_api_surface_core.py | basic |
| `Queue` | test_queues.py | basic: asyncio_queue_basic.py, asyncio_queue_join.py, asyncio_queue_maxsize_block.py, asyncio_queue_task_done_join.py | basic |
| `QueueEmpty` | test_queues.py | basic: asyncio_api_surface_core.py | basic |
| `QueueFull` | test_queues.py | basic: asyncio_api_surface_core.py | basic |
| `QueueShutDown` | test_queues.py | basic: asyncio_api_surface_core.py | basic |
| `Runner` | test_free_threading.py, test_runners.py, test_subprocess.py, test_taskgroups.py | basic: asyncio_run_runner_lifecycle.py, asyncio_runner_get_loop.py | basic |
| `SelectorEventLoop` | test_base_events.py, test_buffered_proto.py, test_events.py, test_runners.py, test_sendfile.py, test_server.py, test_sock_lowlevel.py, test_sslproto.py, test_taskgroups.py, test_unix_events.py, test_windows_events.py | basic: asyncio_api_surface_core.py | basic |
| `Semaphore` | test_locks.py, test_pep492.py | basic: asyncio_semaphore_basic.py | basic |
| `SendfileNotAvailableError` | test_base_events.py, test_proactor_events.py, test_sendfile.py, test_unix_events.py | basic: asyncio_api_surface_core.py | basic |
| `StreamReader` | test_pep492.py, test_ssl.py, test_sslproto.py, test_streams.py, test_windows_events.py | basic: asyncio_streams_basic.py, asyncio_streams_eof_errors.py, asyncio_streams_flow_control.py | basic |
| `StreamReaderProtocol` | test_ssl.py, test_sslproto.py, test_streams.py, test_windows_events.py | basic: asyncio_api_surface_core.py | basic |
| `StreamWriter` | test_ssl.py, test_sslproto.py | basic: asyncio_streams_basic.py, asyncio_streams_eof_errors.py, asyncio_streams_flow_control.py | basic |
| `SubprocessProtocol` | test_base_events.py, test_events.py, test_protocols.py, test_subprocess.py | basic: asyncio_api_surface_core.py | basic |
| `SubprocessTransport` | test_transports.py | basic: asyncio_api_surface_core.py | basic |
| `Task` | test_base_events.py, test_eager_task_factory.py, test_events.py, test_free_threading.py, test_futures2.py, test_graph.py, test_runners.py, test_tasks.py | basic: asyncio_shield_cancel.py, asyncio_task_factory_naming.py | basic |
| `TaskGroup` | test_eager_task_factory.py, test_free_threading.py, test_graph.py, test_locks.py, test_queues.py, test_taskgroups.py, test_timeouts.py | basic: asyncio_taskgroup_basic.py, asyncio_taskgroup_cancel_on_error.py, asyncio_taskgroup_cancel_propagation.py, asyncio_taskgroup_error_order.py, asyncio_taskgroup_exceptiongroup_order.py | basic |
| `TimeoutError` | test_base_events.py, test_locks.py, test_sock_lowlevel.py, test_ssl.py, test_sslproto.py, test_subprocess.py, test_tasks.py, test_timeouts.py, test_waitfor.py | basic: asyncio_shield_wait_for.py, asyncio_wait_for_cancel_propagation.py | basic |
| `TimerHandle` | test_base_events.py, test_events.py | basic: asyncio_api_surface_core.py | basic |
| `Transport` | test_events.py, test_sock_lowlevel.py, test_transports.py, test_windows_events.py | basic: asyncio_api_surface_core.py | basic |
| `WindowsProactorEventLoopPolicy` | test_windows_events.py | basic: asyncio_api_surface_core.py | basic |
| `WindowsSelectorEventLoopPolicy` | test_windows_events.py | basic: asyncio_api_surface_core.py | basic |
| `_get_running_loop` | test_events.py | basic: asyncio_api_surface_modules.py | basic |
| `_set_running_loop` | test_events.py, test_tasks.py, test_unix_events.py | basic: asyncio_api_surface_modules.py | basic |
| `all_tasks` | test_eager_task_factory.py, test_free_threading.py, test_futures2.py, test_tasks.py | basic: asyncio_task_registry_introspection.py | basic |
| `as_completed` | test_tasks.py | basic: asyncio_as_completed_order.py | basic |
| `base_events` | test_futures.py | basic: asyncio_api_surface_modules.py | basic |
| `capture_call_graph` | test_graph.py | basic: asyncio_api_surface_modules.py | basic |
| `create_eager_task_factory` | test_eager_task_factory.py | basic: asyncio_api_surface_modules.py | basic |
| `create_subprocess_exec` | test_streams.py, test_subprocess.py, test_unix_events.py | basic: asyncio_subprocess_cancel_io.py, asyncio_subprocess_exec_shell.py | basic |
| `create_subprocess_shell` | test_subprocess.py | basic: asyncio_subprocess_exec_shell.py | basic |
| `create_task` | test_base_events.py, test_free_threading.py, test_graph.py, test_locks.py, test_queues.py, test_runners.py, test_server.py, test_sock_lowlevel.py, test_subprocess.py, test_taskgroups.py, test_tasks.py, test_timeouts.py, test_unix_events.py, test_waitfor.py | basic: asyncio_as_completed_order.py, asyncio_barrier_basic.py, asyncio_cancel_sleep.py, asyncio_cancel_try_finally.py, asyncio_condition_basic.py, asyncio_condition_fairness.py, asyncio_contextvars_task_propagation.py, asyncio_event_basic.py, asyncio_event_cancelled_waiter.py, asyncio_future_basic.py, asyncio_future_callbacks.py, asyncio_gather_cancel_order.py, asyncio_gather_return_exceptions_cancelled.py, asyncio_gather_task_future_mix.py, asyncio_lock_basic.py, asyncio_run_pending_task_cleanup.py, asyncio_semaphore_basic.py, asyncio_shield_cancel.py, asyncio_shield_wait_for.py, asyncio_subprocess_cancel_io.py, asyncio_task_basic.py, asyncio_task_cancel_chain.py, asyncio_task_cancel_message.py, asyncio_task_current_identity.py, asyncio_task_factory_naming.py, asyncio_task_future_repr.py, asyncio_task_name_set.py, asyncio_task_registry_introspection.py, asyncio_wait_basic.py, asyncio_wait_first_completed.py, asyncio_wait_first_exception.py, asyncio_wait_first_exception_timeout.py, asyncio_wait_for_cancel_propagation.py, asyncio_wait_return_all.py, asyncio_wait_timeout_zero.py, contextvars_task_propagation.py | basic |
| `current_task` | test_eager_task_factory.py, test_free_threading.py, test_graph.py, test_runners.py, test_streams.py, test_taskgroups.py, test_tasks.py, test_timeouts.py, test_unix_events.py | basic: asyncio_shield_cancel.py, asyncio_task_basic.py, asyncio_task_current_identity.py, asyncio_task_registry_introspection.py | basic |
| `eager_task_factory` | test_taskgroups.py | basic: asyncio_api_surface_modules.py | basic |
| `ensure_future` | test_base_events.py, test_futures.py, test_pep492.py, test_tasks.py, test_windows_events.py | basic: asyncio_ensure_future_passthrough.py | basic |
| `events` | test_base_events.py, test_buffered_proto.py, test_context.py, test_eager_task_factory.py, test_events.py, test_free_threading.py, test_futures.py, test_futures2.py, test_graph.py, test_locks.py, test_pep492.py, test_proactor_events.py, test_protocols.py, test_queues.py, test_runners.py, test_selector_events.py, test_sendfile.py, test_server.py, test_sock_lowlevel.py, test_ssl.py, test_sslproto.py, test_staggered.py, test_streams.py, test_subprocess.py, test_taskgroups.py, test_tasks.py, test_threads.py, test_timeouts.py, test_transports.py, test_unix_events.py, test_waitfor.py, test_windows_events.py, test_windows_utils.py | basic: asyncio_api_surface_modules.py | basic |
| `future_add_to_awaited_by` | test_graph.py | basic: asyncio_api_surface_modules.py | basic |
| `future_discard_from_awaited_by` | test_graph.py | basic: asyncio_api_surface_modules.py | basic |
| `futures` | test_events.py, test_free_threading.py, test_graph.py | basic: asyncio_api_surface_modules.py | basic |
| `gather` | test_context.py, test_eager_task_factory.py, test_free_threading.py, test_graph.py, test_locks.py, test_ssl.py, test_streams.py, test_subprocess.py, test_tasks.py, test_threads.py | basic: asyncio_barrier_basic.py, asyncio_condition_basic.py, asyncio_condition_fairness.py, asyncio_condition_wait_for.py, asyncio_event_cancelled_waiter.py, asyncio_gather_basic.py, asyncio_gather_cancel_order.py, asyncio_gather_cancel_sibling.py, asyncio_gather_return_exceptions_cancelled.py, asyncio_gather_task_future_mix.py, asyncio_lock_basic.py, asyncio_queue_basic.py, asyncio_queue_maxsize_block.py, asyncio_semaphore_basic.py, asyncio_wait_first_completed.py, asyncio_wait_first_exception_timeout.py, contextvars_task_propagation.py | basic |
| `get_event_loop` | test_eager_task_factory.py, test_events.py, test_runners.py, test_unix_events.py | basic: asyncio_api_surface_modules.py | basic |
| `get_event_loop_policy` | test_events.py, test_runners.py, test_subprocess.py, test_unix_events.py, test_windows_events.py | basic: asyncio_event_loop_policy_basic.py | basic |
| `get_running_loop` | test_base_events.py, test_eager_task_factory.py, test_events.py, test_free_threading.py, test_futures2.py, test_locks.py, test_queues.py, test_runners.py, test_server.py, test_streams.py, test_subprocess.py, test_tasks.py, test_timeouts.py, test_unix_events.py, test_waitfor.py, test_windows_events.py, utils.py | basic: asyncio_call_later_cancel.py, asyncio_call_soon_order.py, asyncio_contextvars_callback_propagation.py, asyncio_executor_run_in_executor.py, asyncio_future_cancel_callback.py, asyncio_future_exception_result.py, asyncio_get_running_loop_errors.py, asyncio_loop_call_at_time.py, asyncio_loop_exception_handler_debug.py, asyncio_loop_read_write_callbacks.py, asyncio_runner_get_loop.py, asyncio_task_factory_naming.py, asyncio_timeout_at_basic.py | basic |
| `iscoroutine` | test_events.py, test_pep492.py, test_tasks.py | basic: asyncio_api_surface_modules.py | basic |
| `iscoroutinefunction` | test_pep492.py, test_tasks.py | basic: asyncio_api_surface_modules.py | basic |
| `isfuture` | test_futures.py | basic: asyncio_api_surface_modules.py | basic |
| `new_event_loop` | functional.py, test_base_events.py, test_eager_task_factory.py, test_events.py, test_locks.py, test_ssl.py, test_sslproto.py, test_streams.py, test_subprocess.py, test_tasks.py, test_unix_events.py, test_windows_events.py | basic: asyncio_new_event_loop_run_until_complete.py | basic |
| `open_connection` | test_server.py, test_ssl.py, test_sslproto.py, test_streams.py | basic: asyncio_streams_basic.py, asyncio_streams_eof_errors.py, asyncio_streams_flow_control.py | basic |
| `open_unix_connection` | test_streams.py | basic: asyncio_api_surface_modules.py | basic |
| `print_call_graph` | test_graph.py | basic: asyncio_api_surface_modules.py | basic |
| `run` | test_context.py, test_eager_task_factory.py, test_free_threading.py, test_runners.py, test_streams.py, test_subprocess.py, test_tasks.py, test_unix_events.py, test_windows_events.py | basic: async_anext_default_future.py, async_anext_future.py, async_cancellation_token.py, async_closure_decorators.py, async_comprehensions.py, async_comprehensions_nested.py, async_for_else.py, async_for_iter.py, async_for_with_exception_propagation.py, async_generator_asend_after_close.py, async_generator_asend_none_edges.py, async_generator_athrow_after_close.py, async_generator_athrow_after_stop.py, async_generator_close_semantics.py, async_generator_completion_edges.py, async_generator_completion_more.py, async_generator_finalization.py, async_generator_ge_after_stop.py, async_generator_post_stop_edges.py, async_generator_protocol.py, async_hang_probe.py, async_long_running.py, async_loop_sleep.py, async_state_spill_basic.py, async_try_finally.py, async_with_basic.py, async_with_instance_callable.py, async_with_suppress.py, async_yield_spill.py, asyncio_as_completed_order.py, asyncio_barrier_basic.py, asyncio_bounded_semaphore_overrelease.py, asyncio_call_later_cancel.py, asyncio_call_soon_order.py, asyncio_cancel_sleep.py, asyncio_cancel_try_finally.py, asyncio_condition_basic.py, asyncio_condition_fairness.py, asyncio_condition_wait_for.py, asyncio_contextvars_callback_propagation.py, asyncio_contextvars_task_propagation.py, asyncio_ensure_future_passthrough.py, asyncio_event_basic.py, asyncio_event_cancelled_waiter.py, asyncio_event_loop_policy_basic.py, asyncio_executor_run_in_executor.py, asyncio_future_basic.py, asyncio_future_callbacks.py, asyncio_future_cancel_callback.py, asyncio_future_exception_result.py, asyncio_gather_basic.py, asyncio_gather_cancel_order.py, asyncio_gather_cancel_sibling.py, asyncio_gather_return_exceptions_cancelled.py, asyncio_gather_task_future_mix.py, asyncio_lock_async_with.py, asyncio_lock_basic.py, asyncio_loop_call_at_time.py, asyncio_loop_exception_handler_debug.py, asyncio_loop_read_write_callbacks.py, asyncio_queue_basic.py, asyncio_queue_join.py, asyncio_queue_maxsize_block.py, asyncio_queue_priority_lifo.py, asyncio_queue_task_done_join.py, asyncio_run_from_running_loop_error.py, asyncio_run_pending_task_cleanup.py, asyncio_run_shutdown_asyncgens.py, asyncio_semaphore_basic.py, asyncio_shield_cancel.py, asyncio_shield_wait_for.py, asyncio_sleep_result.py, asyncio_streams_basic.py, asyncio_streams_eof_errors.py, asyncio_streams_flow_control.py, asyncio_subprocess_cancel_io.py, asyncio_subprocess_exec_shell.py, asyncio_task_basic.py, asyncio_task_cancel_chain.py, asyncio_task_cancel_message.py, asyncio_task_current_identity.py, asyncio_task_factory_naming.py, asyncio_task_future_repr.py, asyncio_task_name_set.py, asyncio_task_registry_introspection.py, asyncio_taskgroup_basic.py, asyncio_taskgroup_cancel_on_error.py, asyncio_taskgroup_cancel_propagation.py, asyncio_taskgroup_error_order.py, asyncio_taskgroup_exceptiongroup_order.py, asyncio_timeout_at_basic.py, asyncio_timeout_context.py, asyncio_timeout_nested_deadline.py, asyncio_to_thread_propagation.py, asyncio_wait_basic.py, asyncio_wait_first_completed.py, asyncio_wait_first_exception.py, asyncio_wait_first_exception_timeout.py, asyncio_wait_for_basic.py, asyncio_wait_for_cancel_propagation.py, asyncio_wait_for_timeout_edge.py, asyncio_wait_return_all.py, asyncio_wait_timeout_zero.py, composite_task_abi.py, contextlib_async_exitstack.py, contextlib_asynccontextmanager.py, contextvars_task_propagation.py, generator_coroutine_states.py, ifexp.py, inspect_getasyncgenstate.py, inspect_isawaitable.py, nonlocal_and_class_closure.py, pep572_walrus_edges.py | basic |
| `run_coroutine_threadsafe` | test_free_threading.py, test_tasks.py | basic: asyncio_api_surface_modules.py | basic |
| `set_event_loop` | functional.py, test_base_events.py, test_events.py, test_futures.py, test_streams.py, test_tasks.py, test_unix_events.py | basic: asyncio_new_event_loop_run_until_complete.py | basic |
| `set_event_loop_policy` | test_base_events.py, test_buffered_proto.py, test_context.py, test_eager_task_factory.py, test_events.py, test_futures.py, test_futures2.py, test_locks.py, test_pep492.py, test_proactor_events.py, test_protocols.py, test_queues.py, test_runners.py, test_selector_events.py, test_sendfile.py, test_server.py, test_sock_lowlevel.py, test_ssl.py, test_sslproto.py, test_staggered.py, test_streams.py, test_subprocess.py, test_taskgroups.py, test_tasks.py, test_threads.py, test_timeouts.py, test_transports.py, test_unix_events.py, test_waitfor.py, test_windows_events.py, test_windows_utils.py | basic: asyncio_event_loop_policy_basic.py | basic |
| `shield` | test_graph.py, test_tasks.py, test_waitfor.py | basic: asyncio_shield_cancel.py, asyncio_shield_wait_for.py | basic |
| `sleep` | functional.py, test_base_events.py, test_context.py, test_eager_task_factory.py, test_events.py, test_free_threading.py, test_futures2.py, test_graph.py, test_locks.py, test_pep492.py, test_queues.py, test_runners.py, test_selector_events.py, test_server.py, test_sock_lowlevel.py, test_ssl.py, test_sslproto.py, test_staggered.py, test_streams.py, test_subprocess.py, test_taskgroups.py, test_tasks.py, test_timeouts.py, test_unix_events.py, test_waitfor.py, test_windows_events.py, utils.py | basic: async_cancellation_token.py, async_closure_decorators.py, async_comprehensions.py, async_comprehensions_nested.py, async_for_iter.py, async_hang_probe.py, async_long_running.py, async_loop_sleep.py, async_state_spill_basic.py, async_try_finally.py, async_yield_spill.py, asyncio_as_completed_order.py, asyncio_call_later_cancel.py, asyncio_call_soon_order.py, asyncio_cancel_sleep.py, asyncio_cancel_try_finally.py, asyncio_condition_basic.py, asyncio_condition_fairness.py, asyncio_condition_wait_for.py, asyncio_event_basic.py, asyncio_event_cancelled_waiter.py, asyncio_future_basic.py, asyncio_future_callbacks.py, asyncio_future_cancel_callback.py, asyncio_gather_basic.py, asyncio_gather_cancel_order.py, asyncio_gather_cancel_sibling.py, asyncio_gather_return_exceptions_cancelled.py, asyncio_gather_task_future_mix.py, asyncio_lock_basic.py, asyncio_loop_call_at_time.py, asyncio_loop_exception_handler_debug.py, asyncio_new_event_loop_run_until_complete.py, asyncio_queue_maxsize_block.py, asyncio_run_from_running_loop_error.py, asyncio_run_pending_task_cleanup.py, asyncio_run_runner_lifecycle.py, asyncio_run_shutdown_asyncgens.py, asyncio_semaphore_basic.py, asyncio_shield_cancel.py, asyncio_shield_wait_for.py, asyncio_sleep_result.py, asyncio_subprocess_cancel_io.py, asyncio_task_basic.py, asyncio_task_cancel_chain.py, asyncio_task_cancel_message.py, asyncio_task_factory_naming.py, asyncio_task_future_repr.py, asyncio_task_name_set.py, asyncio_task_registry_introspection.py, asyncio_taskgroup_basic.py, asyncio_taskgroup_cancel_on_error.py, asyncio_taskgroup_cancel_propagation.py, asyncio_taskgroup_error_order.py, asyncio_taskgroup_exceptiongroup_order.py, asyncio_timeout_at_basic.py, asyncio_timeout_context.py, asyncio_timeout_nested_deadline.py, asyncio_wait_basic.py, asyncio_wait_first_completed.py, asyncio_wait_first_exception.py, asyncio_wait_first_exception_timeout.py, asyncio_wait_for_basic.py, asyncio_wait_for_cancel_propagation.py, asyncio_wait_for_timeout_edge.py, asyncio_wait_return_all.py, asyncio_wait_timeout_zero.py, composite_task_abi.py, contextvars_task_propagation.py, generator_coroutine_states.py, ifexp.py, inspect_iscoroutinefunction.py, pep572_walrus_edges.py | basic |
| `staggered` | test_eager_task_factory.py | basic: asyncio_api_surface_modules.py | basic |
| `start_server` | test_base_events.py, test_buffered_proto.py, test_server.py, test_ssl.py, test_streams.py | basic: asyncio_streams_basic.py, asyncio_streams_eof_errors.py, asyncio_streams_flow_control.py | basic |
| `start_unix_server` | test_server.py, test_streams.py | basic: asyncio_api_surface_modules.py | basic |
| `streams` | test_streams.py | basic: asyncio_api_surface_modules.py | basic |
| `subprocess` | test_subprocess.py | basic: asyncio_subprocess_cancel_io.py, asyncio_subprocess_exec_shell.py | basic |
| `tasks` | test_eager_task_factory.py, test_events.py, test_free_threading.py, test_graph.py, test_unix_events.py | basic: asyncio_api_surface_modules.py | basic |
| `timeout` | test_graph.py, test_locks.py, test_staggered.py, test_taskgroups.py, test_timeouts.py | basic: asyncio_timeout_context.py, asyncio_timeout_nested_deadline.py | basic |
| `timeout_at` | test_timeouts.py | basic: asyncio_timeout_at_basic.py | basic |
| `to_thread` | test_free_threading.py, test_subprocess.py, test_threads.py | basic: asyncio_to_thread_propagation.py | basic |
| `trsock` | test_events.py | basic: asyncio_api_surface_modules.py | basic |
| `unix_events` | utils.py | basic: asyncio_api_surface_modules.py | basic |
| `wait` | test_graph.py, test_locks.py, test_streams.py, test_tasks.py, test_waitfor.py | basic: asyncio_wait_basic.py, asyncio_wait_first_completed.py, asyncio_wait_first_exception.py, asyncio_wait_first_exception_timeout.py, asyncio_wait_return_all.py, asyncio_wait_timeout_zero.py | basic |
| `wait_for` | test_buffered_proto.py, test_events.py, test_futures2.py, test_locks.py, test_queues.py, test_sock_lowlevel.py, test_ssl.py, test_sslproto.py, test_subprocess.py, test_waitfor.py | basic: asyncio_loop_read_write_callbacks.py, asyncio_shield_wait_for.py, asyncio_streams_flow_control.py, asyncio_wait_for_basic.py, asyncio_wait_for_cancel_propagation.py, asyncio_wait_for_timeout_edge.py | basic |
| `windows_events` | utils.py | basic: asyncio_api_surface_modules.py | basic |
| `wrap_future` | test_futures.py | basic: asyncio_api_surface_modules.py | basic |

## Missing API surface

These asyncio APIs appear in CPython test_asyncio coverage but are not referenced
by any Molt differential test yet:

None
