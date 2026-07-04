---@generic T
---@alias Value T | fun(): T

---@class Config
---@field font_family ?Value<string>
---@field font_size ?Value<number>
---@field line_height ?Value<number>

---@type Config
return {
    font_size = 24.0,
    line_height = 28.0 / 24.0,
};
