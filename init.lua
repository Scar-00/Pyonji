local function replace_root(dir)
    local active = py.active_session;
    local main = py:create_session(dir, 1, nil, nil);
    py:close(active);
    py:switch_tab(1);
    py:detach();
    py:attach(main, 0);
    return main;
end

---@alias Os
---| 'windows'
---| 'linux'
---| 'macos'
---| 'unknown'

---@return Os
function Os()
    local current_os = os.getenv("OS") or "";
    if string.match(current_os, "Windows") then
        return 'windows';
    end
    return 'unknown';
end

function Def()
    if py.tab_count > 1 then
        print("already workspace active");
        return
    end

    local dirs = {
        ['windows'] = "C:/dev/learning",
        ['unix'] = "~/dev",
        ['macos'] = "~/dev",
    };

    local dir = dirs[Os()];

    local main = replace_root(dir);
    py:rename(main, "main");
end

function Work()
    if py.tab_count > 1 then
        print("already workspace active");
        return
    end

    local main = replace_root("C:/aimline/src/dfm-git3");
    py:rename(main, "aimline-main");
end

local function NextFreeTab()
    local tab_count = #py.sessions;
    if tab_count < 9 then
        return tab_count;
    end
    return nil;
end

function PY(current)
    local main = nil;
    if current then
        main = replace_root("C:/dev/learning/pyonji");
    else
        local next_tab = NextFreeTab();
        if next_tab == nil then
            return;
        end
        main = py:create_session("C:/dev/learning/pyonji", next_tab)
    end
    py:rename(main, "nvim-py");
    py:write_to(main, "nvim .\r");
end

local function open(path)
    local tab = NextFreeTab();
    if tab == nil then
        return;
    end
    py:create_session(path, tab, nil, nil);
    py:switch_tab(tab);
end

py:register("open", open);

py:config({
    font_family = "Iosevka",
    font_size = 38.0,
    line_height = 1.1,
    fullscreen = false,
    ssh_sessions = {
        {
            name = "ive",
            ip = "192.168.178.20",
        }
    },
});
