# Pyonji

A terminal emulator whose grid is drawn on the GPU, configured from Lua.

> **Status: not yet buildable from a fresh clone.** Four dependencies in
> `Cargo.toml` are `path` deps pointing outside the repository, at
> `../oss/gpui-component` and `../oss/gpui-ce`. See
> [Building](#building).

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
- **A command palette and a `:` prompt**, over one command table: everything
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
  Iosevka, Noto Sans Mono CJK, and two Nerd Font files for icons and emoji.

There is no image protocol (sixel, iTerm inline images) and no session
multiplexer.

## Building

### Prerequisites

- Rust 1.85 or newer. The crate is edition 2024 and declares no `rust-version`.
- A checkout of [gpui-component](https://github.com/gpui-ce/gpui-component) as
  a **sibling directory**:

  ```
  <somewhere>/
    Pyonji/            <- this repository
    oss/
      gpui-component/  <- required
      gpui-ce/         <- required; the [patch] table in Cargo.toml points here
  ```

  `Cargo.toml` points four crates at `../oss/gpui-component/`, and a `[patch]`
  table redirects every `gpui-ce` package to `../oss/gpui-ce/`. Neither is
  vendored, so `cargo build` in a fresh clone fails on a missing path
  dependency until both checkouts are in place.

### Build

```bash
cargo build --release
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

Almost nothing is bound by default — the only global binding Pyonji installs
itself is <kbd>Ctrl</kbd>+<kbd>Shift</kbd>+<kbd>V</kbd> for paste. Keys are
bound from Lua, which is also what makes them remappable:

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

`py.open_palette()`, `py.open_command()` and the other `open_*` methods bind as
the action they stand for rather than as a Lua wrapper, so a key bound to one
of them is the same key the action answers to everywhere else.

Inside an overlay, <kbd>↑</kbd><kbd>↓</kbd> move, <kbd>Enter</kbd> confirms
and <kbd>Esc</kbd> closes — with two deliberate exceptions: on the release
screen <kbd>Esc</kbd> clears the version filter first, and on the palette it
peels the line back one word at a time. Both close on the press that finds
nothing left to peel.

### Commands

There is one command table, and two ways to reach it.

The **palette** (`py:open_palette()`) is a command line with the matching
commands listed under it. <kbd>Tab</kbd> completes the word being typed, and
pressing it again offers the next-best match. The list is fuzzy-matched on the
name, the summary and the placeholders, and grouped by where each command came
from.

The **`:` prompt** in the status bar takes the same grammar without the list:
`switch 2`, `rename my logs`, `ssh server`. <kbd>↑</kbd> and <kbd>↓</kbd> walk
what you have already run, and <kbd>Tab</kbd> completes. A name that matches
nothing is reported in the bar rather than ignored, with the nearest command
offered as a suggestion.

Both are on the right of the status bar as `commands` and `command`, and each
shows the key your config has bound to it. Nothing is bound by default, so
bind them if you want them on the home row:

```lua
py:bind('<ctrl-b> p', py.open_palette())
py:bind('<ctrl-b> g', py.open_command())
```

The built-in commands are `switch`, `next-tab`, `prev-tab`, `close`,
`move-to`, `split-h`, `split-v`, `focus-next-pane`, `detach`, `rename`, `ssh`,
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
| `py:open_command()` | the `:` line in the status bar |
| `py:open_sessions()` | session list |
| `py:open_opener()` | directory picker |
| `py:open_releases()` | release screen and updater |
| `py:open_rename()` | rename the focused session |
| `py:open_lua()` | `>` prompt in the status bar |

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
- [gpui-ce](https://github.com/gpui-ce/gpui-ce) — windowing, input, and the
  UI toolkit the overlays are built from (a sibling checkout, see above)
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

No license file has been added to this repository yet.
