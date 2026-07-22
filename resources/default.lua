---@class SshSession
---@field name string
---@field user_name string
---@field ip string

---@generic T
---@alias Value T | fun(): T

---@class Config
---@field font_family ?Value<string>
---@field font_size ?Value<number>
---@field line_height ?Value<number>
---@field fullscreen ?Value<boolean>
---@field default_cwd ?Value<string>
---@field ssh_sessions ?SshSession[],
---@field open_palette ?string,

---@type Config
return {
    --font_family = "Iosevka",
    --font_size = 32.0,
};
