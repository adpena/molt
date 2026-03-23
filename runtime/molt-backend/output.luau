--!native
--!strict
-- Molt -> Luau transpiled output
-- Runtime helpers

local molt_func_attrs: {[any]: {[string]: any}} = {}
local molt_module_cache: {[string]: any} = {
	math = nil,
	json = nil,
	time = nil,
	os = nil,
}


-- Entry point
if molt_main then
	molt_main()
end
