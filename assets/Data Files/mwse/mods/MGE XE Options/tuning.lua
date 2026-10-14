--[[
	The window "renderer parameters": a slider for each parameter of the wgpu renderer
	that a person can change while the game runs, to find a good value by eye.

	The list of the parameters is tunables.toml beside this file. The renderer has the same
	list built in. A change of a slider goes to the renderer as a command of the nirender
	plugin: nirender.tune("set <name> <value>").

	The values are not saved. A value that is good goes into tunables.toml as the new start
	value.
]]--

local this = {}

local nirender = include("nirender")
local listPath = "Data Files\\MWSE\\mods\\MGE XE Options\\tunables.toml"

-- The parameters of the file, read one time; and the values that this session set, by name.
local parameters
local values = {}

local function load()
	if not parameters then
		local ok, result = pcall(toml.loadFile, listPath)
		parameters = {}
		if ok and type(result) == "table" and type(result.parameter) == "table" then
			for _, p in ipairs(result.parameter) do
				-- An entry that is not complete has no slider.
				if type(p.name) == "string" and type(p.start) == "number" and type(p.least) == "number"
					and type(p.most) == "number" and type(p.step) == "number" and p.step > 0 and p.most > p.least then
					table.insert(parameters, p)
				end
			end
		else
			mwse.log("[MGE XE Options] %s was not read: %s", listPath, tostring(result))
		end
	end
	return parameters
end

-- The wgpu renderer draws the game, its plugin has the command, and the list is there.
function this.available()
	return nirender ~= nil and nirender.tune ~= nil and nirender.ownDevice ~= nil
		and nirender.ownDevice() == true and #load() > 0
end

-- A value as a text, with the decimals of the step of its parameter.
local function shown(p, value)
	local decimals = math.max(0, math.ceil(-math.log10(p.step) - 1e-6))
	return string.format("%." .. decimals .. "f", value)
end

local function stepsOf(p, value)
	return math.floor((value - p.least) / p.step + 0.5)
end

local function set(p, value)
	values[p.name] = value
	nirender.tune(string.format("set %s %.6f", p.name, value))
end

function this.run()
	local i18n = this.i18n

	-- Close the window if triggered a second time.
	local menu = tes3ui.findMenu("MenuMGE-XE-Tuning")
	if menu then
		menu:destroy()
		return
	end

	local list = load()
	-- The place and the size are not kept from the session before: the window is small,
	-- and a saved state can hide it (the game saves a menu that was made out of menu mode
	-- as not visible).
	menu = tes3ui.createMenu{id = "MenuMGE-XE-Tuning", dragFrame = true}
	menu.text = i18n("TuningTitle")
	menu.width = 520
	menu.height = 200 + 106 * #list
	menu.minWidth = 420
	menu.minHeight = 200

	-- At the left of the screen, so that the picture stays in view.
	menu.positionX = -0.5 * menu.maxWidth + 20
	menu.positionY = 0.5 * menu.height + 20
	menu.visible = true

	local note = menu:createLabel{text = i18n("TuningNote")}
	note.wrapText = true
	note.widthProportional = 1.0
	note.borderAllSides = 8

	local sliders = {}
	for _, p in ipairs(list) do
		local block = menu:createThinBorder{}
		block.flowDirection = "top_to_bottom"
		block.widthProportional = 1.0
		block.autoHeight = true
		block.paddingAllSides = 8
		block.borderLeft = 8
		block.borderRight = 8
		block.borderBottom = 6

		local head = block:createBlock{}
		head.widthProportional = 1.0
		head.autoHeight = true
		head.childAlignY = 0.5

		local label = head:createLabel{text = p.label or p.name}
		label.color = tes3ui.getPalette("header_color")
		local valueBlock = head:createBlock{}
		valueBlock.widthProportional = 1.0
		valueBlock.autoHeight = true
		valueBlock.childAlignX = 1.0
		local valueLabel = valueBlock:createLabel{text = shown(p, values[p.name] or p.start)}
		valueLabel.borderRight = 10
		local reset = head:createButton{text = i18n("Reset")}

		if p.about then
			local about = block:createLabel{text = p.about}
			about.wrapText = true
			about.widthProportional = 1.0
		end

		local steps = stepsOf(p, p.most)
		local slider = block:createSlider{
			id = "MGE-XE-Tuning:" .. p.name,
			current = stepsOf(p, values[p.name] or p.start),
			max = steps,
			step = 1,
			jump = math.max(1, math.floor(steps / 10)),
		}
		slider.widthProportional = 1.0
		slider.borderTop = 6

		slider:register("PartScrollBar_changed", function(e)
			local value = p.least + slider.widget.current * p.step
			set(p, value)
			valueLabel.text = shown(p, value)
			menu:updateLayout()
		end)
		reset:register("mouseClick", function(e)
			slider.widget.current = stepsOf(p, p.start)
			slider:triggerEvent("PartScrollBar_changed")
		end)
		table.insert(sliders, { parameter = p, slider = slider })
	end

	-- Bottom bar: all back to the start values, close.
	local bar = menu:createBlock{}
	bar.widthProportional = 1.0
	bar.heightProportional = 1.0
	bar.childAlignY = 1.0
	bar.borderAllSides = 8

	local resetAll = bar:createButton{id = "MGE-XE-Tuning:ResetAll", text = i18n("ResetAll")}
	resetAll:register("mouseClick", function(e)
		for _, entry in ipairs(sliders) do
			entry.slider.widget.current = stepsOf(entry.parameter, entry.parameter.start)
			entry.slider:triggerEvent("PartScrollBar_changed")
		end
	end)

	local right = bar:createBlock{}
	right.widthProportional = 1.0
	right.autoHeight = true
	right.childAlignX = 1.0
	local close = right:createButton{text = i18n("Close")}
	close:register("mouseClick", function(e) menu:destroy() end)

	-- Run layout twice to iron out use of proportional sizing on multiple branches.
	menu:updateLayout()
	menu:updateLayout()

	return menu
end

return this
