local molt_json_dumps
do
	local function escape_json(value: string): string
		return string.gsub(value, '[%z\1-\31\\"]', function(character)
			if character == '\\' or character == '"' then return '\\' .. character end
			if character == '\b' then return '\\b' end
			if character == '\f' then return '\\f' end
			if character == '\n' then return '\\n' end
			if character == '\r' then return '\\r' end
			if character == '\t' then return '\\t' end
			return string.format('\\u%04x', string.byte(character))
		end)
	end

	local function serialize(value: any, active: {[any]: boolean}): string
		local kind = type(value)
		if kind == 'nil' then return 'null' end
		if kind == 'boolean' then return if value then 'true' else 'false' end
		if kind == 'number' then
			if value ~= value or value == math.huge or value == -math.huge then
				error({__type='ValueError', __msg='Out of range float values are not JSON compliant'})
			end
			if value == math_floor(value) and math.abs(value) < 1e15 then return string.format('%d', value) end
			return tostring(value)
		end
		if kind == 'string' then return '"' .. escape_json(value) .. '"' end
		if kind ~= 'table' then
			error({__type='TypeError', __msg='Object of type ' .. kind .. ' is not JSON serializable'})
		end
		if active[value] then error({__type='ValueError', __msg='Circular reference detected'}) end
		active[value] = true
		local result
		if molt_dict_is_ordered(value) then
			local parts = table.create(molt_dict_len(value))
			local count = 0
			molt_dict_scan(value, function(key, item)
				local key_kind = type(key)
				local text
				if key_kind == 'string' then text = key
				elseif key == nil then text = 'null'
				elseif key_kind == 'boolean' then text = if key then 'true' else 'false'
				elseif key_kind == 'number' then text = tostring(key)
				else error({__type='TypeError', __msg='keys must be str, int, float, bool or None'}) end
				count += 1
				parts[count] = '"' .. escape_json(text) .. '": ' .. serialize(item, active)
			end)
			result = '{' .. table.concat(parts, ', ') .. '}'
		else
			local sequence_kind = rawget(value, molt_sequence_kind_key)
			if sequence_kind ~= 'list' and sequence_kind ~= 'tuple' then
				error({__type='TypeError', __msg='foreign Luau table is not JSON serializable'})
			end
			local count = molt_sequence_len(value)
			local parts = table.create(count)
			for index = 1, count do parts[index] = serialize(rawget(value, index), active) end
			result = '[' .. table.concat(parts, ', ') .. ']'
		end
		active[value] = nil
		return result
	end

	molt_json_dumps = function(value: any): string
		return serialize(value, {})
	end
end

local json = {dumps = molt_json_dumps}
