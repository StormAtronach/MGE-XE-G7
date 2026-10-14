--[=[
	The window "parameters" of a shader: an editor for each value of the shader that a
	person can change while the game runs. The shader list of the MGE XE options opens it.

	Two kinds of shader have parameters:

	- A post shader (an entry of the shader list). Its variables come from MGE XE
	  (shader.variables), and a change goes to the shader at once (shader.<name> = value).
	  The variables that MGE XE sets itself (the time, the fog, the matrices) are left out.
	  A file <name>.toml beside the shader can describe its parameters: see below.

	- A part of the wgpu renderer itself, for example its water. Its parameters are in
	  tunables.toml beside this file, which the renderer has built in. A change goes to
	  the renderer as a command of the nirender plugin: nirender.tune("set <name> <value>").

	The file of the parameters, for both kinds: a list of entries

		[[parameter]]
		name = "intensity"      # the name of the variable
		label = "Intensity"     # the text of its row; the name without it
		about = "..."           # one line that says what it is
		start = 0.5             # the value of the button "Reset"; the first value without it
		least = 0.0             # with least, most and step, the row has a slider
		most = 2.0
		step = 0.05

	An entry of a vector is for each of its parts. Without the file, or for a variable that
	the file does not name, the row has the buttons - and + in place of a slider.

	The values are not saved.
]=]--

local this = {}

local lfs = require("lfs")
local hotreload = require("MGE XE Options.hotreload")
local nirender = include("nirender")

local builtinPath = "Data Files\\MWSE\\mods\\MGE XE Options\\tunables.toml"
local shaderFolder = "Data Files\\shaders\\XEshaders\\"

-- The variables that MGE XE sets in each frame: no row for them.
local driven = {
	lastshader = true, lastpass = true, depthframe = true, watertexture = true,
	eyevec = true, eyepos = true, sunvec = true, suncol = true, sunamb = true, sunpos = true,
	sunvis = true, HDR = true, mview = true, mproj = true, fogcol = true, fognearcol = true,
	fogstart = true, fogrange = true, fognearstart = true, fognearrange = true, rcpres = true,
	fov = true, time = true, waterlevel = true, isInterior = true, isUnderwater = true,
	pointlightcount = true, pointlightpos = true, pointlightcol = true, mgeflags = true,
}

-- The entries of a file of parameters, in their order; none for a file that is not there.
local function readList(path)
	if not lfs.attributes(path, "mode") then return {} end
	local ok, result = pcall(toml.loadFile, path)
	local list = {}
	if ok and type(result) == "table" and type(result.parameter) == "table" then
		for _, p in ipairs(result.parameter) do
			if type(p.name) == "string" then table.insert(list, p) end
		end
	else
		mwse.log("[MGE XE Options] %s was not read: %s", path, tostring(result))
	end
	return list
end

-- The parameters of the renderer itself: the entries of tunables.toml, read one time, and
-- the values that this session set, by name.
local builtin
local builtinValues = {}

local function builtinList()
	if not builtin then
		builtin = {}
		for _, p in ipairs(readList(builtinPath)) do
			-- An entry that is not complete has no row.
			if type(p.start) == "number" and type(p.least) == "number" and type(p.most) == "number"
				and type(p.step) == "number" and p.step > 0 and p.most > p.least then
				table.insert(builtin, p)
			end
		end
	end
	return builtin
end

-- The groups of the parameters of the renderer: the part of a name before its first dot
-- ("water" of "water.blur.angle"). None when the wgpu renderer does not draw the game.
function this.builtinGroups()
	local groups = {}
	if not (nirender and nirender.tune and hotreload.wgpu()) then return groups end
	local seen = {}
	for _, p in ipairs(builtinList()) do
		local group = p.name:match("^([^.]+)%.") or p.name
		if not seen[group] then
			seen[group] = true
			table.insert(groups, group)
		end
	end
	return groups
end

-- The first values of the variables of each shader, for the button "Reset" of a row
-- without a start value in a file.
local firstValues = {}

-- A row of the window: `label`, `about`, `get()`, `set(value)`, `start`, and for a number
-- `least`, `most`, `step`, `integer`; `toggle` for a switch; `note` for a row with a text
-- only.
local function entriesOfGroup(group)
	local entries = {}
	for _, p in ipairs(builtinList()) do
		if (p.name:match("^([^.]+)%.") or p.name) == group then
			table.insert(entries, {
				label = p.label or p.name, about = p.about, start = p.start,
				least = p.least, most = p.most, step = p.step,
				get = function() return builtinValues[p.name] or p.start end,
				set = function(value)
					builtinValues[p.name] = value
					nirender.tune(string.format("set %s %.6f", p.name, value))
				end,
			})
		end
	end
	return entries
end

local parts = { "x", "y", "z", "w" }

local function entriesOfShader(shader)
	local described = readList(shaderFolder .. shader.name .. ".toml")
	local byName = {}
	for index, p in ipairs(described) do
		byName[p.name] = p
		p.order = index
	end
	-- The variables that the file names come first, in its order; then the others by name.
	local names = {}
	for name in pairs(shader.variables) do
		if not driven[name] then table.insert(names, name) end
	end
	table.sort(names, function(a, b)
		local pa, pb = byName[a], byName[b]
		if pa and pb then return pa.order < pb.order end
		if pa or pb then return pa ~= nil end
		return a < b
	end)

	firstValues[shader.name] = firstValues[shader.name] or {}
	local first = firstValues[shader.name]
	local entries = {}
	local function add(key, entry, p)
		if first[key] == nil then first[key] = entry.get() end
		entry.label = entry.label or key
		entry.about = entry.about or p.about
		entry.start = type(p.start) == entry.kind and p.start or first[key]
		if entry.kind == "number" and type(p.least) == "number" and type(p.most) == "number"
			and type(p.step) == "number" and p.step > 0 and p.most > p.least then
			entry.least, entry.most, entry.step = p.least, p.most, p.step
		elseif entry.kind == "number" and type(p.step) == "number" and p.step > 0 then
			entry.step = p.step
		end
		table.insert(entries, entry)
	end
	for _, name in ipairs(names) do
		local kind = shader.variables[name]
		local p = byName[name] or {}
		if kind == "f" or kind == "i" then
			add(name, {
				kind = "number", label = p.label, integer = kind == "i",
				get = function() return shader[name] end,
				set = function(value) shader[name] = value end,
			}, p)
		elseif kind == "b" then
			add(name, {
				kind = "boolean", label = p.label, toggle = true,
				get = function() return shader[name] end,
				set = function(value) shader[name] = value end,
			}, p)
		elseif kind == "2" or kind == "3" or kind == "4" then
			local count = tonumber(kind)
			for part = 1, count do
				add(name .. "." .. parts[part], {
					kind = "number", label = (p.label or name) .. " " .. parts[part],
					about = part == 1 and p.about or "",
					get = function() return shader[name][parts[part]] end,
					set = function(value)
						local vector = shader[name]
						local values = {}
						for i = 1, count do values[i] = vector[parts[i]] end
						values[part] = value
						shader[name] = values
					end,
				}, p)
			end
		else
			table.insert(entries, { label = p.label or name, about = p.about, note = this.i18n("NotEditable") })
		end
	end
	return entries
end

-- A value as a text, with the decimals of a step.
local function shown(value, step)
	local decimals = math.max(0, math.min(6, math.ceil(-math.log10(step) - 1e-6)))
	return string.format("%." .. decimals .. "f", value)
end

-- The step of a row without one: a tenth of the size of the first value.
local function guessedStep(entry)
	if entry.integer then return 1 end
	local size = math.abs(entry.start or 0)
	if size < 1e-6 then return 0.01 end
	return 10 ^ math.floor(math.log10(size)) / 10
end

local function createThinButton(parent, text, width)
	local b = parent:createButton{text = text}
	b.contentPath = "menu_thin_border.nif"
	b.borderAllSides = 0
	b.paddingAllSides = 1
	b.minWidth = width
	return b
end

-- Builds the row of an entry. Returns a function that puts the entry back to its start value.
local function createRow(parent, entry, menu)
	local i18n = this.i18n
	local block = parent:createThinBorder{}
	block.flowDirection = "top_to_bottom"
	block.widthProportional = 1.0
	block.autoHeight = true
	block.paddingAllSides = 8
	block.borderBottom = 6

	local head = block:createBlock{}
	head.widthProportional = 1.0
	head.autoHeight = true
	head.childAlignY = 0.5
	local label = head:createLabel{text = entry.label}
	label.color = tes3ui.getPalette("header_color")
	local right = head:createBlock{}
	right.widthProportional = 1.0
	right.autoHeight = true
	right.childAlignX = 1.0
	right.childAlignY = 0.5

	if entry.note then
		right:createLabel{text = entry.note}
		if entry.about and entry.about ~= "" then
			local about = block:createLabel{text = entry.about}
			about.wrapText = true
			about.widthProportional = 1.0
		end
		return nil
	end

	local step = entry.step or guessedStep(entry)
	local function text(value)
		if entry.toggle then return i18n(value and "On" or "Off") end
		return shown(value, step)
	end

	local valueLabel = right:createLabel{text = text(entry.get())}
	valueLabel.borderRight = 10
	local slider
	local function apply(value)
		if not entry.toggle then
			if entry.integer then value = math.floor(value + 0.5) end
			if entry.least then value = math.max(entry.least, math.min(entry.most, value)) end
		end
		entry.set(value)
		valueLabel.text = text(value)
		menu:updateLayout()
	end

	if entry.toggle then
		local switch = createThinButton(right, i18n("Switch"), 60)
		switch:register("mouseClick", function(e) apply(not entry.get()) end)
	elseif not entry.least then
		local decr = createThinButton(right, "-", 25)
		local incr = createThinButton(right, "+", 25)
		decr:register("mouseClick", function(e) apply(entry.get() - step) end)
		incr:register("mouseClick", function(e) apply(entry.get() + step) end)
	end
	local reset = createThinButton(right, i18n("Reset"), 60)
	reset.borderLeft = 6

	if entry.about and entry.about ~= "" then
		local about = block:createLabel{text = entry.about}
		about.wrapText = true
		about.widthProportional = 1.0
	end

	local function stepsOf(value)
		return math.floor((value - entry.least) / entry.step + 0.5)
	end
	if entry.least then
		local steps = stepsOf(entry.most)
		slider = block:createSlider{
			id = "MGE-XE-Params:" .. entry.label,
			current = math.max(0, math.min(steps, stepsOf(entry.get()))),
			max = steps,
			step = 1,
			jump = math.max(1, math.floor(steps / 10)),
		}
		slider.widthProportional = 1.0
		slider.borderTop = 6
		slider:register("PartScrollBar_changed", function(e)
			apply(entry.least + slider.widget.current * entry.step)
		end)
	end

	local function toStart()
		if slider then
			slider.widget.current = math.max(0, math.min(slider.widget.max, stepsOf(entry.start)))
			slider:triggerEvent("PartScrollBar_changed")
		else
			apply(entry.start)
		end
	end
	reset:register("mouseClick", function(e) toStart() end)
	return toStart
end

-- Opens the window for a post shader (`target.shader`) or for a group of the parameters
-- of the renderer (`target.group`).
function this.open(target)
	local i18n = this.i18n

	local menu = tes3ui.findMenu("MenuMGE-XE-Params")
	if menu then menu:destroy() end

	local shader = target.shader
	local title = shader and shader.name or (target.group .. " " .. i18n("BuiltinSuffix"))
	local entries = shader and entriesOfShader(shader) or entriesOfGroup(target.group)

	-- The place and the size are not kept from the session before: a saved state can hide
	-- the window (the game saves a menu that was made out of menu mode as not visible).
	menu = tes3ui.createMenu{id = "MenuMGE-XE-Params", dragFrame = true}
	menu.text = i18n("Parameters") .. ": " .. title
	menu.width = 540
	menu.height = math.min(760, 230 + 96 * math.max(1, #entries))
	menu.minWidth = 440
	menu.minHeight = 240
	-- At the left of the screen, so that the picture stays in view.
	menu.positionX = -0.5 * menu.maxWidth + 20
	menu.positionY = 0.5 * menu.height + 20
	menu.visible = true

	local note = menu:createLabel{text = i18n("ParamsNote")}
	note.wrapText = true
	note.widthProportional = 1.0
	note.borderAllSides = 8

	-- The result of the last load of the shader, when one is known.
	local status = menu:createLabel{id = "MGE-XE-Params:Status", text = ""}
	status.wrapText = true
	status.widthProportional = 1.0
	status.borderLeft = 8
	status.borderRight = 8
	status.borderBottom = 6
	local function showStatus()
		local last = shader and hotreload.last[shader.name]
		if not last then
			status.visible = false
		else
			status.visible = true
			status.text = last.ok and i18n("ShaderReloaded") or (i18n("ShaderError") .. " " .. i18n("ShaderSeeMessage"))
			status.color = tes3ui.getPalette(last.ok and "positive_color" or "negative_color")
		end
	end
	showStatus()

	local pane = menu:createVerticalScrollPane{id = "MGE-XE-Params:Pane"}
	pane.widthProportional = 1.0
	pane.heightProportional = 1.0
	pane.borderLeft = 8
	pane.borderRight = 8

	local content = pane:getContentElement()
	local resets = {}
	if #entries == 0 then
		content:createLabel{text = i18n("NoParameters")}
	end
	for _, entry in ipairs(entries) do
		local toStart = createRow(content, entry, menu)
		if toStart then table.insert(resets, toStart) end
	end

	-- Bottom bar: load the shader again, all back to the start values, close.
	local bar = menu:createBlock{}
	bar.widthProportional = 1.0
	bar.autoHeight = true
	bar.childAlignY = 0.5
	bar.borderAllSides = 8

	if shader then
		local reload = bar:createButton{id = "MGE-XE-Params:Reload", text = i18n("Reload")}
		reload:register("mouseClick", function(e) hotreload.reload(shader) end)
	end
	local resetAll = bar:createButton{id = "MGE-XE-Params:ResetAll", text = i18n("ResetAll")}
	resetAll:register("mouseClick", function(e)
		for _, toStart in ipairs(resets) do toStart() end
	end)
	local right = bar:createBlock{}
	right.widthProportional = 1.0
	right.autoHeight = true
	right.childAlignX = 1.0
	local close = right:createButton{text = i18n("Close")}
	close:register("mouseClick", function(e) menu:destroy() end)

	-- A load of the shader while the window is open: its result shows, and the window is
	-- made again when the shader compiles, because its variables can be other ones.
	hotreload.onResult = function(name)
		local open = tes3ui.findMenu("MenuMGE-XE-Params")
		if not open or not shader or name ~= shader.name then return end
		local last = hotreload.last[name]
		if last and last.ok then
			this.open(target)
		else
			showStatus()
			open:updateLayout()
		end
	end
	menu:register("destroy", function() hotreload.onResult = nil end)

	-- Run layout twice to iron out use of proportional sizing on multiple branches.
	menu:updateLayout()
	menu:updateLayout()
	return menu
end

return this
