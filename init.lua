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
    font_size = 30.0,
    line_height = 1.1,
    fullscreen = false,
    ssh_sessions = {
        {
            name = "ive",
            ip = "192.168.178.20",
        }
    },
});
--- Startup times for process: Primary (or UI client) ---

times in msec
 clock   self+sourced   self:  sourced script
 clock   elapsed:              other lines

000.006  000.006: --- NVIM STARTING ---
000.096  000.089: event init
000.284  000.188: early init
018.716  018.433: locale set
018.781  000.065: init first window
019.983  001.202: inits 1
020.110  000.127: window checked
020.123  000.013: parsing arguments
021.455  000.695  000.695: require('vim._core.shared')
022.485  000.006  000.006: require('string.buffer')
022.537  000.323  000.317: require('vim.inspect')
022.897  000.344  000.344: require('vim._core.options')
022.906  001.438  000.772: require('vim._core.editor')
023.134  000.223  000.223: require('vim._core.system')
023.141  002.507  000.150: require('vim._init_packages')
023.163  000.533: init lua interpreter
037.852  014.688: nvim_ui_attach
038.293  000.442: nvim_set_client_info
038.301  000.008: --- NVIM STARTED ---

--- Startup times for process: Embedded ---

times in msec
 clock   self+sourced   self:  sourced script
 clock   elapsed:              other lines

000.006  000.006: --- NVIM STARTING ---
000.070  000.063: event init
000.177  000.107: early init
018.615  018.439: locale set
018.668  000.053: init first window
019.532  000.863: inits 1
019.554  000.023: window checked
019.564  000.010: parsing arguments
020.890  000.714  000.714: require('vim._core.shared')
021.891  000.006  000.006: require('string.buffer')
021.942  000.323  000.317: require('vim.inspect')
022.287  000.330  000.330: require('vim._core.options')
022.296  001.395  000.742: require('vim._core.editor')
022.504  000.202  000.202: require('vim._core.system')
022.510  002.452  000.140: require('vim._init_packages')
022.531  000.515: init lua interpreter
023.055  000.524: expanding arguments
023.091  000.036: inits 2
024.070  000.979: init highlight
024.075  000.005: waiting for UI
024.224  000.149: done waiting for UI
024.247  000.023: clear screen
025.194  000.060  000.060: require('vim.keymap')
026.332  000.323  000.323: sourcing nvim_exec2()
026.415  000.033  000.033: require('vim._core.log')
143.968  002.722  002.722: require('vim.tty')
144.143  000.129  000.129: require('vim.text')
158.633  134.379  131.113: require('vim._core.defaults')
158.648  000.022: init default mappings & autocommands
159.296  000.164  000.164: sourcing C:\Program Files\Neovim\share\nvim\runtime\ftplugin.vim
159.870  000.093  000.093: sourcing C:\Program Files\Neovim\share\nvim\runtime\indent.vim
162.574  000.315  000.315: require('vim._async')
162.592  001.764  001.448: require('vim.pack')
163.030  000.419  000.419: require('vim.fs')
163.271  000.031  000.031: require('vim.F')
217.416  056.857  054.643: require('plugin')
220.687  003.235  003.235: require('vim.filetype')
238.390  008.938  008.938: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\gruvbox\colors\gruvbox.vim
241.737  000.222  000.222: sourcing C:\Program Files\Neovim\share\nvim\runtime\syntax\synload.vim
245.915  000.077  000.077: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-fugitive\ftdetect\fugitive.vim
246.979  000.108  000.108: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\rust.vim\ftdetect\rust.vim
247.827  003.301  003.116: sourcing nvim_exec2() called at C:\Program Files\Neovim\share\nvim\runtime\filetype.lua:0
247.844  003.534  000.233: sourcing C:\Program Files\Neovim\share\nvim\runtime\filetype.lua
255.268  006.041  006.041: require('vim.filetype.detect')
256.802  017.262  007.465: sourcing C:\Program Files\Neovim\share/nvim/runtime/syntax/syntax.vim
257.177  036.088  009.888: sourcing C:\Users\fiffi/.config/nvim/legacy.vim
257.201  036.493  000.405: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
263.312  001.615  001.615: require('cmp.utils.debug')
266.887  001.841  001.841: require('cmp.utils.char')
266.909  003.585  001.744: require('cmp.utils.str')
272.390  001.838  001.838: require('cmp.utils.misc')
274.081  001.675  001.675: require('cmp.utils.buffer')
275.825  001.731  001.731: require('cmp.utils.api')
275.852  007.199  001.954: require('cmp.utils.keymap')
275.860  008.944  001.745: require('cmp.utils.feedkeys')
285.156  001.660  001.660: require('cmp.types.cmp')
286.962  001.790  001.790: require('cmp.types.lsp')
288.544  001.571  001.571: require('cmp.types.vim')
288.555  006.978  001.956: require('cmp.types')
288.567  008.876  001.897: require('cmp.config.mapping')
290.225  001.652  001.652: require('cmp.utils.cache')
293.830  001.856  001.856: require('cmp.config.compare')
295.481  001.640  001.640: require('cmp.config.window')
295.491  005.252  001.757: require('cmp.config.default')
295.527  017.738  001.958: require('cmp.config')
295.564  019.699  001.962: require('cmp.utils.async')
298.915  001.622  001.622: require('cmp.utils.pattern')
298.931  003.360  001.738: require('cmp.context')
305.028  002.060  002.060: require('cmp.utils.snippet')
306.906  001.865  001.865: require('cmp.matcher')
310.184  009.346  005.421: require('cmp.entry')
310.202  011.265  001.919: require('cmp.source')
313.954  001.700  001.700: require('cmp.utils.event')
319.538  001.634  001.634: require('cmp.utils.options')
319.554  003.712  002.077: require('cmp.utils.window')
319.562  005.595  001.883: require('cmp.view.docs_view')
323.374  001.720  001.720: require('cmp.utils.autocmd')
323.394  003.827  002.106: require('cmp.view.custom_entries_view')
325.234  001.833  001.833: require('cmp.view.wildmenu_entries_view')
327.101  001.856  001.856: require('cmp.view.native_entries_view')
328.912  001.797  001.797: require('cmp.view.ghost_text_view')
328.935  018.726  002.118: require('cmp.view')
329.360  069.746  002.552: require('cmp.core')
331.335  001.823  001.823: require('cmp.config.sources')
331.443  073.733  002.164: require('cmp')
332.012  074.540  000.807: require('lsp.cmp')
336.386  002.066  002.066: require('cmp_nvim_lsp.source')
336.397  004.073  002.007: require('cmp_nvim_lsp')
344.991  003.176  003.176: require('vim.lsp.protocol')
345.131  005.611  002.434: require('vim.lsp.log')
349.108  003.965  003.965: require('vim.lsp.util')
354.353  002.546  002.546: require('vim.lsp.sync')
354.369  005.245  002.699: require('vim.lsp._changetracking')
359.644  002.572  002.572: require('vim.lsp._transport')
359.724  000.067  000.067: require('vim._core.stringbuffer')
359.806  005.429  002.791: require('vim.lsp.rpc')
359.883  023.450  003.201: require('vim.lsp')
366.449  003.359  003.359: require('vim.lsp.completion')
366.555  006.640  003.280: require('vim.lsp.handlers')
370.996  004.429  004.429: require('vim.diagnostic')
390.652  002.928  002.928: require('lspconfig.util')
394.761  062.740  021.221: require('lsp.lsp')
394.782  137.569  000.289: require('lsp')
396.642  000.578  000.578: require('mason-core.path')
398.642  000.534  000.534: require('mason-core.functional.data')
399.279  000.613  000.613: require('mason-core.functional.function')
400.030  000.726  000.726: require('mason-core.functional.list')
400.587  000.524  000.524: require('mason-core.functional.relation')
401.132  000.524  000.524: require('mason-core.functional.logic')
401.657  000.505  000.505: require('mason-core.functional.number')
402.267  000.582  000.582: require('mason-core.functional.string')
402.849  000.560  000.560: require('mason-core.functional.table')
403.414  000.509  000.509: require('mason-core.functional.type')
403.434  006.014  000.936: require('mason-core.functional')
403.597  006.930  000.917: require('mason-core.platform')
404.250  000.639  000.639: require('mason.settings')
404.277  008.876  000.729: require('mason-core.installer.InstallLocation')
406.326  000.769  000.769: require('mason-core.log')
406.349  001.343  000.574: require('mason-core.EventEmitter')
407.117  000.754  000.754: require('mason-registry.sources')
407.264  002.974  000.877: require('mason-registry')
407.283  012.486  000.636: require('mason')
408.345  000.886  000.886: require('mason.api.command')
411.624  000.719  000.719: require('fidget.spinner.patterns')
411.635  001.301  000.582: require('fidget.spinner')
412.909  000.653  000.653: require('fidget.health')
412.920  001.277  000.624: require('fidget.options')
412.976  003.303  000.726: require('fidget.progress.display')
414.530  000.756  000.756: require('fidget.logger')
414.549  001.565  000.810: require('fidget.progress.lsp')
418.040  000.860  000.860: require('fidget.poll')
418.059  002.028  001.168: require('fidget.notification.model')
419.255  001.190  001.190: require('fidget.notification.window')
420.437  001.171  001.171: require('fidget.notification.view')
420.491  005.257  000.868: require('fidget.notification')
420.520  005.964  000.707: require('fidget.progress.handle')
420.568  011.612  000.779: require('fidget.progress')
421.765  001.190  001.190: require('fidget.commands')
422.966  000.599  000.599: require('fidget.integration.nvim-tree')
423.630  000.653  000.653: require('fidget.integration.xcodebuild-nvim')
423.644  001.866  000.614: require('fidget.integration')
423.669  015.310  000.642: require('fidget')
432.678  002.591  002.591: require('vim.version')
436.974  009.782  007.191: require('lsp_signature.helper')
439.825  002.755  002.755: require('vim.iter')
439.864  015.642  003.105: require('lsp_signature')
442.981  003.028  003.028: require('vim.lsp.client')
451.853  001.523  001.523: require('telescope.builtin')
452.115  008.871  007.348: require('vim.remap')
454.783  000.788  000.788: require('fluoromachine.palette')
456.939  001.278  001.278: require('fluoromachine.utils')
458.151  001.193  001.193: require('fluoromachine.utils.color')
458.164  003.368  000.897: require('fluoromachine.highlights')
458.174  005.029  000.874: require('fluoromachine.config')
458.181  005.839  000.810: require('fluoromachine')
460.982  000.835  000.835: require('rose-pine.config')
460.996  002.757  001.922: require('rose-pine')
462.438  000.267  000.267: require('gruber-darker.config')
462.449  000.562  000.296: require('gruber-darker')
463.431  000.280  000.280: require('gruber-darker.highlight')
463.977  000.259  000.259: require('gruber-darker.color')
463.991  000.548  000.289: require('gruber-darker.palette')
464.036  001.252  000.424: require('gruber-darker.highlights.colorscheme')
464.878  000.525  000.525: require('gruber-darker.highlights.vim')
464.892  000.850  000.325: require('gruber-darker.highlights.lsp')
465.183  000.285  000.285: require('gruber-darker.highlights.terminal')
465.616  000.423  000.423: require('gruber-darker.highlights.treesitter')
465.969  000.343  000.343: require('gruber-darker.highlights.cmp')
466.269  000.290  000.290: require('gruber-darker.highlights.telescope')
466.559  000.280  000.280: require('gruber-darker.highlights.rainbow')
466.569  004.114  000.391: require('gruber-darker.highlights')
468.584  006.802  002.126: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\gruber-darker.nvim\colors\gruber-darker.lua
468.861  016.738  001.340: require('vim.color')
468.869  025.844  000.235: require('vim')
469.125  000.249  000.249: require('neovide')
472.456  001.462  001.462: require('lualine_require')
476.281  007.147  005.685: require('lualine')
476.478  000.040  000.040: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
485.658  001.807  001.807: require('lualine.highlight')
488.709  000.022  000.022: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
488.746  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
488.771  000.009  000.009: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
488.789  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
488.807  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
488.828  000.008  000.008: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
488.845  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
488.862  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.162  000.013  000.013: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.180  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.198  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.217  000.008  000.008: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.237  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.254  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.272  000.009  000.009: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.290  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.305  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.326  000.008  000.008: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.488  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.506  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.589  000.009  000.009: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.606  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
489.633  000.019  000.019: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
500.823  001.401  001.401: require('lualine.utils.mode')
509.828  000.056  000.056: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
509.863  000.011  000.011: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
513.824  000.022  000.022: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
513.858  000.008  000.008: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.057  000.185  000.185: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.091  000.008  000.008: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.111  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.175  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.198  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.222  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.242  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.260  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.277  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.298  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.315  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.334  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.360  000.011  000.011: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.380  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.397  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.414  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.433  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.449  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
514.471  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
516.735  000.059  000.059: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
516.759  000.005  000.005: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
516.802  000.034  000.034: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
516.822  000.011  000.011: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.116  000.022  000.022: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.157  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.181  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.211  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.230  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.253  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.278  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.409  000.010  000.010: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.435  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.456  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.476  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.497  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.518  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.539  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.562  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.695  000.009  000.009: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.758  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.813  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.837  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.855  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.872  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.897  000.008  000.008: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.933  000.008  000.008: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.956  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
525.981  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
526.006  000.008  000.008: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
526.027  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
526.047  000.006  000.006: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
537.752  000.031  000.031: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
537.866  000.045  000.045: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
537.914  000.036  000.036: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
537.934  000.009  000.009: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
537.954  000.007  000.007: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim\init.lua:0
539.008  001.031  001.031: require('nvim-treesitter')
540.145  001.126  001.126: require('nvim-treesitter.config')
547.263  001.326  001.326: require('nvim-autopairs._log')
548.697  001.416  001.416: require('nvim-autopairs.utils')
548.719  004.241  001.499: require('nvim-autopairs.conds')
548.732  005.555  001.314: require('nvim-autopairs.rule')
548.742  006.878  001.323: require('nvim-autopairs.rules.basic')
548.766  008.610  001.732: require('nvim-autopairs')
549.326  388.914  059.125: sourcing C:\Users\fiffi\AppData\Local\nvim\init.lua
549.414  001.595: sourcing vimrc file(s)
552.265  000.090  000.090: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\compile-mode.nvim\plugin\completion.vim
552.690  000.165  000.165: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\compile-mode.nvim\plugin\highlights.vim
553.014  000.057  000.057: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\compile-mode.nvim\plugin\statusline.vim
561.058  001.225  001.225: require('plenary.tbl')
561.075  002.399  001.174: require('plenary.vararg.rotate')
561.082  003.519  001.121: require('plenary.vararg')
562.165  001.077  001.077: require('plenary.errors')
563.670  001.494  001.494: require('plenary.functional')
563.689  007.423  001.333: require('plenary.async.async')
568.699  001.189  001.189: require('plenary.async.structs')
568.722  002.595  001.406: require('plenary.async.control')
568.768  003.874  001.280: require('plenary.async.util')
568.777  005.080  001.206: require('plenary.async.tests')
568.784  014.033  001.531: require('plenary.async')
573.172  002.923  002.923: require('vim.ui')
573.253  003.678  000.755: require('compile-mode.utils')
573.297  004.507  000.829: require('compile-mode.errors')
573.931  000.626  000.626: require('compile-mode.log')
573.970  020.545  001.379: require('compile-mode')
574.005  020.733  000.188: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\compile-mode.nvim\plugin\command.lua
599.897  024.808  024.808: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\lazygit.nvim\plugin\lazygit.vim
601.129  000.122  000.122: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\neoformat\plugin\neoformat.vim
614.632  002.677  002.677: require('vim.treesitter.language')
616.940  002.291  002.291: require('vim.func')
619.227  002.273  002.273: require('vim.treesitter._range')
621.409  002.166  002.166: require('vim.func._memoize')
621.481  012.488  003.082: require('vim.treesitter.query')
621.584  016.274  003.786: require('vim.treesitter.languagetree')
621.603  018.987  002.713: require('vim.treesitter')
621.802  019.445  000.458: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\nvim-treesitter\plugin\filetypes.lua
622.388  000.253  000.253: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\nvim-treesitter\plugin\nvim-treesitter.lua
622.828  000.155  000.155: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\nvim-treesitter\plugin\query_predicates.lua
624.196  000.505  000.505: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\telescope.nvim\plugin\telescope.lua
625.124  000.154  000.154: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\plenary.nvim\plugin\plenary.vim
628.251  002.384  002.384: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-fugitive\plugin\fugitive.vim
636.144  007.027  007.027: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-easymotion\plugin\EasyMotion.vim
638.166  000.935  000.935: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-better-whitespace\plugin\better-whitespace.vim
639.070  000.091  000.091: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\nvim-web-devicons\plugin\nvim-web-devicons.vim
640.346  000.336  000.336: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\nvim-dap\plugin\dap.lua
641.592  000.237  000.237: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\rust.vim\plugin\cargo.vim
641.971  000.092  000.092: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\rust.vim\plugin\rust.vim
644.470  000.219  000.219: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-colortuner\autoload\colortuner.vim
644.915  002.181  001.962: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-colortuner\plugin\colortuner.vim
652.157  000.585  000.585: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vital\vsnip.vim
653.905  000.171  000.171: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vital\_vsnip\VS\LSP\Position.vim
655.417  000.099  000.099: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vital\_vsnip.vim
655.962  005.814  004.959: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vsnip\snippet.vim
657.966  000.287  000.287: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vital\_vsnip\VS\LSP\TextEdit.vim
659.744  000.141  000.141: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vital\_vsnip\VS\LSP\Text.vim
661.676  000.226  000.226: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vital\_vsnip\VS\Vim\Buffer.vim
663.519  000.139  000.139: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vital\_vsnip\VS\Vim\Option.vim
665.572  000.283  000.283: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vital\_vsnip\VS\LSP\Diff.vim
665.900  017.168  010.277: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vsnip\session.vim
666.284  019.054  001.886: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\autoload\vsnip.vim
667.058  021.367  002.313: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-vsnip\plugin\vsnip.vim
670.238  001.776  001.776: require('cmp.utils.highlight')
671.061  000.018  000.018: sourcing nvim_exec2() called at C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\nvim-cmp\plugin\cmp.lua:0
671.072  003.097  001.302: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\nvim-cmp\plugin\cmp.lua
673.105  000.706  000.706: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\nvim-lspconfig\plugin\lspconfig.lua
678.069  000.346  000.346: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\gzip.vim
685.644  000.723  000.723: sourcing C:\Program Files\Neovim\share\nvim\runtime\pack\dist\opt\matchit\plugin\matchit.vim
685.912  007.556  006.834: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\matchit.vim
686.509  000.289  000.289: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\matchparen.vim
694.198  000.729  000.729: sourcing C:\Program Files\Neovim\share\nvim\runtime\pack\dist\opt\netrw\plugin\netrwPlugin.vim
694.459  007.644  006.915: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\netrwPlugin.vim
695.608  000.060  000.060: sourcing C:\Users\fiffi\AppData\Local\nvim-data/rplugin.vim
695.646  000.889  000.828: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\rplugin.vim
696.109  000.183  000.183: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\shada.vim
696.478  000.077  000.077: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\spellfile.vim
696.964  000.201  000.201: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\tarPlugin.vim
697.527  000.245  000.245: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\tohtml.vim
697.910  000.086  000.086: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\tutor.vim
698.476  000.271  000.271: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\zipPlugin.vim
698.975  000.179  000.179: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\editorconfig.lua
699.544  000.269  000.269: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\man.lua
700.266  000.424  000.424: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\net.lua
700.802  000.234  000.234: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\nvim.lua
701.418  000.317  000.317: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\osc52.lua
701.982  000.265  000.265: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\shada.lua
702.414  000.130  000.130: sourcing C:\Program Files\Neovim\share\nvim\runtime\plugin\spellfile.lua
702.700  028.738: loading rtp plugins
703.467  000.767: loading packages
706.682  002.223  002.223: require('cmp_cmdline')
706.810  002.491  000.268: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\cmp-cmdline\after\plugin\cmp_cmdline.lua
710.093  002.272  002.272: require('cmp_path')
710.148  002.460  000.188: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\cmp-path\after\plugin\cmp_path.lua
719.197  001.821  001.821: require('cmp_buffer.timer')
719.216  004.123  002.302: require('cmp_buffer.buffer')
719.225  006.305  002.182: require('cmp_buffer.source')
719.232  008.107  001.802: require('cmp_buffer')
719.289  008.312  000.205: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\cmp-buffer\after\plugin\cmp_buffer.lua
720.281  000.127  000.127: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\cmp-nvim-lsp\after\plugin\cmp_nvim_lsp.lua
720.308  003.451: loading after plugins
720.394  000.087: inits 3
724.744  004.350: reading ShaDa
725.380  000.635: opening buffers
725.801  000.421: BufEnter autocommands
725.814  000.013: editing files in windows
727.907  000.159  000.159: sourcing C:\Users\fiffi\AppData\Local\nvim-data\site\pack\core\opt\vim-colortuner\autoload\colortuner\conv.vim
848.837  122.864: VimEnter autocommands
849.300  000.463: UIEnter autocommands
908.166  056.008  056.008: sourcing C:\Program Files\Neovim\share\nvim\runtime\autoload\provider\clipboard.vim
908.203  002.895: before starting main loop
908.999  000.796: first screen update
909.006  000.007: --- NVIM STARTED ---

