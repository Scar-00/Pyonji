//! Workspace: session/tab data model.
//!
//! Owns everything about *what* is open — sessions, tabs, splits,
//! detached sessions, SSH inventory and the default working directory —
//! without knowing how it is rendered (see `ui::terminal`) or how input
//! is dispatched (see `input`). `Surface` composes this component and
//! delegates all tab/session operations to it.

use std::path::{Path, PathBuf};

use crate::{
    pty::SshConnection,
    terminal::{PaneGeometry, SessionId, SessionManager, SplitDirection, Tab},
};

pub const TAB_COUNT: usize = 9;

pub struct Workspace {
    pub session_manager: SessionManager,
    pub tabs: [Option<Tab>; TAB_COUNT],
    pub current_tab: usize,
    pub detached_sessions: Vec<SessionId>,
    pub ssh_sessions: Vec<SshConnection>,
    pub default_cwd: Option<PathBuf>,
    /// Set when scrollback was reset by an interaction; cleared by the
    /// caller after notifying. Keeps scroll state inside the component.
    pub wheel_remainder: f32,
}

impl Workspace {
    pub fn new(event_tx: async_channel::Sender<crate::pty::Event>) -> Self {
        Self {
            session_manager: SessionManager::new(event_tx),
            tabs: std::array::from_fn(|_| None),
            current_tab: 0,
            detached_sessions: Vec::new(),
            ssh_sessions: Vec::new(),
            default_cwd: None,
            wheel_remainder: 0.0,
        }
    }

    /// Boot the first session after config load. CLI path wins over
    /// `default_cwd`, matching the old event-loop startup order.
    pub fn ensure_initial_session(&mut self, initial_path: Option<&Path>) {
        if self.tabs.iter().any(|tab| tab.is_some()) {
            return;
        }
        let cwd = initial_path.or(self.default_cwd.as_deref());
        if let Ok(id) = self.session_manager.create_session(20, 80, cwd) {
            self.tabs[0] = Some(Tab::new(id));
        }
    }

    pub fn next_tab_index(&self) -> usize {
        (self.current_tab + 1) % self.tabs.len()
    }

    pub fn previous_tab_index(&self) -> usize {
        if self.current_tab == 0 {
            self.tabs.len() - 1
        } else {
            self.current_tab - 1
        }
    }

    fn switch_to_previous_live_tab_or_stay(&mut self, closed_tab: usize, cols: u16, rows: u16) {
        if let Some(tab) = (0..self.tabs.len())
            .map(|offset| (closed_tab + self.tabs.len() - 1 - offset) % self.tabs.len())
            .find(|&tab| self.tabs[tab].is_some())
        {
            self.current_tab = tab;
            self.wheel_remainder = 0.0;
            self.resize_tab(cols, rows);
            return;
        }

        self.current_tab = closed_tab.min(self.tabs.len().saturating_sub(1));
    }

    pub fn take_wheel_steps(&mut self, delta_lines: f32) -> i32 {
        let total = self.wheel_remainder + delta_lines;
        let whole = if total > 0.0 {
            total.floor() as i32
        } else if total < 0.0 {
            total.ceil() as i32
        } else {
            0
        };
        self.wheel_remainder = total - whole as f32;
        whole
    }

    pub fn reset_wheel(&mut self) {
        self.wheel_remainder = 0.0;
    }

    pub fn active_session(&self) -> Option<SessionId> {
        self.tabs[self.current_tab]
            .as_ref()
            .and_then(Tab::active_session)
    }

    pub fn tab_layouts(&self, cols: u16, rows: u16) -> Vec<(SessionId, PaneGeometry)> {
        self.tabs[self.current_tab]
            .as_ref()
            .map(|tab| {
                tab.layout(PaneGeometry {
                    x: 0,
                    y: 0,
                    cols,
                    rows,
                })
            })
            .unwrap_or_default()
    }

    pub fn tab_dividers(
        &self,
        cols: u16,
        rows: u16,
    ) -> Vec<crate::terminal::Divider> {
        self.tabs[self.current_tab]
            .as_ref()
            .map(|tab| {
                tab.dividers(PaneGeometry {
                    x: 0,
                    y: 0,
                    cols,
                    rows,
                })
            })
            .unwrap_or_default()
    }

    pub fn resize_tab(&mut self, cols: u16, rows: u16) {
        let current = self.current_tab;
        self.resize_tab_at(current, cols, rows);
    }

    pub fn resize_tab_at(&mut self, index: usize, cols: u16, rows: u16) {
        let Some(tab) = self.tabs[index].as_ref() else {
            return;
        };
        for (session_id, geometry) in tab.layout(PaneGeometry {
            x: 0,
            y: 0,
            cols,
            rows,
        }) {
            self.session_manager.resize_session(
                session_id,
                geometry.rows.max(1),
                geometry.cols.max(1),
            );
        }
    }

    pub fn set_active_session(&mut self, session_id: SessionId) {
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if tab.set_active_session(session_id) {
            self.wheel_remainder = 0.0;
        }
    }

    pub fn focus_next_pane(&mut self) -> Option<SessionId> {
        let tab = self.tabs[self.current_tab].as_mut()?;
        let next = tab.focus_next()?;
        self.wheel_remainder = 0.0;
        Some(next)
    }

    pub fn resize_active_pane(
        &mut self,
        direction: SplitDirection,
        delta_first: i16,
        cols: u16,
        rows: u16,
    ) {
        let area = PaneGeometry {
            x: 0,
            y: 0,
            cols,
            rows,
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return;
        };
        if !tab.resize_active_split(area, direction, delta_first) {
            return;
        }
        self.wheel_remainder = 0.0;
        self.resize_tab(cols, rows);
    }

    pub fn resize_split_by_position(
        &mut self,
        path: &[crate::terminal::PanePathStep],
        direction: SplitDirection,
        position: f32,
        cols: u16,
        rows: u16,
    ) -> bool {
        let area = PaneGeometry {
            x: 0,
            y: 0,
            cols,
            rows,
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return false;
        };
        if !tab.resize_split_by_position(area, path, direction, position) {
            return false;
        }
        self.wheel_remainder = 0.0;
        self.resize_tab(cols, rows);
        true
    }

    pub fn split_current_tab(
        &mut self,
        direction: SplitDirection,
        cols: u16,
        rows: u16,
    ) -> Option<SessionId> {
        let active_session = self.active_session()?;
        let (_, geometry) = self
            .tab_layouts(cols, rows)
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)?;

        let can_split = match direction {
            SplitDirection::Horizontal => geometry.rows >= 2,
            SplitDirection::Vertical => geometry.cols >= 2,
        };
        if !can_split {
            return None;
        }

        let new_rows = match direction {
            SplitDirection::Horizontal => geometry.rows / 2,
            SplitDirection::Vertical => geometry.rows,
        }
        .max(1);
        let new_cols = match direction {
            SplitDirection::Horizontal => geometry.cols,
            SplitDirection::Vertical => geometry.cols / 2,
        }
        .max(1);

        let session_id = match self
            .session_manager
            .create_session(new_rows, new_cols, self.default_cwd.as_deref())
        {
            Ok(session_id) => session_id,
            Err(error) => {
                tracing::error!(error = ?error, "failed to split session");
                return None;
            }
        };
        let tab = self.tabs[self.current_tab].as_mut()?;

        if !tab.split_active(direction, session_id) {
            return None;
        }

        self.wheel_remainder = 0.0;
        self.resize_tab(cols, rows);
        Some(session_id)
    }

    pub fn switch_tab(&mut self, tab: usize, cols: u16, rows: u16) -> bool {
        if tab >= self.tabs.len() {
            return false;
        }
        if self.tabs[tab].is_none() {
            let id = match self.session_manager.create_session(
                rows.max(1),
                cols.max(1),
                self.default_cwd.as_deref(),
            ) {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create tab session");
                    return false;
                }
            };
            self.tabs[tab] = Some(Tab::new(id));
        }

        self.current_tab = tab;
        self.wheel_remainder = 0.0;
        self.resize_tab(cols, rows);
        true
    }

    pub fn move_session_to_tab(
        &mut self,
        session: SessionId,
        target: usize,
        cols: u16,
        rows: u16,
    ) -> bool {
        if target >= self.tabs.len() || target == self.current_tab {
            return false;
        }
        let mut source = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if index == target {
                continue;
            }
            if let Some(tab) = tab.as_mut()
                && tab.remove_session(session)
            {
                source = Some(index);
                break;
            }
        }
        let Some(source) = source else {
            return false;
        };
        self.detached_sessions.retain(|id| *id != session);

        match self.tabs[target].as_mut() {
            Some(tab) => {
                if !tab.split_active(SplitDirection::Vertical, session) {
                    self.detached_sessions.push(session);
                }
            }
            None => self.tabs[target] = Some(Tab::new(session)),
        }

        self.current_tab = target;
        self.wheel_remainder = 0.0;
        if self.tabs[source]
            .as_ref()
            .is_some_and(|tab| !tab.is_empty())
        {
            self.resize_tab_at(source, cols, rows);
        }
        self.resize_tab(cols, rows);
        true
    }

    pub fn detach_active_session(&mut self, cols: u16, rows: u16) -> bool {
        let Some(active_session) = self.active_session() else {
            return false;
        };
        let Some(tab) = self.tabs[self.current_tab].as_mut() else {
            return false;
        };
        if !tab.remove_session(active_session) {
            return false;
        }
        self.detached_sessions.push(active_session);
        if self.tabs[self.current_tab]
            .as_ref()
            .is_some_and(Tab::is_empty)
        {
            self.tabs[self.current_tab] = None;
            self.switch_to_previous_live_tab_or_stay(self.current_tab, cols, rows);
        } else {
            self.resize_tab(cols, rows);
        }
        true
    }

    pub fn close_session(&mut self, session: SessionId, cols: u16, rows: u16) -> bool {
        if self.session_manager.session(session).is_none() {
            return false;
        }
        self.session_manager.remove_session(session);
        self.detached_sessions.retain(|detached| *detached != session);

        let mut removed_current_tab = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let Some(tab_state) = tab.as_mut() else {
                continue;
            };
            if !tab_state.remove_session(session) {
                continue;
            }
            if tab_state.is_empty() {
                *tab = None;
                removed_current_tab |= index == self.current_tab;
            }
        }

        if removed_current_tab {
            self.switch_to_previous_live_tab_or_stay(self.current_tab, cols, rows);
        } else {
            self.resize_tab(cols, rows);
        }
        true
    }

    pub fn reattach_session(
        &mut self,
        session: SessionId,
        target: usize,
        cols: u16,
        rows: u16,
    ) -> bool {
        if target >= self.tabs.len() || !self.detached_sessions.contains(&session) {
            return false;
        }
        self.detached_sessions.retain(|id| *id != session);
        match self.tabs[target].as_mut() {
            Some(tab) => {
                if !tab.split_active(SplitDirection::Vertical, session) {
                    self.detached_sessions.push(session);
                    return false;
                }
            }
            None => self.tabs[target] = Some(Tab::new(session)),
        }
        self.current_tab = target;
        self.wheel_remainder = 0.0;
        self.resize_tab(cols, rows);
        true
    }

    pub fn live_detached_sessions(&self) -> Vec<SessionId> {
        self.detached_sessions
            .iter()
            .copied()
            .filter(|id| self.session_manager.session(*id).is_some())
            .collect()
    }

    pub fn rename_session(&mut self, session: SessionId, name: &str) -> bool {
        let Some(session) = self.session_manager.session_mut(session) else {
            return false;
        };
        session.rename(name.to_string());
        true
    }

    pub fn rename_active(&mut self, name: &str) {
        let Some(session) = self.active_session() else {
            return;
        };
        self.rename_session(session, name);
    }

    pub fn tab_program_name(&self, tab_index: usize) -> &str {
        self.tabs[tab_index]
            .as_ref()
            .and_then(Tab::active_session)
            .and_then(|session_id| self.session_manager.session(session_id))
            .map_or("shell", |session| session.title())
    }

    pub fn status_tabs(&self) -> Vec<(String, bool)> {
        self.tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| {
                tab.as_ref().map(|_| {
                    (
                        format!("[{}] {}", index + 1, self.tab_program_name(index)),
                        index == self.current_tab,
                    )
                })
            })
            .collect()
    }

    pub fn open_session_in_dir(&mut self, path: &Path, cols: u16, rows: u16) {
        let next_free = self.tabs.iter().position(|tab| tab.is_none());
        if let Some(free) = next_free {
            let id = match self
                .session_manager
                .create_session(rows.max(1), cols.max(1), Some(path))
            {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create session in dir");
                    return;
                }
            };
            self.tabs[free] = Some(Tab::new(id));
            self.current_tab = free;
            self.wheel_remainder = 0.0;
            self.resize_tab(cols, rows);
            return;
        }
        if self.tabs[self.current_tab].is_some() {
            let id = match self
                .session_manager
                .create_session(rows.max(1), cols.max(1), Some(path))
            {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create session in dir");
                    return;
                }
            };
            let Some(tab) = &mut self.tabs[self.current_tab] else {
                return;
            };
            tab.split_active(SplitDirection::Horizontal, id);
        }
    }

    pub fn create_remote_session(&mut self, session: &SshConnection, cols: u16, rows: u16) {
        let next_free = self.tabs.iter().position(|tab| tab.is_none());
        if let Some(free) = next_free {
            let id = match self.session_manager.create_remote_session(
                rows.max(1),
                cols.max(1),
                session,
            ) {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create tab session");
                    return;
                }
            };
            self.tabs[free] = Some(Tab::new(id));
            self.current_tab = free;
            self.wheel_remainder = 0.0;
            self.resize_tab(cols, rows);
            return;
        }
        if self.tabs[self.current_tab].is_some() {
            let id = match self.session_manager.create_remote_session(
                rows.max(1),
                cols.max(1),
                session,
            ) {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create tab session");
                    return;
                }
            };

            let Some(tab) = &mut self.tabs[self.current_tab] else {
                return;
            };

            tab.split_active(SplitDirection::Horizontal, id);
        }
    }

    /// Create a session for the Lua `create_session` API, placing it on
    /// `target` tab (or current) anchored at `parent` when given.
    pub fn create_session_placed(
        &mut self,
        dir: Option<&Path>,
        tab: Option<usize>,
        direction: SplitDirection,
        parent: Option<SessionId>,
        cols: u16,
        rows: u16,
    ) -> anyhow::Result<SessionId> {
        let session = self.session_manager.create_session(
            rows.max(1),
            cols.max(1),
            dir.or(self.default_cwd.as_deref()),
        )?;
        let target = tab
            .map(|tab| tab.min(self.tabs.len().saturating_sub(1)))
            .unwrap_or(self.current_tab);
        let tab_state = &mut self.tabs[target];
        let placed = if let Some(tab_state) = tab_state {
            let anchor = parent
                .filter(|anchor| tab_state.sessions().contains(anchor))
                .or_else(|| tab_state.active_session());
            match anchor {
                Some(anchor) => tab_state.split_on(anchor, direction, session),
                None => false,
            }
        } else {
            *tab_state = Some(Tab::new(session));
            true
        };
        if !placed {
            self.detached_sessions.push(session);
        }
        self.wheel_remainder = 0.0;
        self.resize_tab(cols, rows);
        Ok(session)
    }

    pub fn pane_at(
        &self,
        col: u16,
        row: u16,
        cols: u16,
        rows: u16,
    ) -> Option<(SessionId, u16, u16)> {
        for (session_id, geometry) in self.tab_layouts(cols, rows) {
            if !geometry.contains_global_cell(col, row) {
                continue;
            }
            let (col, row) = geometry.local_cell(col, row);
            return Some((session_id, col, row));
        }
        None
    }
}
