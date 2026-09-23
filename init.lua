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
    py:write(main, "nvim .\r");
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
    status_height = 0.75,
});

function NextFreeTab()
    local tab_count = #py.sessions;
    if tab_count < 9 then
        return tab_count;
    end
    return nil;
end

py:bind('<ctrl-b> l', py.open_lua());
py:bind('<ctrl-b> r', py.open_rename());
py:bind('<ctrl-b> f', py.open_sessions());
py:bind('<ctrl-b> o', function ()
    open("/home/ahri/dev/core/");
end);

for i = 1, 9 do
    py:bind('<ctrl-b> ' .. i, py.switch_tab(i - 1));
end

py:config({
    editor = "nvim",
});

py:bind('<alt-shift> q', function()
    py:close(py.active_session);
end)

