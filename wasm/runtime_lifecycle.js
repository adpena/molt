(function installRuntimeLifecycle(root, factory) {
  const api = factory();
  if (typeof module === 'object' && module && module.exports) module.exports = api;
  root.MoltRuntimeLifecycle = api;
})(typeof globalThis !== 'undefined' ? globalThis : this, () => {
  // Thrown JavaScript values need not be Errors, or even safely inspectable.
  // Diagnostics must never replace the original failure with a getter, proxy,
  // JSON or string-conversion failure. Keep the raw value in errors/cause.
  const formatTraceError = (error) => {
    try {
      if (error instanceof Error) {
        const stack = error.stack;
        if (typeof stack === 'string' && stack) return stack;
        const message = error.message;
        if (typeof message === 'string' && message) return message;
      }
    } catch {}
    try {
      const json = JSON.stringify(error);
      if (typeof json === 'string') return json;
    } catch {}
    try { return String(error); } catch {}
    return '<unformattable thrown value>';
  };
  // Keep the first failure as the cause; teardown must never erase it.
  const combinedError = (errors) => {
    if (errors.length === 1) return errors[0];
    return new AggregateError(errors, errors.map(formatTraceError).join('; '),
      { cause: errors[0] });
  };

  const requireExports = (instance, names) => {
    const exports = instance?.exports;
    for (const name of names) {
      if (typeof exports?.[name] !== 'function') {
        throw new Error(`runtime missing ownership export ${name}`);
      }
    }
    return exports;
  };

  // Exact signed-i64 admission precedes the WASM boundary: Number -> BigInt
  // cannot recover precision already lost by JavaScript, and WASM wraps i64.
  const exactHostInt = (value) => {
    if (typeof value === 'number') {
      if (!Number.isSafeInteger(value)) throw new RangeError('host integer Number must be a safe integer');
      value = BigInt(value);
    } else if (typeof value === 'string' && value.trim() === value && /^[+-]?[0-9]+$/.test(value)) {
      value = BigInt(value);
    } else if (typeof value !== 'bigint') {
      throw new TypeError('host integer requires a safe Number, BigInt, or decimal integer string');
    }
    if (value < -(1n << 63n) || value >= (1n << 63n)) {
      throw new RangeError('host integer is outside signed i64');
    }
    return value;
  };

  // The runtime alone owns NaN-box layout and heap-integer allocation. The
  // returned value is owned, even when the runtime chooses an inline encoding.
  const boxRuntimeInt = (instance, value) => {
    const integer = exactHostInt(value);
    const exports = requireExports(instance,
      ['molt_int_from_i64', 'molt_exception_pending_fast', 'molt_dec_ref_obj']);
    if (Number(exports.molt_exception_pending_fast()) !== 0) {
      throw new Error('pending runtime exception before integer allocation');
    }
    const result = exports.molt_int_from_i64(integer);
    try {
      if (Number(exports.molt_exception_pending_fast()) !== 0) {
        throw new Error('integer allocation failed');
      }
    } catch (error) {
      const errors = [error];
      try { exports.molt_dec_ref_obj(result); } catch (cleanup) { errors.push(cleanup); }
      throw combinedError(errors);
    }
    return result;
  };

  // Materialize progressively so a later conversion failure cannot orphan
  // earlier arguments. Normally consume decodes/releases its result. ABI
  // adapters returning a new owner mark resultIsOwned so failed argument
  // cleanup also releases that otherwise-unreachable result.
  const withRuntimeOwnedValues = (instance, values, materialize, consume, resultIsOwned = false) => {
    const exports = requireExports(instance, ['molt_dec_ref_obj']);
    const owned = [];
    const errors = [];
    let result;
    let completed = false;
    try {
      for (const value of values) owned.push(materialize(value));
      result = consume(owned);
      completed = true;
    } catch (error) { errors.push(error); }
    for (let i = owned.length - 1; i >= 0; i--) {
      try { exports.molt_dec_ref_obj(owned[i]); } catch (error) { errors.push(error); }
    }
    if (completed && resultIsOwned && errors.length) {
      try { exports.molt_dec_ref_obj(result); } catch (error) { errors.push(error); }
    }
    if (errors.length) throw combinedError(errors);
    return result;
  };

  // The constructor takes a borrowed Python integer. Its result is an opaque
  // stream handle with explicit drop custody, not a generic object owner.
  const createRuntimeStream = (instance, capacity) => {
    const exports = requireExports(instance, ['molt_stream_new', 'molt_stream_drop',
      'molt_exception_pending_fast']);
    let stream;
    let created = false;
    try {
      return withRuntimeOwnedValues(instance, [capacity],
        value => boxRuntimeInt(instance, value), ([capacityBits]) => {
          stream = exports.molt_stream_new(capacityBits);
          if (Number(exports.molt_exception_pending_fast()) !== 0) {
            throw new Error('stream allocation failed');
          }
          created = true;
          return stream;
        });
    } catch (error) {
      if (!created) throw error;
      const errors = [error];
      try { exports.molt_stream_drop(stream); } catch (cleanup) { errors.push(cleanup); }
      throw combinedError(errors);
    }
  };

  // Borrowed append / consuming finish, shared by every host adapter. Both
  // temporary integer owners and failed unpublished aggregates are released.
  const makeRuntimeIntList = (instance, values) => {
    if (!Array.isArray(values)) throw new TypeError('integer list requires an array');
    const exports = requireExports(instance, ['molt_list_builder_new', 'molt_list_builder_append',
      'molt_list_builder_finish', 'molt_int_from_i64', 'molt_exception_pending_fast', 'molt_dec_ref_obj']);
    const integers = Array.from(values, exactHostInt);
    let builder;
    let created = false;
    let consumed = false;
    let result;
    let finished = false;
    const errors = [];
    try {
      withRuntimeOwnedValues(instance, [integers.length], value => boxRuntimeInt(instance, value), ([capacity]) => {
        builder = exports.molt_list_builder_new(capacity);
        created = true;
        if (Number(exports.molt_exception_pending_fast()) !== 0) {
          throw new Error('list builder allocation failed');
        }
      });
      for (const value of integers) {
        withRuntimeOwnedValues(instance, [value], item => boxRuntimeInt(instance, item), ([item]) => {
          if (Number(exports.molt_list_builder_append(builder, item)) !== 0 ||
              Number(exports.molt_exception_pending_fast()) !== 0) {
            throw new Error('list builder append failed');
          }
        });
      }
      consumed = true;
      result = exports.molt_list_builder_finish(builder);
      finished = true;
      if (Number(exports.molt_exception_pending_fast()) !== 0) {
        throw new Error('list builder finish failed');
      }
    } catch (error) { errors.push(error); }
    if (created && !consumed) {
      try { exports.molt_dec_ref_obj(builder); } catch (error) { errors.push(error); }
    }
    if (finished && errors.length) {
      try { exports.molt_dec_ref_obj(result); } catch (error) { errors.push(error); }
    }
    if (errors.length) throw combinedError(errors);
    return result;
  };

  // A reusable browser owner finalizes its runtime before closing any backing
  // services. Keep this state machine shared by the full and minimal embeds.
  const createRuntimeDisposer = (getLifetime, cleanupActions) => {
    let state = 'live';
    let failed = false;
    let failure;
    return () => {
      if (state !== 'live') {
        if (failed) throw failure;
        return;
      }
      state = 'disposing';
      const errors = [];
      try { getLifetime()?.dispose(); } catch (error) {
        if (error?.code === 'MOLT_RUNTIME_ACTIVE') {
          state = 'live';
          throw error;
        }
        errors.push(error);
      }
      for (const cleanup of cleanupActions) {
        try { cleanup(); } catch (error) { errors.push(error); }
      }
      state = 'disposed';
      if (errors.length) {
        failed = true;
        failure = combinedError(errors);
        throw failure;
      }
    };
  };

  const createRuntimeLifetime = (instance, exportNames, checkPending) => {
    let state = 'live';
    let active = 0;
    let executionEntered = false;
    let disposalError;
    let disposalFailed = false;
    let bindings;
    let admissionError;
    let admissionFailed = false;
    const assertLive = () => {
      if (state !== 'live') throw new Error('Molt runtime has been disposed');
    };
    const resolve = () => {
      if (bindings) return bindings;
      if (admissionFailed) throw admissionError;
      const result = {};
      for (const key of ['runtime_execution_enter', 'runtime_execution_leave', 'runtime_shutdown']) {
        const name = exportNames?.[key];
        if (typeof name !== 'string' || typeof instance?.exports?.[name] !== 'function') {
          admissionFailed = true;
          admissionError = new Error(`runtime missing canonical lifetime export ${key}`);
          throw admissionError;
        }
        result[key] = instance.exports[name];
      }
      bindings = result;
      return bindings;
    };
    const execute = (operation) => {
      assertLive();
      const exports = resolve();
      const token = exports.runtime_execution_enter();
      if (typeof token !== 'bigint' || token === 0n) {
        throw new Error('runtime returned an invalid execution-boundary token');
      }
      executionEntered = true;
      active++;
      const errors = [];
      let value;
      try {
        checkPending();
        value = operation();
        if (value && typeof value.then === 'function') {
          throw new Error('guest execution boundary must be synchronous');
        }
        checkPending();
      } catch (error) {
        errors.push(error);
      } finally {
        try { exports.runtime_execution_leave(token); } catch (error) { errors.push(error); }
        active--;
      }
      // A successful leave may itself publish a deferred runtime failure.
      if (!errors.length) {
        try { checkPending(); } catch (error) { errors.push(error); }
      }
      if (errors.length) throw combinedError(errors);
      return value;
    };
    const initialize = (operation) => {
      assertLive();
      resolve();
      if (active) throw new Error('cannot initialize Molt runtime during active execution');
      // WASI reactor initialization binds libc/host memory and runs constructors
      // that runtime_execution_enter itself depends on. It is lifetime-owned
      // bootstrap, not an application execution boundary.
      active++;
      const errors = [];
      let value;
      try {
        value = operation();
        if (value && typeof value.then === 'function') {
          throw new Error('runtime initialization must be synchronous');
        }
        checkPending();
      } catch (error) {
        errors.push(error);
      } finally {
        active--;
      }
      if (errors.length) throw combinedError(errors);
      return value;
    };
    const dispose = () => {
      if (state !== 'live') {
        if (disposalFailed) throw disposalError;
        return;
      }
      if (active) {
        const error = new Error('cannot dispose Molt runtime during active execution');
        error.code = 'MOLT_RUNTIME_ACTIVE';
        throw error;
      }
      state = 'disposing';
      if (admissionFailed) { state = 'disposed'; return; }
      const errors = [];
      let exports = null;
      try { exports = resolve(); } catch (error) { errors.push(error); }
      // The canonical pending-status query is pure and non-initializing. Every
      // admitted lifetime snapshots it while the runtime objects still exist,
      // including setup and execution-entry failure paths.
      if (exports) {
        try { checkPending(); } catch (error) { errors.push(error); }
        try {
          const status = exports.runtime_shutdown();
          // WASI/libc bootstrap can run before Molt itself leaves Uninitialized.
          // Only a successful execution admission makes shutdown status 0 a
          // refused teardown rather than the canonical unused-runtime result.
          if (status !== 1n && !(status === 0n && !executionEntered)) {
            throw new Error(`runtime shutdown did not complete: status=${String(status)}`);
          }
        } catch (error) { errors.push(error); }
      }
      state = 'disposed';
      if (errors.length) {
        disposalFailed = true;
        disposalError = combinedError(errors);
        throw disposalError;
      }
    };
    return { execute, initialize, dispose, assertLive, admit: resolve };
  };
  return { createRuntimeLifetime, createRuntimeDisposer, boxRuntimeInt, createRuntimeStream,
    withRuntimeOwnedValues, makeRuntimeIntList, combinedError, formatTraceError };
});
