return function (fn)
    local info = debug.getinfo(fn, 'u');
    local names = {};
    for i = 1, info.nparams do
        local name = debug.getlocal(fn, i);
        names[i] = name;
    end
    return names, info.isvararg;
end
