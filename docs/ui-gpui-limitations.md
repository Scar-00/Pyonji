# UI work blocked or worked around because of gpui / gpui-component

Things the UI should do but can't cleanly today, with the workaround in place
(if any) and what would let us do it properly.

## Dialogs can't size themselves to their content

- **Want:** the palette and sessions dialogs grow and shrink with their list,
  up to a cap, without the view having to know its own pixel height.
- **Blocker:** `gpui_component::dialog::Dialog` wraps its children in a
  `v_flex().size_full().overflow_y_scrollbar()`. A child that is `size_full`
  inside that scroll area grows to its natural height instead of the dialog's,
  so a fixed footer gets pushed out of view. And the dialog's height is chosen
  in the dialog builder, which runs while `Pyonji` is being updated, so the
  builder can't read the app to measure the list (`cannot read Pyonji while it
  is already being updated`).
- **Workaround:** `src/ui/overlay/mod.rs` pins the content to the dialog's
  height minus its border. `PaletteView` and `SessionsView` estimate their
  height from their row constants during their own render, cache it
  (`fit_height`), and call `window.refresh()` once when it changes.
- **Proper fix:** a dialog option that lays out children without the scroll
  wrapper (or a `content_fit` / `max_h` mode that measures its child), so the
  views can be `flex_1` and the dialog uses intrinsic height.

## Scroll-aware overflow cues

- **Want:** a fade at the bottom of the release notes only while there is more
  to scroll.
- **Blocker:** `ScrollHandle` has no change event, so a view isn't re-rendered
  when its scroll position changes (scrollbar drags, keyboard scrolling).
- **Workaround:** the directory picker reads `offset()` / `max_offset()` during
  render and re-renders on scroll-wheel events. Release notes use an always
  visible scrollbar in a separate overlay, preserving main's scrolling fix.
- **Proper fix:** an `on_scroll` callback (or a notify on offset change) for
  scroll handles.

## Split divider hover feedback

- **Want:** a resize cursor and a highlighted divider while the pointer hovers
  a split divider.
- **Blocker:** the dividers are drawn by the wgpu renderer into one surface, not
  as gpui elements, so there is no per-divider element to put `.cursor(...)` or
  `.hover(...)` on.
- **Workaround:** none yet. Dividers next to the focused pane use
  `split_divider_active`, and inactive panes are dimmed.
- **Proper fix:** lay thin, transparent gpui hit-target elements over the
  divider positions in `ui/terminal.rs` that set `CursorStyle::ResizeLeftRight`
  / `ResizeUpDown` and report hover back to the renderer.

## Dialog backdrop doesn't cover the terminal

- **Want:** the dialog's own backdrop (`Theme::overlay`) dims everything behind
  an open dialog.
- **Blocker:** the backdrop never shows over the terminal grid, even at the
  library's default alpha. The grid is a wgpu render-target surface, and the
  backdrop doesn't composite over it.
- **Workaround:** `ui/terminal.rs` checks `has_active_dialog` on each render and
  passes `Chrome::dim` to the renderer, which darkens the grid by
  `overlay_backdrop`'s alpha.
- **Proper fix:** let gpui elements composite over custom GPU surfaces (or
  give the surface element a tint/opacity), so the stock backdrop works.

## Component theme overrides get reset

- **Want:** set a few `gpui_component::Theme` fields (selection, primary buttons, mono font) once
  at startup.
- **Blocker:** the theme registry reloads asynchronously after startup and
  calls `Theme::change`, which rebuilds the theme and drops direct field edits.

  Primary buttons read `tokens.button_primary`, not `Theme::primary`.
- **Workaround:** `Theme::keep_component_overrides` (`ui/theme.rs`) re-applies the
  overrides from an `observe_global` hook. The shared theme overrides set the primary button tokens to the accent colour.
- **Proper fix:** a supported way to register app-level theme overrides that
  survive `Theme::change` (or a custom theme in the registry).

## Uniform lists can't mix row heights

- **Want:** compact section headings in the palette (about 28px) between 38px
  command rows.
- **Blocker:** `uniform_list` measures its first item and gives every item
  that height, and the first item is a heading.
- **Workaround:** headings are as tall as rows (`SECTION = ROW` in
  `palette.rs`) and sit their label at the bottom of the slot.
- **Proper fix:** a virtualised list that takes a per-item height (gpui's
  `list` with measured items, or a `uniform_list` variant with an item-height
  callback).
