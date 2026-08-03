py:bind({'ctrl', 'shift'}, 'F', py.open_palette());
py:bind({'ctrl', 'shift'}, 'S', py.open_sessions());

py:bind({'shift', 'ctrl'}, 'V', py.create_session(nil, 4, nil, nil));
py:bind({'shift', 'ctrl'}, 'V', py.create_session("", 4, nil, nil));

py:config({
    font_family = "Iosevka",
    font_size = 30.0,
    line_height = 1.1,
    fullscreen = true,
    ssh_sessions = {
        {
            name = "ive",
            ip = "192.168.178.20",
        }
    },
});
