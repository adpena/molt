/// Luau's internal execution-frame authority.
///
/// Each coroutine owns a reusable context containing only code/source-location
/// and unwind custody. This intentionally does not expose Python frame objects,
/// tracing hooks, or `__traceback__.tb_frame`; those require a separate exact
/// introspection capability that Luau rejects before source generation.
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
	return {depth=0, codes={}, lines={}, lastis={}, cols={}, end_cols={}, globals={}}
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

local function molt_frame_enter(code: any): (any, number, any, any)
	if type(code) ~= "table" or code.co_name == nil then
		molt_frame_invariant("trace_enter_slot references an unbound code object")
	end
	local context, owner = molt_frame_context()
	context.depth += 1
	local index = context.depth
	context.codes[index] = code
	context.lines[index] = if type(code.co_firstlineno) == "number" then code.co_firstlineno else 0
	context.lastis[index] = 0
	context.cols[index] = -1
	context.end_cols[index] = -1
	context.globals[index] = code.co_globals or (if index > 1 then context.globals[index - 1] else nil)
	return context, index, code, owner
end

local function molt_frame_bind_code(code: any): nil
	if type(code) ~= "table" then
		molt_frame_invariant("code_slot_set requires a code object")
	end
	local context = molt_frame_context()
	if context.depth > 0 then
		code.co_globals = context.globals[context.depth]
	end
	return nil
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

local function molt_frame_locals_set(context: any, locals_value: any): nil
	local index = context.depth
	if index < 1 then
		molt_frame_invariant("frame_locals_set requires an active Python execution frame")
	end
	if type(locals_value) ~= "table" then
		molt_frame_invariant("frame_locals_set requires a locals dictionary")
	end
	local code = context.codes[index]
	if code.co_name == "<module>" then
		context.globals[index] = locals_value
		code.co_globals = locals_value
	end
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
	local thread: any = coroutine.create(function(...)
		local context, owner = molt_frame_context()
		execution_context = context
		execution_owner = owner
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
		execution_context = nil
		execution_owner = nil
		thread = nil
		return close_error
	end
	local function resume(...)
		if finalized or thread == nil then
			error({__type="RuntimeError", __msg="cannot resume finalized coroutine"}, 0)
		end
		local results = table.pack(coroutine.resume(thread, ...))
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
