---@meta

require('lua.keybind');

---@class SshSession
---@field name string
---@field user_name ?string
---@field ip string

---@generic T
---@alias Value T | fun(): T

---@overload fun(self: Pyonji, mods: Modifier[], key: Key, action: function)
---@alias BindFn fun(self: Pyonji, binding: string, action: function)

---@class Config
---@field font_family ?Value<string>
---@field font_size ?Value<number>
---@field line_height ?Value<number>
---@field fullscreen ?Value<boolean>
---@field default_cwd ?Value<string>
---@field ssh_sessions ?SshSession[],

---@class Pyonji
---@field current_tab integer
---@field font_size number
---@field line_height number
---@field font_family ?string
---@field rows integer
---@field cols integer
---@field bind fun(self: Pyonji, mods: Modifier[], key: Key, action: function)
---@field register fun(self: Pyonji, name: string, action: function)
---@field config fun(self: Pyonji, config: Config)
---@field open_palette fun(self: Pyonji?): function
---@field open_sessions fun(self: Pyonji?): function
py = {};

---@param self ?Pyonji
---@param dir ?string
---@param tab ?integer
---@param direction ?string
---@param parent ?integer
---@return function
function py.create_session(self, dir, tab, direction, parent) end

---@param dir ?string
---@param tab ?integer
---@param direction ?string
---@param parent ?integer
---@return function
function py.create_session(dir, tab, direction, parent) end

--[[
---@param self Pyonji
---@param mods Modifier[]
---@param key Key
---@param action function
---@overload fun(self: Pyonji, mods: Modifier[], key: Key, action: function)
function py:bind(self, mods, key, action) end
--]]

--[[
local bind = KeyBind('A', { "ctrl" });
py:bind(bind, py.open_palette());

py:register("test", function (...)
    print(...);
end);
--]]
