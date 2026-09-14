local function replace_root(dir)
    local active = py.active_session;
    local main = py:create_session(dir, 1);
    py:close(active);
    py:switch_tab(1);
    py:detach();
    py:attach(main, 0);
    return main;
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

function Work(current)
    local main = nil;
    if current then
        main = replace_root("C:/aimline/src/dfm-git3");
    else
        local next_tab = NextFreeTab();
        if next_tab == nil then
            return;
        end
        main = py:create_session("C:/aimline/src/dfm-git3", next_tab);
    end
    py:rename(main, "aimline-main");
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
    py:create_session(path, tab);
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

--[[Workspaces = {};

local function create_workspace(name)
    local workspace = { tabs = py.sessions };
    for _, tab in pairs(py.sessions) do
        for _, session in tab do
            py:detach(session);
        end
    end
end]]--

function NextFreeTab()
    local tab_count = #py.sessions;
    if tab_count < 9 then
        return tab_count;
    end
    return nil;
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
