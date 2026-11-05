---@meta

---@class SshSession
---@field name string The name the palette lists it under.
---@field user_name ?string Login name (your local one if left out).
---@field ip string Host name or address.

---@alias Value<T> T | fun(): T

---@alias BindFn
---| fun(self: Pyonji, binding: string, action: function)

---@class Config
---@field font_family ?Value<string> An installed font family for the grid.
---@field font_size ?Value<number> Grid font size in points.
---@field line_height ?Value<number> Line height as a multiple of font_size.
---@field status_height ?Value<number> Status bar height as a multiple of line_height (default 1.0).
---@field fullscreen ?Value<boolean> Start in fullscreen.
---@field default_cwd ?Value<string> Directory new sessions start in.
---@field ssh_sessions ?SshSession[] SSH targets to offer in the palette.

---@class Pyonji
---@field current_tab integer Index of the tab in front (0-based).
---@field font_size number Grid font size in points.
---@field line_height number Grid line height as a multiple of font_size.
---@field font_family ?string Grid font family, if one is set.
---@field rows integer Rows in the grid.
---@field cols integer Columns in the grid.
---@field active_session ?integer Session id of the focused pane.
---@field tab_count integer Number of non-empty tabs.
---@field detached_sessions integer[] Ids of detached (hidden) sessions.
---@field ssh_sessions SshSession[] SSH targets from the config.
---@field sessions table<integer, table<integer, integer>> Session ids of each tab's panes, by tab.
---@field bind BindFn Run `action` when the key sequence `binding` is pressed.
---@field register fun(self: Pyonji, name: string, action: function) Add `action` to the command palette as `name`.
---@field config fun(self: Pyonji, config: Config) Apply settings: font, size, cwd, SSH targets.
---@field open_palette fun(self: Pyonji?) Command palette: filter, pick, run.
---@field open_sessions fun(self: Pyonji?) List running and detached sessions to switch to.
---@field open_detached fun(self: Pyonji?) List hidden sessions to attach again.
---@field open_releases fun(self: Pyonji?) Check GitHub for a newer Pyonji build.
---@field open_opener fun(self: Pyonji?) Pick a file or directory to open in a new session.
---@field open_lua fun() Open the lua prompt in the status bar.
---@field open_rename fun(self: Pyonji?) Rename the focused session in the status bar.
---@field rename (fun(self: Pyonji, session: integer, name: string): boolean) | (fun(self: Pyonji, name: string): boolean) Rename a session (the focused one if none is given).
---@field detach fun(self: Pyonji): boolean Hide the focused session without closing it.
---@field attach fun(self: Pyonji, session: integer, tab: integer?): boolean Put a detached session back in a tab (the current one by default).
---@field close fun(self: Pyonji, session: integer?): boolean Close a session (the focused one if none is given).
---@field move_to fun(self: Pyonji, tab: integer): boolean Move the focused session into another tab.
---@field split fun(self: Pyonji, direction: string): integer? Split the focused pane ("h" or "v"); returns the new session id.
---@field create_session fun(self: Pyonji, dir: string?, tab: integer?, direction: string?, parent: integer?): integer Start a session, optionally in `dir`, a tab, or split off `parent`.
---@field switch_tab fun(self: Pyonji?, tab: integer): boolean Go to a tab, opening a session if that slot is empty.
---@field next_tab fun(self: Pyonji): integer Go to the next tab; returns its index.
---@field prev_tab fun(self: Pyonji): integer Go to the previous tab; returns its index.
---@field focus_next_pane fun(self: Pyonji): integer? Move focus to the next pane; returns its session id.
---@field write fun(self: Pyonji, session: integer, text: string): boolean Send `text` to a session as if typed.
---@field reload_config fun(self: Pyonji) Read init.lua from disk again.
---@field toggle_fullscreen fun(self: Pyonji): boolean Enter or leave fullscreen; returns the new state.
---@field toggle_decorations fun(self: Pyonji) Show or hide the window's title bar and borders.
---@field toggle_status_bar fun(self: Pyonji) Show or hide the status bar.
---@field quit fun(self: Pyonji) Close every session and exit.

---@type Pyonji
py = py or {};

py:config({
    -- An installed font family. Left out, the platform's monospace is used.
    -- font_family = "Iosevka",
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
