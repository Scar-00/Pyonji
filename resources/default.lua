---@meta

---@class SshSession
---@field name string
---@field user_name ?string
---@field ip string

---@alias Value<T> T | fun(): T

---@alias BindFn
---| fun(self: Pyonji, binding: string, action: function)

---@alias RenameFn
---| fun(self: Pyonji, session: integer, name: string): boolean
---| fun(self: Pyonji, name: string): boolean

---@class Config
---@field font_family ?Value<string>
---@field font_size ?Value<number>
---@field line_height ?Value<number>
---@field status_height ?Value<number> status bar height as a multiple of line_height (default 1.0)
---@field fullscreen ?Value<boolean>
---@field default_cwd ?Value<string>
---@field ssh_sessions ?SshSession[]

---@class Pyonji
---@field current_tab integer
---@field font_size number
---@field line_height number
---@field font_family ?string
---@field rows integer
---@field cols integer
---@field active_session ?integer session id of the focused pane
---@field tab_count integer number of non-empty tabs
---@field detached_sessions integer[] ids of detached (hidden) sessions
---@field ssh_sessions SshSession[]
---@field sessions table<integer, table<integer, integer>>
---@field bind BindFn
---@field register fun(self: Pyonji, name: string, action: function)
---@field config fun(self: Pyonji, config: Config)
---@field open_palette fun(self: Pyonji?) command palette: filter, pick, run
---@field open_sessions fun(self: Pyonji?)
---@field open_detached fun(self: Pyonji?) list hidden sessions to attach again
---@field open_releases fun(self: Pyonji?)
---@field open_opener fun(self: Pyonji?)
---@field open_lua fun() opens the `>` lua prompt
---@field open_rename fun(self: Pyonji?)
---@field rename RenameFn
---@field detach fun(self: Pyonji): boolean
---@field attach fun(self: Pyonji, session: integer, tab: integer?): boolean
---@field close fun(self: Pyonji, session: integer?): boolean
---@field move_to fun(self: Pyonji, tab: integer): boolean
---@field split fun(self: Pyonji, direction: string): integer?
---@field create_session fun(self: Pyonji, dir: string?, tab: integer?, direction: string?, parent: integer?): integer
---@field switch_tab fun(self: Pyonji?, tab: integer): boolean
---@field next_tab fun(self: Pyonji): integer
---@field prev_tab fun(self: Pyonji): integer
---@field focus_next_pane fun(self: Pyonji): integer?
---@field write fun(self: Pyonji, session: integer, text: string): boolean
---@field reload_config fun(self: Pyonji)
---@field toggle_fullscreen fun(self: Pyonji): boolean
---@field toggle_decorations fun(self: Pyonji)
---@field toggle_status_bar fun(self: Pyonji)
---@field quit fun(self: Pyonji)

---@type Pyonji
py = py or {};

py:config({
    font_family = "Iosevka",
    font_size = 24,
    line_height = 1.1,
    fullscreen = false,
    default_cwd = nil,
    ssh_sessions = {
        -- { name = "server", user_name = "root", ip = "192.168.1.100" }
    },
});

-- Open Commands with this shortcut. Remap it here.
py:bind('<ctrl-shift>p', py.open_palette());

--[[
-- Methods are callable two ways:
--  * py:method(...) runs the action immediately and returns a real value
--    (e.g. py:create_session() returns the new session id)
--  * py.method(...) returns a callback instead, which can be used with py:bind
py:bind({ "ctrl", "shift" }, "F", py.open_palette());
py:bind({ "ctrl", "shift" }, "S", py.open_sessions());

-- Commands, copy, and paste have default shortcuts. Add a sequence if preferred:
py:bind('<ctrl-b> p', py.open_palette());

-- py:register adds a command to the palette. The
-- argument names are read off the function, so they show up as <placeholders>:
py:register("open", function (...)
    print(...);
end);
--]]
