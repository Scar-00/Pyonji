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
---| '0'
---| '1'
---| '2'
---| '3'
---| '4'
---| '5'
---| '6'
---| '7'
---| '8'
---| '9'
---| 'F1'
---| 'F2'
---| 'F3'
---| 'F4'
---| 'F5'
---| 'F6'
---| 'F7'
---| 'F8'
---| 'F9'
---| 'F10'
---| 'F11'
---| 'F12'
---| 'ArrowUp'
---| 'ArrowDown'
---| 'ArrowLeft'
---| 'ArrowRight'
---| 'Space'
---| 'Enter'
---| 'Esc'
---| 'Tab'
---| 'Backspace'
---| 'Delete'
---| 'Insert'
---| 'Home'
---| 'End'
---| 'PageUp'
---| 'PageDown'
---| 'Semicolon'
---| 'Comma'
---| 'Period'
---| 'Slash'
---| 'Backslash'
---| 'Minus'
---| 'Equals'
---| 'Quote'
---| 'Backquote'
---| 'BracketLeft'
---| 'BracketRight'

---@alias Modifier
---| 'ctrl'
---| 'shift'
---| 'alt'
---| 'mod'

---create a valid keybind
---@param key Key
---@param mods table<integer, Modifier>
---@return string
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
