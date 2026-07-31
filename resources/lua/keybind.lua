
---@alias Key
---| 'A',

---@alias Modifier
---| "ctrl"
---| "shift"
---| "alt"
---| "mod"

---create a valid keybind
---@param key Key
---@param mods table<integer, Modifier>,
function KeyBind(key, mods)
    local bind = "<";
    for i, mod in pairs(mods) do
        bind = bind .. mod;
        if i ~= #mods then
            bind = bind .. "+";
        end
    end
    bind = bind .. ">";
    bind = bind .. "-" .. key;
    return bind;
end
