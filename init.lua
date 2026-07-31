py:bind("<ctrl + shift>-W", py:open_palette());

py:config({
    font_family = "Iosevka",
    font_size = 30.0,
    line_height = 1.1,
    fullscreen = true,
    ssh_sessions = {
        {
            name = "ive",
            user_name = "ive",
            ip = "192.168.178.20",
        }
    },
});
