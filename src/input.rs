//! InputManager: keyboard shortcut ownership.
//!
//! Owns the keymap, the action-mode trigger and all transient key states
//! (action mode, pending tab-move, resize gestures). `Surface` composes
//! this component and asks it whether a keystroke matches, instead of
//! storing the map itself.

use std::collections::HashMap;

use gpui::KeyDownEvent;

use crate::{config::KeyBinding, BuiltinAction, KeyAction};

pub struct InputManager {
    pub keymap: HashMap<KeyBinding, KeyAction>,
    pub action: KeyBinding,
    pub action_mode: bool,
    pub pending_move_to_tab: bool,
    pub resize_mode_held: bool,
    pub resize_mode_used: bool,
}

impl InputManager {
    pub fn new() -> Self {
        Self {
            keymap: default_keymap(),
            action: KeyBinding::control("b"),
            action_mode: false,
            pending_move_to_tab: false,
            resize_mode_held: false,
            resize_mode_used: false,
        }
    }

    pub fn reset_keymap(&mut self) {
        self.keymap = default_keymap();
    }

    pub fn match_action(&self, event: &KeyDownEvent) -> Option<KeyAction> {
        self.keymap
            .iter()
            .find_map(|(binding, action)| binding.matches_event(event).then(|| action.clone()))
    }

    pub fn is_action_trigger(action: &KeyAction) -> bool {
        matches!(action, KeyAction::Builtin(BuiltinAction::Action))
    }

    /// Enter action mode (the action key itself was pressed).
    pub fn enter_action_mode(&mut self) {
        self.resize_mode_held = true;
        self.resize_mode_used = false;
        self.action_mode = true;
    }

    /// Leave action mode after a non-trigger key consumed it.
    pub fn consume_action_mode(&mut self) {
        self.action_mode = false;
    }

    pub fn cancel_action_mode(&mut self) {
        self.action_mode = false;
    }

    pub fn begin_move_to_tab(&mut self) {
        self.pending_move_to_tab = true;
        self.action_mode = true;
    }

    pub fn take_move_to_tab(&mut self) -> bool {
        if self.pending_move_to_tab {
            self.pending_move_to_tab = false;
            self.action_mode = false;
            true
        } else {
            false
        }
    }

    pub fn cancel_move_to_tab(&mut self) {
        self.pending_move_to_tab = false;
        self.action_mode = false;
    }

    pub fn handle_action_key_up(&mut self, key: &str) -> bool {
        if key.to_lowercase() == self.action.key {
            self.resize_mode_held = false;
            if self.resize_mode_used {
                self.action_mode = false;
            }
            self.resize_mode_used = false;
            true
        } else {
            false
        }
    }
}

impl Default for InputManager {
    fn default() -> Self {
        Self::new()
    }
}

pub fn default_keymap() -> HashMap<KeyBinding, KeyAction> {
    use BuiltinAction as B;
    use gpui::Modifiers;
    let mut keymap = HashMap::new();
    keymap.insert(KeyBinding::control("b"), KeyAction::Builtin(B::Action));
    keymap.insert(
        KeyBinding::new(
            Modifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
            "f",
        ),
        KeyAction::Builtin(B::Palette),
    );
    keymap.insert(
        KeyBinding::new(
            Modifiers {
                control: true,
                shift: true,
                ..Default::default()
            },
            "s",
        ),
        KeyAction::Builtin(B::Sessions),
    );
    let action_keys: &[(&str, BuiltinAction)] = &[
        ("1", B::Tab(0)),
        ("2", B::Tab(1)),
        ("3", B::Tab(2)),
        ("4", B::Tab(3)),
        ("5", B::Tab(4)),
        ("6", B::Tab(5)),
        ("7", B::Tab(6)),
        ("8", B::Tab(7)),
        ("9", B::Tab(8)),
        ("k", B::NextTab),
        ("j", B::PrevTab),
        ("w", B::NextPane),
        ("v", B::SplitVertical),
        ("h", B::SplitHorizontal),
        ("t", B::ToggleDecorations),
        ("s", B::ToggleStatusBar),
        ("p", B::Palette),
        ("semicolon", B::StatusPrompt),
        ("l", B::LuaPrompt),
        ("m", B::MoveToTab),
        ("d", B::DetachSession),
        ("r", B::RenameSession),
        ("a", B::Detached),
    ];
    for (key, action) in action_keys {
        keymap.insert(
            KeyBinding::new(Modifiers::default(), *key),
            KeyAction::Builtin(*action),
        );
    }
    keymap
}

/// Digit value for `1`–`9` keystrokes (tab selection).
pub fn digit_index(event: &KeyDownEvent) -> Option<usize> {
    match event.keystroke.key.as_str() {
        "1" => Some(0),
        "2" => Some(1),
        "3" => Some(2),
        "4" => Some(3),
        "5" => Some(4),
        "6" => Some(5),
        "7" => Some(6),
        "8" => Some(7),
        "9" => Some(8),
        _ => event
            .keystroke
            .key_char
            .as_deref()
            .and_then(|ch| match ch {
                "1" => Some(0),
                "2" => Some(1),
                "3" => Some(2),
                "4" => Some(3),
                "5" => Some(4),
                "6" => Some(5),
                "7" => Some(6),
                "8" => Some(7),
                "9" => Some(8),
                _ => None,
            }),
    }
}

/// Whether the keystroke arrived without modifiers (bare action-mode key).
pub fn bare_key(event: &KeyDownEvent) -> bool {
    let mods = &event.keystroke.modifiers;
    !mods.control && !mods.alt && !mods.shift && !mods.platform
}
