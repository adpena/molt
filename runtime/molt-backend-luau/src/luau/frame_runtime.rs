/// Luau's internal execution-frame authority.
///
/// Each coroutine owns a reusable context containing code/source-location,
/// exact code identity, explicit globals/builtins/locals, and unwind custody.
/// Callable handoffs are slot-keyed and do not create visible frames. This intentionally
/// does not expose Python frame objects, tracing hooks, or
/// `__traceback__.tb_frame`; those require a separate exact introspection
/// capability that Luau rejects before source generation.
pub(super) const FRAME_RUNTIME: &str = r#"
-- Luau does not implement ephemeron tables, so this lookup is non-owning in
-- both directions. Local frames and wrappers strongly own live contexts; the
-- root coroutine has the separate strong slot below.
local molt_frame_contexts: {[any]: any} = setmetatable({}, {__mode = "kv"})
local molt_main_context_key = coroutine.running() or {}
local molt_main_context: any = nil
local molt_frame_context_allocations = 0

local function molt_frame_invariant(message: string): never
	error({__type="RuntimeError", __msg="Luau execution-frame invariant: " .. message}, 0)
end

local function molt_frame_new_context(): any
	molt_frame_context_allocations += 1
	return {depth=0, codes={}, lines={}, lastis={}, cols={}, end_cols={}, globals={}, builtins={}, locals={}, invocations={}, exceptions={handlers={}, baseline=0, pending=nil, inherited=nil}}
end

local function molt_frame_owned_context(owner: any): any
	return if owner == molt_main_context_key then molt_main_context else molt_frame_contexts[owner]
end

local function molt_frame_forget_context(owner: any): nil
	if owner == molt_main_context_key then
		molt_main_context = nil
	else molt_frame_contexts[owner] = nil end
	return nil
end

local function molt_frame_context(): (any, any)
	local key: any = coroutine.running() or molt_main_context_key
	if key == molt_main_context_key then
		molt_main_context = molt_main_context or molt_frame_new_context()
		return molt_main_context, key
	end
	local context = molt_frame_contexts[key]
	if context == nil then
		context = molt_frame_new_context()
		molt_frame_contexts[key] = context
	end
	return context, key
end

-- An invocation transports one exact activation to its compiled slot. It
-- never pushes a Python frame; mismatching entries cannot consume the owner.
local function molt_frame_invoke(func: any, code: any, globals: any, builtins: any, ...): ...any
	if type(code) ~= "table" or code.__molt_frame_slot == nil then return func(...) end
	if type(globals) ~= "table" then
		molt_frame_invariant("compiled invocation has no globals dictionary")
	end
	local context = molt_frame_context()
	local pending = context.invocations
	local depth = #pending + 1
	local entry = {slot=code.__molt_frame_slot, code=code, globals=globals, builtins=builtins}
	pending[depth] = entry
	local results = table.pack(pcall(func, ...))
	if #pending ~= depth or pending[depth] ~= entry then
		molt_frame_invariant("compiled invocation custody is not LIFO")
	end
	pending[depth] = nil
	if not results[1] then error(results[2], 0) end
	return table.unpack(results, 2, results.n)
end

local function molt_frame_enter(slot: any, lexical_builtins: any?): (any, number, any, any)
	if type(slot) ~= "table" or type(slot.code) ~= "table" or slot.code.co_name == nil then
		molt_frame_invariant("trace_enter_slot references an unbound code object")
	end
	if type(slot.globals) ~= "table" then
		molt_frame_invariant("trace_enter_slot references an unbound globals dictionary")
	end
	local code = slot.code
	local context, owner = molt_frame_context()
	local globals = slot.globals
	local builtins = lexical_builtins
	local pending = context.invocations[#context.invocations]
	if pending ~= nil and pending.slot == slot.id and pending.code ~= nil then
		code = pending.code
		globals = pending.globals
		builtins = pending.builtins
		pending.code = nil
		pending.globals = nil
		pending.builtins = nil
	end
	context.depth += 1
	local index = context.depth
	context.codes[index] = code
	context.lines[index] = if type(code.co_firstlineno) == "number" then code.co_firstlineno else 0
	context.lastis[index] = 0
	context.cols[index] = -1
	context.end_cols[index] = -1
	context.globals[index] = globals
	context.builtins[index] = builtins
	context.locals[index] = nil
	return context, index, code, owner
end

local function molt_frame_set_line(context: any, line: number, col: number?, final_col: number?): nil
	local index = context.depth
	if index < 1 then
		molt_frame_invariant("line requires an active Python execution frame")
	end
	if type(line) ~= "number" then
		molt_frame_invariant("line requires a numeric source line")
	end
	context.lines[index] = line
	context.lastis[index] += 2
	context.cols[index] = col or -1
	context.end_cols[index] = final_col or -1
	return nil
end

-- Internal namespace projection used by compiler-generated global stores.
local function molt_globals_builtin(): any
	local context = molt_frame_context()
	if context.depth < 1 then molt_frame_invariant("globals requires an active frame") end
	return context.globals[context.depth]
end

local function molt_frame_locals_set(context: any, locals_value: any): nil
	local index = context.depth
	if index < 1 then
		molt_frame_invariant("frame_locals_set requires an active Python execution frame")
	end
	if type(locals_value) ~= "table" then
		molt_frame_invariant("frame_locals_set requires a locals dictionary")
	end
	context.locals[index] = locals_value
	return nil
end

local function molt_frame_exit(context: any, entry_depth: number, code: any, owner: any): nil
	local index = context.depth
	local thread = coroutine.running()
	local current_owner: any = thread or molt_main_context_key
	if owner ~= current_owner or molt_frame_owned_context(owner) ~= context or index < 1 or index ~= entry_depth or context.codes[index] ~= code then
		molt_frame_invariant("trace_exit cookie does not match the active execution-context frame")
	end
	context.codes[index] = nil
	context.lines[index] = nil
	context.lastis[index] = nil
	context.cols[index] = nil
	context.end_cols[index] = nil
	context.globals[index] = nil
	context.builtins[index] = nil
	context.locals[index] = nil
	context.depth -= 1
	return nil
end

local function molt_frame_restore_depth(context: any, depth: number): nil
	if type(depth) ~= "number" or depth < 0 or depth > context.depth or depth ~= math.floor(depth) then
		molt_frame_invariant("unwind depth is outside the active execution-context stack")
	end
	while context.depth > depth do
		local index = context.depth
		context.codes[index] = nil
		context.lines[index] = nil
		context.lastis[index] = nil
		context.cols[index] = nil
		context.end_cols[index] = nil
		context.globals[index] = nil
		context.builtins[index] = nil
		context.locals[index] = nil
		context.depth -= 1
	end
	return nil
end

local function molt_exception_attach_traceback(context: any, error_value: any): any
	local exception = error_value
	if type(exception) ~= "table" then
		exception = {__type="RuntimeError", __msg=tostring(error_value)}
	end
	if rawget(exception, "__molt_traceback_locations") ~= nil then
		return exception
	end
	local locations = table.create(context.depth)
	for index = 1, context.depth do
		local code = context.codes[index]
		locations[index] = {
			filename = code.co_filename,
			name = code.co_name,
			line = context.lines[index],
			lasti = context.lastis[index],
			col_offset = context.cols[index],
			end_col_offset = context.end_cols[index],
		}
	end
	rawset(exception, "__molt_traceback_locations", locations)
	return exception
end

local function molt_frame_finalize(context: any, owner: any, baseline_depth: number, error_value: any, attach_traceback: boolean): (any, boolean)
	local exception = error_value
	if attach_traceback then
		local attachment = table.pack(pcall(molt_exception_attach_traceback, context, error_value))
		exception = attachment[2]
		if not attachment[1] then
			exception = {
				__type="RuntimeError",
				__msg="traceback attachment failed",
				__cause__=error_value,
				__molt_traceback_attachment_error=tostring(attachment[2]),
			}
		end
	end
	local restoration = table.pack(pcall(molt_frame_restore_depth, context, baseline_depth))
	if not restoration[1] then
		molt_frame_forget_context(owner)
		return {
			__type="RuntimeError",
			__msg="execution-frame restoration failed",
			__cause__=exception,
			__molt_frame_restoration_error=tostring(restoration[2]),
		}, false
	end
	return exception, true
end

-- Project the explicit runtime exception protocol onto the same coroutine-owned
-- context as Python frames. Pending propagation and dynamically handled state
-- are independent: clearing a pending exception never clears an outer handler.
local function molt_exception_stack_depth(): number
	return #molt_frame_context().exceptions.handlers
end

local function molt_exception_stack_enter(): number
	local state = molt_frame_context().exceptions
	local previous = state.baseline
	state.baseline = #state.handlers
	return previous
end

local function molt_exception_stack_exit(previous: any): nil
	molt_frame_context().exceptions.baseline = if type(previous) == "number" and previous >= 0 then previous else 0
	return nil
end

local function molt_exception_stack_set_depth(depth: any): nil
	local state = molt_frame_context().exceptions
	local target = if type(depth) == "number" and depth >= 0 then depth else 0
	while #state.handlers > target do state.handlers[#state.handlers] = nil end
	while #state.handlers < target do state.handlers[#state.handlers + 1] = {} end
	return nil
end

local function molt_exception_stack_clear(): nil
	return molt_exception_stack_set_depth(molt_frame_context().exceptions.baseline)
end

local function molt_exception_push(): nil
	local handlers = molt_frame_context().exceptions.handlers
	handlers[#handlers + 1] = {}
	return nil
end

local function molt_exception_pop(): nil
	local state = molt_frame_context().exceptions
	if #state.handlers > state.baseline then state.handlers[#state.handlers] = nil end
	return nil
end

local function molt_exception_context_set(exception: any): nil
	local handlers = molt_frame_context().exceptions.handlers
	if #handlers > 0 then handlers[#handlers].exception = exception end
	return nil
end

local function molt_exception_active(): any
	local state = molt_frame_context().exceptions
	for index = #state.handlers, 1, -1 do
		if state.handlers[index].exception ~= nil then return state.handlers[index].exception end
	end
	return state.inherited
end

local function molt_exception_pending(): boolean
	return molt_frame_context().exceptions.pending ~= nil
end

local function molt_exception_last_pending(): any
	return molt_frame_context().exceptions.pending
end

local function molt_exception_clear(): nil
	molt_frame_context().exceptions.pending = nil
	return nil
end

local function molt_exception_last(): any
	local state = molt_frame_context().exceptions
	local active = molt_exception_active()
	if state.pending == nil then return active end
	local exception = state.pending
	if #state.handlers > 0 then
		exception = active or exception
		molt_exception_context_set(exception)
		state.pending = nil
	end
	return exception
end

local function molt_exception_current(): any
	return molt_exception_active() or molt_exception_last()
end

local function molt_exception_set_last(exception: any): nil
	local context = molt_frame_context()
	if exception == nil then context.exceptions.pending = nil; return nil end
	local active = molt_exception_active()
	exception = molt_exception_attach_traceback(context, exception)
	if active ~= nil and active ~= exception and rawget(exception, "__context__") == nil then
		rawset(exception, "__context__", active)
	end
	context.exceptions.pending = exception
	return nil
end

local function molt_exception_set_value(exception: any, value: any): nil
	if type(exception) ~= "table" or exception.__type ~= "StopIteration" then
		return molt_exception_set_last({__type="TypeError", __msg="exception value field requires StopIteration layout"})
	end
	exception.value = value
	return nil
end

local function molt_exception_set_cause(exception: any, cause: any): nil
	exception.__cause__ = cause
	exception.__suppress_context__ = true
	return nil
end

local function molt_exception_reraise(): nil
	return molt_exception_set_last(molt_exception_active() or {__type="RuntimeError", __msg="No active exception to reraise"})
end

local function molt_exception_propagate(): nil
	local pending = molt_exception_last_pending()
	if pending ~= nil then error(pending, 0) end
	return nil
end

local function molt_exception_capture(context: any, owner: any, frame_depth: number, exception_depth: number, baseline: number, error_value: any): nil
	local exception, restored = molt_frame_finalize(context, owner, frame_depth, error_value, true)
	-- A poisoned execution context was forgotten by finalization. Never publish
	-- into a replacement context and continue as though its custody survived.
	if not restored then error(exception, 0) end
	-- A host throw can bypass a callee's explicit cleanup. Restore only the
	-- caller-owned baseline captured at the operation boundary, then publish.
	-- Capture dynamic context before discarding the failed callee's handlers.
	molt_exception_set_last(exception)
	molt_exception_stack_set_depth(exception_depth)
	molt_exception_stack_exit(baseline)
	return nil
end

-- Own the execution boundary inside the new coroutine. The resume closure only
-- transports yields and rethrows the exact stored exception after the
-- coroutine has attached locations and restored its own context.
local function molt_coroutine_execution_wrap(func: (...any) -> ...any): ((...any) -> ...any, () -> nil)
	local execution_context: any = nil
	local execution_owner: any = nil
	local baseline_depth = 0
	local context_restored = false
	local context_restore_attempted = false
	local finalized = false
	local pending_error: any = nil
	local resume_handled_exception: any = nil
	local thread: any = coroutine.create(function(...)
		local context, owner = molt_frame_context()
		execution_context = context
		execution_owner = owner
		context.exceptions.inherited = resume_handled_exception
		baseline_depth = context.depth
		local function on_error(error_value: any): any
			context_restore_attempted = true
			local exception, restored = molt_frame_finalize(context, owner, baseline_depth, error_value, true)
			context_restored = restored
			pending_error = exception
			return exception
		end
		local results = table.pack(xpcall(func, on_error, ...))
		if results[1] then
			context_restore_attempted = true
			local restoration_error, restored = molt_frame_finalize(context, owner, baseline_depth, nil, false)
			context_restored = restored
			if restoration_error ~= nil then
				pending_error = restoration_error
				error(restoration_error, 0)
			end
			return table.unpack(results, 2, results.n)
		end
		return nil
	end)
	local function finalize(close_suspended: boolean): any
		if finalized then return nil end
		local close_error: any = nil
		if close_suspended and thread ~= nil and coroutine.status(thread) ~= "dead" then
			local close_ok, error_value = coroutine.close(thread)
			if not close_ok then close_error = error_value end
		end
		if execution_context ~= nil and not context_restored and not context_restore_attempted then
			context_restore_attempted = true
			local restoration_error, restored = molt_frame_finalize(execution_context, execution_owner, baseline_depth, nil, false)
			context_restored = restored
			if restoration_error ~= nil and close_error == nil then close_error = restoration_error end
		end
		finalized = true
		-- Terminal wrappers no longer own a resumable context. Detach the
		-- non-owning index now; GC scheduling must not govern terminal custody.
		if execution_owner ~= nil then molt_frame_forget_context(execution_owner) end
		execution_context = nil
		execution_owner = nil
		thread = nil
		return close_error
	end
	local function resume(...)
		if finalized or thread == nil then
			error({__type="RuntimeError", __msg="cannot resume finalized coroutine"}, 0)
		end
		resume_handled_exception = molt_exception_active()
		if execution_context ~= nil then execution_context.exceptions.inherited = resume_handled_exception end
		local results = table.pack(coroutine.resume(thread, ...))
		resume_handled_exception = nil
		if execution_context ~= nil then execution_context.exceptions.inherited = nil end
		if not results[1] then
			local resume_error = pending_error or results[2]
			pending_error = nil
			finalize(false)
			error(resume_error, 0)
		end
		if coroutine.status(thread) == "dead" then
			local error_to_raise = pending_error
			pending_error = nil
			finalize(false)
			if error_to_raise ~= nil then error(error_to_raise, 0) end
		end
		return table.unpack(results, 2, results.n)
	end
	local function close(): nil
		local close_error = finalize(true)
		if close_error ~= nil then error(close_error, 0) end
		return nil
	end
	return resume, close
end
"#;

/// Definition-time namespace capture shares one callable-frame authority.
/// Emitted after dictionary helpers and the module cache exist.
pub(super) const CALLABLE_FRAME_RUNTIME: &str = r#"
local function molt_frame_namespace_get(namespace: any, name: any): (boolean, any)
	if type(namespace) ~= "table" then return false, nil end
	if molt_dict_is_ordered(namespace) then
		if molt_dict_contains(namespace, name) then return true, molt_dict_getitem(namespace, name) end
		return false, nil
	end
	local value = rawget(namespace, name)
	return value ~= nil, value
end

local function molt_frame_effective_builtins(globals: any): any
	local present, selected = molt_frame_namespace_get(globals, "__builtins__")
	if present then return selected end
	local context = molt_frame_context()
	if context.depth > 0 then return context.builtins[context.depth] end
	return molt_module_cache["builtins"]
end

local function molt_frame_bind_code(id: number, code: any, globals: any): any
	if type(code) ~= "table" or code.co_name == nil or type(globals) ~= "table" then
		molt_frame_invariant("code slot requires code and globals")
	end
	if code.__molt_frame_slot ~= nil and code.__molt_frame_slot ~= id then
		molt_frame_invariant("code object cannot move between compiled slots")
	end
	code.__molt_frame_slot = id
	return {id=id, code=code, globals=globals}
end

local function molt_frame_enter_slot(slot: any): (any, number, any, any)
	local globals = if type(slot) == "table" then slot.globals else nil
	return molt_frame_enter(slot, molt_frame_effective_builtins(globals))
end

-- Each definition has independent metadata even when its machine target is
-- shared. The wrapper owns its attributes directly; no reverse strong map or
-- additional Python frame is introduced.
local molt_frame_function_context_key = {}

local function molt_frame_function_capture(func: any, module_name: any): nil
	local attrs = molt_func_attrs[func]
	local link = if attrs ~= nil then attrs[molt_frame_function_context_key] else nil
	-- Runtime helpers have metadata but do not own compiled Python activations.
	if link == nil then return nil end
	local captured = link.value
	if captured == nil then molt_frame_invariant("live callable lost its owned context") end
	local context = molt_frame_context()
	local globals = if context.depth > 0 then context.globals[context.depth] else molt_module_cache[module_name]
	captured.globals = globals
	captured.builtins = molt_frame_effective_builtins(globals)
	return nil
end

local function molt_frame_function_new(target: any): any
	-- Luau weak-key tables are not ephemerons. Keep namespace owners only in
	-- the callable closure. The existing attrs lookup has a weak, non-owning
	-- link so metadata can finalize custody after default expressions execute.
	local captured = {}
	local attrs = {[molt_frame_function_context_key]=setmetatable({value=captured}, {__mode="v"})}
	local function callable(...): ...any
		return molt_frame_invoke(target, attrs.__code__, captured.globals, captured.builtins, ...)
	end
	molt_func_attrs[callable] = attrs
	local signature = molt_function_metadata[target]
	if signature ~= nil then molt_function_metadata[callable] = table.clone(signature) end
	return callable
end
"#;

/// Packed code metadata and mutable callable defaults form a separate runtime
/// slice from argument binding. Keeping this slice here makes callable-frame
/// custody explicit without charging unrelated container/callargs programs for
/// metadata publication.
pub(super) const CALLABLE_METADATA_RUNTIME: &str = r#"local function molt_function_init_metadata_packed(func: any, metadata: any, _code: any, bind_kind: any): nil
	if type(func) ~= "function" or type(metadata) ~= "table" or molt_sequence_len(metadata) ~= 14 then
		error({__type="TypeError", __msg="invalid packed function metadata"})
	end
	local execution_kind = rawget(metadata, 12)
	local freevars, cellvars = rawget(metadata, 13), rawget(metadata, 14)
	local function validate_names(names: any): nil
		if type(names) ~= "table" or rawget(names, molt_sequence_kind_key) ~= "tuple" then
			error({__type="TypeError", __msg="code lexical names must be tuples"})
		end
		for index = 1, molt_sequence_len(names) do
			if type(rawget(names, index)) ~= "string" then
				error({__type="TypeError", __msg="code lexical names must be strings"})
			end
		end
		return nil
	end
	validate_names(freevars)
	validate_names(cellvars)
	if type(execution_kind) ~= "number" or execution_kind < 0 or execution_kind > 3 or execution_kind ~= math.floor(execution_kind) then
		error({__type="TypeError", __msg="invalid function execution kind"})
	end
	if type(_code) == "table" then
		local previous = rawget(_code, "__molt_execution_kind")
		if previous ~= nil and previous ~= execution_kind then
			error({__type="SystemError", __msg="code execution kind cannot change"})
		end
		if previous ~= nil then
			for name, names in {co_freevars=freevars, co_cellvars=cellvars} do
				local published = rawget(_code, name)
				if type(published) ~= "table" or molt_sequence_len(published) ~= molt_sequence_len(names) then
					error({__type="SystemError", __msg="code lexical names cannot change"})
				end
				for index = 1, molt_sequence_len(names) do
					if rawget(published, index) ~= rawget(names, index) then
						error({__type="SystemError", __msg="code lexical names cannot change"})
					end
				end
			end
		end
		rawset(_code, "co_freevars", freevars)
		rawset(_code, "co_cellvars", cellvars)
		rawset(_code, "__molt_execution_kind", execution_kind)
	elseif execution_kind ~= 0 then
		error({__type="SystemError", __msg="task callable requires code metadata"})
	end
	molt_func_attr_set(func, "__name__", rawget(metadata, 1))
	molt_func_attr_set(func, "__qualname__", rawget(metadata, 2))
	molt_func_attr_set(func, "__module__", rawget(metadata, 3))
	molt_func_attr_set(func, "__defaults__", rawget(metadata, 9))
	molt_func_attr_set(func, "__kwdefaults__", rawget(metadata, 10))
	molt_func_attr_set(func, "__doc__", rawget(metadata, 11))
	molt_func_attr_set(func, "__code__", _code)
	molt_frame_function_capture(func, rawget(metadata, 3))
	molt_func_attr_set(func, "__molt_bind_kind__", bind_kind)
	local previous_metadata = molt_function_metadata[func]
	molt_function_metadata[func] = {
		arg_names = rawget(metadata, 4),
		posonly = rawget(metadata, 5) or 0,
		kwonly = rawget(metadata, 6) or molt_pack_tuple(),
		vararg = rawget(metadata, 7),
		varkw = rawget(metadata, 8),
		defaults = rawget(metadata, 9),
		kwdefaults = rawget(metadata, 10),
		is_builtin = previous_metadata ~= nil and previous_metadata.is_builtin == true,
	}
	return nil
end

local function molt_function_attr_set(func: any, name: any, value: any): nil
	local metadata = molt_function_metadata[func]
	if metadata ~= nil then
		if name == "__defaults__" then
			if value ~= nil and (type(value) ~= "table" or rawget(value, molt_sequence_kind_key) ~= "tuple") then
				error({__type="TypeError", __msg="__defaults__ must be set to a tuple object"})
			end
			metadata.defaults = value
		elseif name == "__kwdefaults__" then
			if value ~= nil and not molt_dict_is_ordered(value) then
				error({__type="TypeError", __msg="__kwdefaults__ must be set to a dict object"})
			end
			metadata.kwdefaults = value
		end
	end
	return molt_func_attr_set(func, name, value)
end

local function molt_function_attr_del(func: any, name: any): nil
	local metadata = molt_function_metadata[func]
	if metadata ~= nil then
		if name == "__defaults__" then metadata.defaults = nil
		elseif name == "__kwdefaults__" then metadata.kwdefaults = nil end
	end
	return molt_func_attr_del(func, name)
end

local function molt_function_set_defaults(func: any, defaults: any, kwdefaults: any): nil
	local metadata = molt_function_metadata[func]
	if metadata == nil then error({__type="TypeError", __msg="expected function metadata"}) end
	molt_function_attr_set(func, "__defaults__", defaults)
	molt_function_attr_set(func, "__kwdefaults__", kwdefaults)
	return nil
end
"#;
