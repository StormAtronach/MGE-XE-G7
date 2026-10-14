--[[
    Mod: MGE XE In-game Config
    Author: Hrnchamd
    Version: 1.0
]]--

local gui = require("MGE XE Options.gui")
local shaderparams = require("MGE XE Options.shaderparams")
local hotreload = require("MGE XE Options.hotreload")
local i18n = mwse.loadTranslations("MGE XE Options")

-- MCM

local modConfig = {}
function modConfig.onCreate(parent)
	local container = parent:createThinBorder{}
	container.flowDirection = "top_to_bottom"
	container.layoutHeightFraction = 1.0
	container.layoutWidthFraction = 1.0
	container.paddingAllSides = 12

	local header = container:createLabel{text = "MGE XE"}
	header.color = tes3ui.getPalette("header_color")
	header.borderAllSides = 12

	local button = container:createButton{text = i18n("OpenSettings")}
	button.paddingAllSides = 10
	button.paddingBottom = nil

	-- Opens a window of this mod in the place of the MCM.
	local function open(run)
		-- Check if MGE API is available.
		if mge.render then
			-- Click MCM OK button.
			local ok = parent:getTopLevelMenu():findChild("MWSE:ModConfigMenu_Close")
			if ok then
				ok:triggerEvent("mouseClick")
			end
			-- Hide MenuOptions if in-game.
			local menuOptions = tes3ui.findMenu("MenuOptions")
			if menuOptions and not tes3.onMainMenu() then
				menuOptions.visible = false
			end
			-- Open the window.
			local mgeMenu = run()
			-- Show MenuOptions again when window is closed.
			if mgeMenu then
				mgeMenu:register("destroy", function()
					local menuOptions = tes3ui.findMenu("MenuOptions")
					if menuOptions then
						menuOptions.visible = true
					end
				end)
			end
		else
			tes3.messageBox{message = i18n("PleaseUpgrade")}
		end
	end

	button:register("mouseClick", function(e) open(gui.run) end)
end

event.register("modConfigReady", function ()
	mwse.registerModConfig("MGE XE", modConfig)
end)

-- Video options

local function addGammaResetButton(e)
	if not e.newlyCreated then return end

	local gamma = e.element:findChild("MenuVideo_GamaSlider")
	if gamma then
		local r = gamma.parent:createButton{ id = "MGE-XE:ResetGamma", text = i18n("Reset") }
		r.absolutePosAlignX = 1
		r.absolutePosAlignY = 0.38

		r:register(tes3.uiEvent.mouseClick, function(e)
			gamma.widget.current = 50
			gamma:triggerEvent("PartScrollBar_changed")
			gamma:getTopLevelMenu():updateLayout()
		end)

		gamma:getTopLevelMenu():updateLayout()
	end
end
event.register(tes3.event.uiActivated, addGammaResetButton, { filter = "MenuVideo" })

-- Init

local function onInitialized(mod)
	gui.i18n = i18n
	shaderparams.i18n = i18n
	hotreload.i18n = i18n
	-- A shader is loaded again when its file changes. The game stops each timer when it
	-- loads a save: the watch starts again after each load.
	hotreload.start()
	event.register("loaded", hotreload.start)
end
event.register("initialized", onInitialized)
