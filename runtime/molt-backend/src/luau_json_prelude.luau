local molt_json_dumps
do
	local function serialize(val: any, depth: number): string
		if depth > 50 then return '"[max depth]"' end
		local t = type(val)
		if t == "nil" then
			return "null"
		elseif t == "boolean" then
			return val and "true" or "false"
		elseif t == "number" then
			if val ~= val then return "null" end
			if val == math.huge then return "1e308" end
			if val == -math.huge then return "-1e308" end
			if val == math_floor(val) and math.abs(val) < 1e15 then
				return string.format("%d", val)
			end
			return tostring(val)
		elseif t == "string" then
			local escaped = val:gsub('[\\"]', function(c) return "\\" .. c end)
			escaped = escaped:gsub("\n", "\\n"):gsub("\r", "\\r"):gsub("\t", "\\t")
			return '"' .. escaped .. '"'
		elseif t == "table" then
			-- Detect array vs object: array if sequential integer keys from 1
			local is_array = true
			local n = #val
			if n == 0 then
				-- Check if it has any keys at all
				if next(val) ~= nil then is_array = false end
			else
				for k in pairs(val) do
					if type(k) ~= "number" or k < 1 or k > n or k ~= math_floor(k) then
						is_array = false
						break
					end
				end
			end
			local parts = {}
			if is_array then
				for i = 1, n do
					table.insert(parts, serialize(val[i], depth + 1))
				end
				return "[" .. table.concat(parts, ", ") .. "]"
			else
				for k, v in pairs(val) do
					local ks = type(k) == "string" and k or tostring(k)
					local escaped = ks:gsub('[\\"]', function(c) return "\\" .. c end)
					table.insert(parts, '"' .. escaped .. '": ' .. serialize(v, depth + 1))
				end
				return "{" .. table.concat(parts, ", ") .. "}"
			end
		end
		return '"' .. tostring(val) .. '"'
	end
	molt_json_dumps = function(val: any): string
		return serialize(val, 0)
	end
end

-- json module bridge
local json = {
	dumps = molt_json_dumps,
}
