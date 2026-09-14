 Pyonji

A GPU-accelerated terminal emulator written in Rust with Lua configuration, SSH support, and an in-app command palette.

## Features

- GPU-accelerated rendering via wgpu (Vulkan)
- Multi-tab support (up to 9 tabs) with split panes
- VT100/xterm terminal emulation with 2000-line scrollback
- 256-color + true color (24-bit) ANSI support (Catppuccin-inspired palette)
- Multiple cursor styles (Bar, Block, Underline)
- Pane splitting (horizontal/vertical) with keyboard resize
- Overlay dialogs with fuzzy-search command palette
- SSH remote session management
- Release list from GitHub releases
- In-app directory picker for opening sessions in a chosen folder
- Lua-based configuration with hot-reloading
- Multiple bundled fonts (Iosevka, NotoSansMonoCJK, Nerd Font icons)

## Requirements

- Rust 1.85+ (Edition 2024)
- Windows (uses `cmd.exe` by default)

Fonts are bundled — no manual font installation required.

## Installation

```bash
cargo build --release
```

Run in portable mode from the project directory, or install:

```bash
cargo build --release --features install
```

## Usage

```bash
cargo run --release [path]
```

Pass an optional path to start a session in a specific directory.

### Tabs & Panes

Press `Ctrl+B` then release, followed by:

| Key | Action |
|-----|--------|
| `1`–`9` | Switch to / create tab |
| `K` | Next tab |
| `J` | Previous tab |
| `H` | Split pane horizontally |
| `V` | Split pane vertically |
| `W` | Focus next pane |
| `R` | Reload config |
| `S` | Toggle status bar |
| `P` | Open command palette |

Hold `Ctrl+B` and press **Arrow Keys** to resize the active pane.

### Overlay

| Shortcut | Overlay |
|----------|---------|
| `Ctrl+Shift+F` or `Ctrl+B` `P` | Command palette (fuzzy search) |
| `Ctrl+Shift+S` | Sessions list |
| `Escape` | Close current overlay |

Within a dialog, use **Arrow Keys** to navigate and **Enter** to confirm.

### Mouse

- Left click a pane to focus it
- Scroll wheel for scrollback / alternate screen scrolling

### Configuration

Create an `init.lua` file in the project directory (or `%LOCALAPPDATA%/pyonji/` with the `install` feature):

```lua
return {
  font_family = "Iosevka Term",
  font_size = 24,
  line_height = 28/24,
  fullscreen = false,
  default_cwd = "C:\\Users\\me\\projects",
  ssh_sessions = {
    { name = "server", user_name = "root", ip = "192.168.1.100" }
  },
  open_palette = "<ctrl>-p",
}
```

Config changes are applied automatically at runtime.

## Tech Stack

- [wgpu](https://wgpu.rs/) — GPU terminal rendering
- [GPUI](https://github.com/gpui-ce/gpui-ce) — Window management and UI (dialogs, status bar)
- [vt100](https://github.com/doy/vt100-rust) — Terminal emulation
- [portable-pty](https://github.com/wez/wezterm) — PTY handling
- [mlua](https://github.com/khvzak/mlua) — Lua config integration
- [swash](https://github.com/BrianSharpe/swash) — Font shaping
- [nucleo-matcher](https://github.com/helix-editor/nucleo) — Fuzzy matching

## License

MIT
