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

---@type Config
return {
    font_size = 24.0,
    line_height = 28.0 / 24.0,
    fullscreen = true,
    ssh_sessions = {
        {
            name = "ive",
            user_name = "ive",
            ip = "192.168.178.20",
        }
    }
};
