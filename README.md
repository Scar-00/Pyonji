# Pyonji

A terminal emulator whose grid is drawn on the GPU, configured from Lua.

## What it does

- **GPU-drawn grid.** One wgpu render pass per frame, with three pipelines:
  the window background, the terminal grid, and the pane dividers. Glyphs and
  cell backgrounds are both submitted as GPU primitives rather than composited
  on the CPU. The backend is whatever wgpu selects for the platform — it is not
  pinned to Vulkan.
- **Terminal emulation** via [vt100](https://github.com/doy/vt100-rust), with
  2000 lines of scrollback.
- **Tabs and split panes.** Up to 9 tabs, each holding a tree of splits
  (horizontal or vertical).
- **Colour.** A fixed 16-colour palette, the standard xterm 6×6×6 cube and
  grayscale ramp for 256-colour, and 24-bit truecolor passed straight through.
  The 16 base colours are not configurable.
- **Cursor shape** follows the terminal's own `DECSCUSR` sequence — bar, block
  or underline — so applications that set it are honoured.
- **IME preedit** and clipboard paste on <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd>.
- **SSH sessions** over libssh2, declared in the config.
- **Overlays** for the command palette, the session list, a directory picker,
  and the release screen.
- **A command palette**, over one command table: everything
  Pyonji can do, one entry per configured SSH host, and one per callback the
  config registers.
- **A status bar** that doubles as a prompt: tab chips, a session rename field,
  a `:` command line with history and completion, and a `>` Lua line with LSP
  completion.
- **Self-update** from GitHub releases, showing which build matches the
  machine it is running on.
- **Lua configuration**, reloaded from disk when it changes and written on
  first run if missing.
- **Bundled fonts** — nothing to install. Iosevka Term across nine weights,
  Iosevka, Noto Sans Mono CJK, Nerd Fonts for icons, and Noto Emoji for
  monochrome emoji in the terminal foreground colour.

There is no image protocol (sixel, iTerm inline images) and no session
multiplexer.

## Building

### Prerequisites

- Rust 1.95 or newer, as required by GPUI.
- Git and the platform's C/C++ build tools for native dependencies.

Cargo fetches [the GPUI fork](https://github.com/Scar-00/gpui-ce-fork)
and [the component fork](https://github.com/Scar-00/gpui-component-fork)
directly from GitHub. No sibling checkouts are required. The GPUI revision
is pinned in `Cargo.toml`, including the patches used by the component crates.

On Debian/Ubuntu, install the native development libraries used by GPUI:

```bash
sudo apt-get install libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev libx11-dev libxcb-shape0-dev libxcb-xfixes0-dev libxcb-randr0-dev libxcb-xinput-dev libegl1-mesa-dev libgles2-mesa-dev libglib2.0-dev libfontconfig-dev libssl-dev
```

### Build

```bash
cargo build --locked --release
```

Run it in place, reading `./init.lua`:

```bash
cargo run --release
```

Build with the `install` feature for a machine-wide install. The config then
lives in the platform config directory (`%LOCALAPPDATA%\pyonji\init.lua` on
Windows) instead of the working directory:

```bash
cargo build --release --features install
```

### Passing a starting directory

```bash
cargo run --release -- ~/dev/project
```

The single optional positional argument is the directory the first session
opens in. Without it, `default_cwd` from the config is used, falling back to
the process working directory.

### Releases

Pushing a `v*` tag builds three binaries and attaches them to a GitHub
release, which is where the in-app updater looks:

| Target | Feature |
|---|---|
| `x86_64-unknown-linux-gnu` | — |
| `x86_64-pc-windows-msvc` | `install` |
| `aarch64-apple-darwin` | — |

Pull requests are built and linted with `clippy -D warnings`.

## Configuration

The config is a Lua file executed on load, not a table to return. It is
re-executed whenever it changes on disk.

By default it is `./init.lua`; with the `install` feature it is
`<config dir>/pyonji/init.lua`. A commented starter file is written on first
run.

```lua
py:config({
    font_family = "Iosevka",
    font_size = 24,
    line_height = 1.1,      -- a multiple of font_size
    fullscreen = false,
    default_cwd = nil,
    editor = "nvim",        -- typed into a session when the opener picks a file
    ssh_sessions = {
        { name = "server", user_name = "root", ip = "192.168.1.100" },
    },
})
```

`resources/default.lua` is meant to be the annotated reference for the `py`
object. Treat it as a draft: it lists several methods that are not implemented
(see below).

### Keybindings

Commands is available from the status bar and with **Ctrl+Shift+P**. Use
**Ctrl+Shift+C** to copy selected terminal text and **Ctrl+Shift+V** to paste.
Drag to select output; hold **Shift** to select locally in mouse-enabled apps.
Right-click a pane for Copy and Paste; hold Shift when the application uses the mouse. Selection is scoped to that pane and
clears when its output changes or it is resized. Scrollback can be selected,
and scrolling during a drag extends the selection into history.

Keys are bound from Lua and can be remapped:

```lua
py:bind('<ctrl-b> f', py.open_sessions())
py:bind('<ctrl-b> r', py.open_rename())
py:bind('<ctrl-b> 1', py.switch_tab(0))   -- first tab
py:bind('<ctrl-b> v', py.split('v'))
```

`py:bind` takes a binding string and a callback. A method called with `py:`
returns its value; called as `py.method` it returns the callback to bind. The
`(modifiers, key, callback)` form works too, and is the same binding written
out:

```lua
py:bind({ "ctrl", "shift" }, "F", py.open_sessions())   -- '<ctrl-shift> F'
```

A binding whose modifiers are followed by another word is a *sequence*, not a
chord: `'<ctrl-b> f'` waits for <kbd>Ctrl</kbd>+<kbd>B</kbd> and then for
<kbd>F</kbd>, like a prefix key. `<ctrl-shift>F` is the chord. The status bar
writes sequences the same way when it shows you a key.

`py.open_palette()` binds as the Commands action, so its configured shortcut
is shown on the status bar. Other callbacks run through Lua.

Inside an overlay, <kbd>↑</kbd><kbd>↓</kbd> move, <kbd>Enter</kbd> confirms
and <kbd>Esc</kbd> closes — with two deliberate exceptions: on the release
screen <kbd>Esc</kbd> clears the version filter first, and on the palette it
clears arguments first, then the command filter. Both close on the press that finds
nothing left to peel.

### Commands

Commands uses one command table.

The **palette** (`py:open_palette()`) is a command line with the matching
commands listed under it. <kbd>Tab</kbd> completes the word being typed, and
pressing it again offers the next-best match. The list is fuzzy-matched on the
name, the summary and the placeholders, and grouped by where each command came
from.

The Commands button shows the shortcut from your configuration. For a prefix
sequence, bind it with:

```lua
py:bind('<ctrl-b> p', py.open_palette())
```

The built-in commands are `switch`, `next-tab`, `prev-tab`, `close`,
`move-to`, `split-h`, `split-v`, `focus-next-pane`, `detach`, `detached`, `rename`, `ssh`,
`open-in`, `sessions`, `releases`, `commands`, `lua` and `reload-config`. Tabs
are written 1-based, the way the status bar counts them; sessions by the bare
number the status bar shows.

`py:register` puts a callback in the same table, so it is reachable from both
surfaces. The argument names are read off the Lua function and show up as
placeholders:

```lua
py:register("open", function (path, tab)
    local id = py:create_session(path, tab);
    py:write(id, "nvim .\r");
end);
```

### Opening things

| Call | Opens |
|---|---|
| `py:open_palette()` | command palette |
| `py:open_sessions()` | session list |
| `py:open_detached()` | detached sessions and attachment destination |
| `py:open_opener()` | directory picker |
| `py:open_releases()` | release screen and updater |
| `py:open_rename()` | rename the focused session |
| `py:open_lua()` | `>` prompt in the status bar |

The detached session view is available through `detached` in the command
palette, or a configured binding:

```lua
py:bind('<ctrl-b> d', py.open_detached())
```

Search by name or session number and choose with <kbd>↑</kbd>/<kbd>↓</kbd>.
Press <kbd>Enter</kbd> to attach to the current tab, or
<kbd>Alt</kbd>+<kbd>1</kbd>–<kbd>9</kbd> to attach directly to that tab.
An occupied tab gains a split; an empty tab uses the existing session
without starting another process. <kbd>Esc</kbd> clears a search first,
then closes the view. The general session list also puts detached sessions
first, while **Switch** focuses sessions already in tabs.

## Not implemented

`resources/default.lua` annotates four methods that `config.rs` never
registers, so calling them from a config errors: `toggle_fullscreen`,
`toggle_decorations`, `toggle_status_bar` and `quit`. It also documents a
`status_height` config field that nothing reads.

The Lua prompt keeps no history. Its line lives inside the prompt's own view
rather than on the bar, which is also why <kbd>↑</kbd> and <kbd>↓</kbd> there
move the language server's suggestions instead of walking what came before.

`resources/main.ts` was the palette's first prototype, in TypeScript for a
`gpui-shell` host that is not part of the build. The palette in
`src/ui/overlay/palette.rs` replaces it; the prototype is gone.

## Tech stack

- [wgpu](https://wgpu.rs/) — GPU rasterisation
- [gpui-ce](https://github.com/Scar-00/gpui-ce-fork) — windowing, input, and the
  UI toolkit the overlays are built from
- [gpui-component](https://github.com/gpui-ce/gpui-component) — dialogs,
  inputs, buttons, scrollbars, and the theme
- [vt100](https://github.com/doy/vt100-rust) — terminal emulation
- [portable-pty](https://github.com/wezterm/wezterm) — local PTYs
- [ssh2](https://github.com/alexcrichton/ssh2-rs) — SSH
- [swash](https://github.com/BrianSharpe/swash) — font shaping and rasterising
- [mlua](https://github.com/mlua-rs/mlua) — Lua, vendored build
- [self_update](https://github.com/jaemk/self_update) — the updater
- [async-lsp](https://github.com/oxidecomputer/async-lsp) — completion in the
  Lua prompt

## License

No license file has been added to this repository yet. The bundled Noto Emoji
font is licensed under the SIL Open Font License 1.1; its license and source
are in `resources/fonts/NotoEmoji-OFL.txt` and `resources/fonts/NotoEmoji-README.md`.
