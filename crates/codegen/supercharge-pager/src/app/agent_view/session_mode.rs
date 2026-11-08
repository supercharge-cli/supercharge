//! Session modes an agent publishes, kept next to the plan flags every plan surface already reads.

use agent_client_protocol as acp;
use supercharge_tools::types::SessionMode;

use crate::app::agent_view::AgentView;

/// One Shift+Tab stop. Session modes keep the agent's id. The last two stops are permissions.
pub(crate) enum ShiftTabStop {
    Mode { id: String, name: String },
    AlwaysApprove,
    AutoReview,
}

impl AgentView {
    /// Stores the agent's offered modes. A published list drops permission staged by the
    /// pre-session Grok ring, which has no arm here and would otherwise stick on the prompt row.
    pub(crate) fn apply_session_modes(&mut self, modes: Option<acp::SessionModeState>) -> bool {
        let Some(modes) = modes else {
            return false;
        };
        self.session_mode = SessionMode::from_id(&modes.current_mode_id.0);
        self.session_mode_pending = None;
        self.plan_mode_active = self.session_mode.is_plan();
        self.published_mode_id = Some(modes.current_mode_id.0.to_string());
        self.available_modes = modes.available_modes;
        self.session.yolo_mode = false;
        self.session.auto_mode = false;
        self.deferred_permission_mode = None;
        true
    }

    /// Optimistic pick, else the confirmed mode. Plan exits through the plan flags must not
    /// still read as plan here.
    pub(crate) fn effective_session_mode(&self) -> SessionMode {
        if self.plan_mode_pending.unwrap_or(self.plan_mode_active) {
            return SessionMode::Plan;
        }
        let picked = self
            .session_mode_pending
            .as_ref()
            .unwrap_or(&self.session_mode);
        match picked {
            SessionMode::Ask => SessionMode::Ask,
            SessionMode::Default | SessionMode::Plan => SessionMode::Default,
        }
    }

    /// Prompt-row / peek flag for a published non-plan mode. Agent shows none.
    pub(crate) fn published_mode_label(&self) -> Option<&'static str> {
        match self.effective_session_mode() {
            SessionMode::Ask => Some(SessionMode::Ask.as_id()),
            SessionMode::Default | SessionMode::Plan => None,
        }
    }

    /// Same flag the prompt row uses when it is not in commenting / plan-approval chrome.
    pub(crate) fn prompt_row_mode_label(&self) -> Option<&'static str> {
        if self.plan_mode_pending.unwrap_or(self.plan_mode_active) {
            Some("plan")
        } else {
            self.published_mode_label()
        }
    }

    /// Next Shift+Tab stop. Custom agents stay in the ring. Always-Approve and Auto-Review follow the published modes.
    pub(crate) fn next_shift_tab_stop(&self) -> Option<ShiftTabStop> {
        if self.available_modes.is_empty() {
            return None;
        }
        let custom = supercharge_agent::custom_agent::modes_for_cwd(&self.session.cwd);
        let mut stops: Vec<ShiftTabStop> = self
            .available_modes
            .iter()
            .filter(|mode| {
                let id = mode.id.0.as_ref();
                id == "default" || id == "plan" || id == "ask" || custom.iter().any(|item| item.id == id)
            })
            .map(|mode| ShiftTabStop::Mode {
                id: mode.id.0.to_string(),
                name: mode.name.clone(),
            })
            .collect();
        for mode in custom {
            if stops.iter().any(|stop| matches!(stop, ShiftTabStop::Mode { id, .. } if id == &mode.id)) {
                continue;
            }
            stops.push(ShiftTabStop::Mode {
                id: mode.id,
                name: mode.name,
            });
        }
        stops.push(ShiftTabStop::AlwaysApprove);
        stops.push(ShiftTabStop::AutoReview);
        let current = if self.session.is_yolo() {
            "always-approve".to_string()
        } else if self.session.is_auto() {
            "auto".to_string()
        } else if self.plan_mode_pending.unwrap_or(self.plan_mode_active) {
            "plan".to_string()
        } else {
            self.published_mode_id
                .clone()
                .unwrap_or_else(|| self.session_mode.as_id().to_string())
        };
        let position = stops.iter().position(|stop| {
            let id = match stop {
                ShiftTabStop::Mode { id, .. } => id.as_str(),
                ShiftTabStop::AlwaysApprove => "always-approve",
                ShiftTabStop::AutoReview => "auto",
            };
            id == current
        });
        let index = position.map_or(0, |found| (found + 1) % stops.len());
        stops.into_iter().nth(index)
    }

    /// Optimistic Shift+Tab pick, pending the agent's `CurrentModeUpdate`.
    pub(crate) fn pick_published_mode(&mut self, mode_id: &str) {
        let mode = SessionMode::from_id(mode_id);
        self.plan_mode_pending = Some(mode.is_plan());
        self.session_mode_pending = Some(mode);
        self.published_mode_id = Some(mode_id.to_string());
        self.session.yolo_mode = false;
        self.session.auto_mode = false;
    }
}
