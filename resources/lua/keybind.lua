---@alias Key
---| 'A'
---| 'B'
---| 'C'
---| 'D'
---| 'E'
---| 'F'
---| 'G'
---| 'H'
---| 'I'
---| 'J'
---| 'K'
---| 'L'
---| 'M'
---| 'N'
---| 'O'
---| 'P'
---| 'Q'
---| 'R'
---| 'S'
---| 'T'
---| 'U'
---| 'V'
---| 'W'
---| 'X'
---| 'Y'
---| 'Z'

---@alias Modifier
---| 'ctrl'
---| 'shift'
---| 'alt'
---| 'mod'

---create a valid keybind
---@param key Key
---@param mods table<integer, Modifier>,
function Keybind(mods, key)
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
