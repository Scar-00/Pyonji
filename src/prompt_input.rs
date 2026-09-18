//! Single-line command and rename prompts built on gpui-component `Input`.
//!
//! Replaces the crude `StatusBarState` buffer/caret rendering (`before`,
//! `after`, manual cursor) with the shared editing engine (`InputState` +
//! `gpui_component::input::Input`). The engine owns text, cursor, selection,
//! IME, clipboard and key handling; these wrappers only add a badge +
//! placeholder, flatten newlines (single-line strips them), and emit
//! host-owned submit/cancel events.
//!
//! History, completion and mode switching stay in the `StatusBar` entity
//! (like `LuaSingleLineInput` leaves history in the bar); `Surface`
//! subscribes to `Submit`/`Cancel` to execute and refocus the terminal.

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, IntoElement, MouseButton, Render,
    SharedString, Subscription, Window, div, prelude::*,
};
// Engine (state/events) comes from the base crate; only the rendered `Input`
// view is still the styled component one (borderless/chromeless here, framed
// by `prompt_frame`).
use gpui_base::input::{InputEvent, InputState};
use gpui_component::input::Input;

/// Events emitted by [`CommandPrompt`].
#[derive(Debug, Clone)]
pub enum CommandPromptEvent {
    /// Text changed (for the host to recompute the completion menu).
    Change,
    /// Enter pressed with the trimmed line (possibly empty; host closes).
    Submit(SharedString),
    /// Escape pressed; host closes and refocuses.
    Cancel,
}

/// Events emitted by [`RenamePrompt`].
#[derive(Debug, Clone)]
pub enum RenamePromptEvent {
    Change,
    Submit(SharedString),
    Cancel,
}

fn prompt_frame(focused: bool) -> gpui::Div {
    div()
        .w_full()
        .rounded(crate::theme::radius::SM)
        .bg(crate::theme::role::input_bg())
        .border_1()
        .border_color(if focused {
            crate::theme::role::input_border_focus()
        } else {
            crate::theme::role::input_border()
        })
}

fn prompt_badge(label: &str) -> impl IntoElement {
    div()
        .px(crate::theme::space::_1)
        .rounded(crate::theme::radius::SM)
        .bg(crate::theme::role::accent())
        .text_color(crate::theme::role::accent_fg())
        .child(label.to_string())
}

/// Single-line command prompt (`:command args…`).
pub struct CommandPrompt {
    state: Entity<InputState>,
    _subscription: Subscription,
}

impl CommandPrompt {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Command - Enter to run")
        });
        let subscription = cx.subscribe_in(&state, window, Self::on_input_event);
        Self {
            state,
            _subscription: subscription,
        }
    }

    fn on_input_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => cx.emit(CommandPromptEvent::Change),
            InputEvent::PressEnter { .. } => {
                // `set_value` below does not re-emit, so reading here is safe.
                // Trim like the old buffer path; empty just closes.
                let value = self.state.read(cx).value().trim().to_string();
                cx.emit(CommandPromptEvent::Submit(value.into()));
            }
            _ => {}
        }
    }

    pub fn state(&self) -> &Entity<InputState> {
        &self.state
    }

    pub fn value(&self, cx: &App) -> SharedString {
        self.state.read(cx).value()
    }

    pub fn set_value(&self, value: &str, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_value(value, window, cx);
        });
    }

    pub fn insert_text(&self, text: &str, window: &mut Window, cx: &mut App) {
        if text.is_empty() {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.insert(text, window, cx);
        });
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.state.read(cx).focus_handle(cx).is_focused(window)
    }

    /// Emit a cancel (called by the host's Escape action).
    pub fn emit_cancel(&self, cx: &mut Context<Self>) {
        cx.emit(CommandPromptEvent::Cancel);
    }
}

impl EventEmitter<CommandPromptEvent> for CommandPrompt {}

impl Focusable for CommandPrompt {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }
}

impl Render for CommandPrompt {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.is_focused(window, cx);
        prompt_frame(focused)
            .flex()
            .flex_row()
            .items_center()
            .gap(crate::theme::space::_1)
            .px(crate::theme::space::_1)
            .text_color(crate::theme::role::text())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.focus(window, cx);
                cx.notify();
            }))
            .child(prompt_badge(":"))
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&self.state).appearance(false).bordered(false).text_sm()),
            )
    }
}

/// Single-line rename prompt (prefilled session title, no history/menu).
pub struct RenamePrompt {
    state: Entity<InputState>,
    _subscription: Subscription,
}

impl RenamePrompt {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Session name — Enter to rename")
        });
        let subscription = cx.subscribe_in(&state, window, Self::on_input_event);
        Self {
            state,
            _subscription: subscription,
        }
    }

    fn on_input_event(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => cx.emit(RenamePromptEvent::Change),
            InputEvent::PressEnter { .. } => {
                let value = self.state.read(cx).value().trim().to_string();
                cx.emit(RenamePromptEvent::Submit(value.into()));
            }
            _ => {}
        }
    }

    pub fn state(&self) -> &Entity<InputState> {
        &self.state
    }

    pub fn value(&self, cx: &App) -> SharedString {
        self.state.read(cx).value()
    }

    pub fn set_value(&self, value: &str, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_value(value, window, cx);
        });
    }

    /// Replace the value and select all, so typing replaces the prefilled name.
    pub fn set_value_select_all(&self, value: &str, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| {
            state.set_value(value, window, cx);
            state.select_all(window, cx);
        });
    }

    pub fn insert_text(&self, text: &str, window: &mut Window, cx: &mut App) {
        if text.is_empty() {
            return;
        }
        self.state.update(cx, |state, cx| {
            state.insert(text, window, cx);
        });
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    pub fn is_focused(&self, window: &Window, cx: &App) -> bool {
        self.state.read(cx).focus_handle(cx).is_focused(window)
    }

    pub fn emit_cancel(&self, cx: &mut Context<Self>) {
        cx.emit(RenamePromptEvent::Cancel);
    }
}

impl EventEmitter<RenamePromptEvent> for RenamePrompt {}

impl Focusable for RenamePrompt {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.read(cx).focus_handle(cx)
    }
}

impl Render for RenamePrompt {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.is_focused(window, cx);
        prompt_frame(focused)
            .flex()
            .flex_row()
            .items_center()
            .gap(crate::theme::space::_1)
            .px(crate::theme::space::_1)
            .text_color(crate::theme::role::text())
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                this.focus(window, cx);
                cx.notify();
            }))
            .child(prompt_badge("R:"))
            .child(
                div()
                    .flex_1()
                    .child(Input::new(&self.state).appearance(false).bordered(false).text_sm()),
            )
    }
}

// Key actions for the prompts. `CommandUp`/`CommandDown` are bound under the
// shared `Input` context (later registration runs first, like `LuaTabAccept`)
// because single-line `MoveUp`/`MoveDown` consume the keys without
// propagating. The rest are bound under the prompt wrappers' own contexts
// (`status-command`, `status-rename`); single-line `Tab`/`Escape` propagate.
gpui::actions!(status_command, [CommandCancel, CommandTabAccept]);
gpui::actions!(status_rename, [RenameCancel]);
gpui::actions!([CommandUp, CommandDown]);

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    async fn command_submit_trims(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        cx.update(gpui_component::init);
        let submitted: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
        let (view, vcx) = cx.add_window_view(CommandPrompt::new);
        let _sub = vcx.update(|_, cx| {
            cx.subscribe(&view, {
                let submitted = submitted.clone();
                move |_, event: &CommandPromptEvent, _| {
                    if let CommandPromptEvent::Submit(value) = event {
                        submitted.borrow_mut().push(value.to_string());
                    }
                }
            })
        });
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
            view.update(cx, |view, cx| {
                view.set_value("  switch 1  ", window, cx);
                view.focus(window, cx);
            });
        });
        vcx.run_until_parked();
        vcx.dispatch_action(gpui_base::input::Enter {
            secondary: false,
            shift: false,
        });
        vcx.run_until_parked();
        assert_eq!(submitted.borrow().as_slice(), ["switch 1"]);
        // Single-line strips newlines without host help.
        vcx.update(|_, cx| {
            assert_eq!(view.read(cx).value(cx).as_ref(), "  switch 1  ");
        });
    }

    #[gpui::test]
    async fn rename_prefill_selects_all(cx: &mut gpui::TestAppContext) {
        cx.update(gpui_component::init);
        let (view, vcx) = cx.add_window_view(RenamePrompt::new);
        vcx.update(|window, cx| {
            let _ = window.draw(cx);
            view.update(cx, |view, cx| {
                view.set_value_select_all("old-name", window, cx);
                view.focus(window, cx);
            });
        });
        vcx.run_until_parked();
        vcx.update(|_, cx| {
            let prompt = view.read(cx);
            assert_eq!(prompt.value(cx).as_ref(), "old-name");
            let selected = prompt
                .state()
                .read(cx)
                .selected_value()
                .to_string();
            assert_eq!(selected, "old-name");
        });
    }
}
