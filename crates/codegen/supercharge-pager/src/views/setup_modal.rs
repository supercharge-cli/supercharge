//! `/setup` wizard: base URL, API key, fetched model list.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;
use unicode_width::UnicodeWidthStr;
use supercharge_shell::agent::provider_setup::{
    CUSTOM_MODEL_ID, CUSTOM_PROVIDER_ID, ProviderModel, ProviderSetup, SetupProviderProfile,
    SetupWireFormat, find_setup_provider, normalize_provider_base_url, setup_provider_profiles,
};

use crate::input::line_editor::{LineEditOutcome, LineEditor};
use crate::theme::Theme;
use crate::views::modal_window::{
    self as mw, ModalSizing, ModalWindowConfig, ModalWindowOutcome, ModalWindowState, Shortcut,
};

pub const MODAL_TITLE: &str = "Provider setup";
pub const IMAGE_MODAL_TITLE: &str = "Image provider setup";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupKind {
    Chat,
    Image,
}

#[derive(Debug)]
pub enum SetupStep {
    PickProvider,
    BaseUrl,
    ApiKey,
    /// Nous Portal RFC 8628 device-code login (free-tier JWTs).
    DeviceAuth,
    Fetching,
    FetchError { message: String },
    PickModel,
    PickEffort,
    CustomModel,
    Saving,
}

#[derive(Debug)]
pub struct SetupModalState {
    pub window: ModalWindowState,
    pub step: SetupStep,
    pub(crate) base_url: LineEditor,
    pub(crate) api_key: LineEditor,
    pub(crate) custom_model: LineEditor,
    pub(crate) reveal_api_key: bool,
    pub(crate) filter: LineEditor,
    pub(crate) models: Vec<ProviderModel>,
    pub(crate) selected: usize,
    pub(crate) generation: u64,
    pub(crate) provider_id: String,
    pub(crate) format: SetupWireFormat,
    pub kind: SetupKind,
    /// `/model` live picker: skip the URL/key wizard and close on Esc.
    pub(crate) picker_only: bool,
    pending_model: Option<ProviderModel>,
    /// Per-model effort ids shown in `PickEffort` (xAI catalog).
    effort_levels: Vec<String>,
    device_user_code: String,
    device_verification_uri: String,
    device_status: String,
}

#[derive(Debug)]
pub enum SetupOutcome {
    Unchanged,
    Changed,
    Close,
    Fetch {
        base_url: String,
        api_key: String,
        format: SetupWireFormat,
        provider_id: String,
        generation: u64,
    },
    Save {
        setup: ProviderSetup,
        generation: u64,
    },
    /// Live `/model` picker: switch the session without rewriting provider setup.
    SwitchSession {
        model_id: String,
        effort: Option<String>,
    },
    /// Start Nous Portal device-code OAuth.
    StartNousDeviceLogin {
        generation: u64,
    },
    /// Open the Nous login URL in the default browser.
    OpenNousLoginUrl,
    /// Copy the Nous login URL to the clipboard.
    CopyNousLoginUrl,
    /// Poll until the user approves the device code.
    PollNousDeviceLogin {
        device_code: String,
        user_code: String,
        verification_uri: String,
        verification_uri_complete: String,
        expires_in: u64,
        interval: u64,
        generation: u64,
    },
}

#[derive(Clone, Copy)]
enum ProviderRow {
    Builtin(&'static SetupProviderProfile),
    Custom,
}

impl SetupModalState {
    pub fn new() -> Self {
        Self::with_kind(SetupKind::Chat)
    }

    pub fn image() -> Self {
        Self::with_kind(SetupKind::Image)
    }

    fn with_kind(kind: SetupKind) -> Self {
        let mut base_url = LineEditor::default();
        let mut api_key = LineEditor::default();
        let mut custom_model = LineEditor::default();
        let mut format = SetupWireFormat::Openai;
        let mut provider_id = CUSTOM_PROVIDER_ID.to_owned();
        match kind {
            SetupKind::Chat => {
                if let Some(existing) = supercharge_shell::agent::provider_setup::load_existing_provider_setup_details()
                {
                    base_url.set_text(existing.base_url);
                    api_key.set_text(existing.api_key);
                    format = existing.format;
                    provider_id = existing.provider_id;
                }
            }
            SetupKind::Image => {
                if let Some((existing_url, existing_key, model)) =
                    supercharge_shell::agent::image_provider::load_existing_image_provider_setup()
                {
                    base_url.set_text(existing_url);
                    api_key.set_text(existing_key);
                    custom_model.set_text(model);
                }
            }
        }
        Self {
            window: ModalWindowState::default(),
            step: SetupStep::PickProvider,
            base_url,
            api_key,
            custom_model,
            reveal_api_key: false,
            filter: LineEditor::default(),
            models: Vec::new(),
            selected: 0,
            generation: 0,
            provider_id,
            format,
            kind,
            picker_only: false,
            pending_model: None,
            effort_levels: Vec::new(),
            device_user_code: String::new(),
            device_verification_uri: String::new(),
            device_status: String::new(),
        }
    }

    /// `/effort` picker: skip URL/key/model and list this model's levels.
    pub fn effort_picker(model: ProviderModel, levels: Vec<String>) -> Self {
        let mut state = Self::with_kind(SetupKind::Chat);
        state.picker_only = true;
        state.pending_model = Some(model);
        state.effort_levels = levels;
        state.selected = 0;
        state.step = SetupStep::PickEffort;
        state
    }

    /// `/model` picker: live-fetch `/models` with the saved URL + key.
    pub fn live_model_picker() -> (Self, Option<SetupOutcome>) {
        let mut state = Self::with_kind(SetupKind::Chat);
        state.picker_only = true;
        let outcome = state.begin_live_fetch();
        (state, outcome)
    }

    pub fn seed_catalog_models(&mut self, models: &crate::acp::model_state::ModelState) {
        use supercharge_shell::sampling::types::supports_reasoning_effort_meta;
        if models.available.is_empty() {
            return;
        }
        self.models = models
            .available
            .iter()
            .map(|(id, info)| ProviderModel {
                id: id.0.to_string(),
                name: info.name.clone(),
                supports_thinking: supports_reasoning_effort_meta(info.meta.as_ref())
                    || supercharge_shell::agent::provider_setup::infer_thinking_from_id(&info.name)
                    || supercharge_shell::agent::provider_setup::infer_thinking_from_id(
                        id.0.as_ref(),
                    ),
            })
            .collect();
        if !self.models.is_empty() {
            self.step = SetupStep::PickModel;
            self.selected = 0;
        }
    }

    pub fn begin_live_fetch(&mut self) -> Option<SetupOutcome> {
        match normalize_provider_base_url(self.base_url.text()) {
            Ok(base_url) => {
                self.generation = self.generation.saturating_add(1);
                self.step = SetupStep::Fetching;
                Some(SetupOutcome::Fetch {
                    base_url,
                    api_key: self.api_key.text().trim().to_owned(),
                    format: self.format,
                    provider_id: self.provider_id.clone(),
                    generation: self.generation,
                })
            }
            Err(_) => None,
        }
    }

    fn provider_rows(&self) -> Vec<ProviderRow> {
        let query = self.filter.text().trim().to_ascii_lowercase();
        let mut rows: Vec<ProviderRow> = setup_provider_profiles()
            .iter()
            .filter(|profile| {
                if self.kind == SetupKind::Image
                    && profile.format
                        != supercharge_shell::agent::provider_setup::SetupWireFormat::Openai
                {
                    return false;
                }
                query.is_empty()
                    || profile.id.contains(&query)
                    || profile.name.to_ascii_lowercase().contains(&query)
                    || profile.description.to_ascii_lowercase().contains(&query)
            })
            .map(ProviderRow::Builtin)
            .collect();
        if query.is_empty()
            || "custom".contains(&query)
            || "openai".contains(&query)
            || "base url".contains(&query)
        {
            rows.push(ProviderRow::Custom);
        }
        rows
    }

    fn filtered_models(&self) -> Vec<ProviderModel> {
        let query = self.filter.text().trim().to_ascii_lowercase();
        let mut models: Vec<ProviderModel> = self
            .models
            .iter()
            .filter(|model| {
                query.is_empty()
                    || model.id.to_ascii_lowercase().contains(&query)
                    || model.name.to_ascii_lowercase().contains(&query)
            })
            .cloned()
            .collect();
        if query.is_empty() || "custom".contains(&query) || "type".contains(&query) {
            models.push(ProviderModel {
                id: CUSTOM_MODEL_ID.to_owned(),
                name: "Type a custom model id…".to_owned(),
                supports_thinking: false,
            });
        }
        models
    }

    fn selected_model(&self) -> Option<ProviderModel> {
        self.filtered_models().get(self.selected).cloned()
    }

    pub fn apply_fetched_models(
        &mut self,
        generation: u64,
        result: Result<Vec<ProviderModel>, String>,
    ) {
        if generation != self.generation {
            return;
        }
        match result {
            Ok(models) => {
                if self.models.is_empty() {
                    self.models = models;
                } else {
                    for model in models {
                        if let Some(existing) = self
                            .models
                            .iter_mut()
                            .find(|m| m.id == model.id)
                        {
                            existing.supports_thinking |= model.supports_thinking;
                            if existing.name == existing.id && model.name != model.id {
                                existing.name = model.name;
                            }
                        } else {
                            self.models.push(model);
                        }
                    }
                }
                if self.models.is_empty() {
                    if let Some(profile) = find_setup_provider(&self.provider_id) {
                        self.models.push(ProviderModel {
                            id: profile.default_model.to_owned(),
                            name: profile.default_model.to_owned(),
                            supports_thinking: supercharge_shell::agent::provider_setup::infer_thinking_from_id(
                                profile.default_model,
                            ),
                        });
                    }
                }
                self.filter.reset();
                self.selected = 0;
                self.step = SetupStep::PickModel;
            }
            Err(message) => {
                if self.kind == SetupKind::Image {
                    self.models.clear();
                    self.filter.reset();
                    self.selected = 0;
                    self.step = SetupStep::PickModel;
                    let _ = message;
                } else {
                    self.step = SetupStep::FetchError { message };
                }
            }
        }
    }

    pub fn apply_saved(&mut self, generation: u64, result: Result<(), String>) -> bool {
        if generation != self.generation {
            return false;
        }
        match result {
            Ok(()) => true,
            Err(message) => {
                self.step = SetupStep::FetchError { message };
                false
            }
        }
    }

    pub fn apply_nous_device_started(
        &mut self,
        generation: u64,
        pending: supercharge_shell::agent::nous_oauth::DeviceCodePending,
    ) -> Option<SetupOutcome> {
        if generation != self.generation {
            return None;
        }
        self.device_user_code = pending.user_code.clone();
        self.device_verification_uri = if pending.verification_uri_complete.is_empty() {
            pending.verification_uri.clone()
        } else {
            pending.verification_uri_complete.clone()
        };
        self.device_status = "Opened in your browser. Waiting for approval…".into();
        self.step = SetupStep::DeviceAuth;
        Some(SetupOutcome::PollNousDeviceLogin {
            device_code: pending.device_code,
            user_code: pending.user_code,
            verification_uri: pending.verification_uri,
            verification_uri_complete: pending.verification_uri_complete,
            expires_in: pending.expires_in,
            interval: pending.interval,
            generation,
        })
    }

    pub fn apply_nous_device_finished(
        &mut self,
        generation: u64,
        result: Result<String, String>,
    ) -> Option<SetupOutcome> {
        if generation != self.generation {
            return None;
        }
        match result {
            Ok(access_token) => {
                self.api_key.set_text(access_token);
                self.base_url
                    .set_text(supercharge_shell::agent::nous_oauth::INFERENCE_BASE);
                self.device_status = "Logged in. Fetching models…".into();
                match normalize_provider_base_url(self.base_url.text()) {
                    Ok(base_url) => {
                        self.generation = self.generation.saturating_add(1);
                        self.step = SetupStep::Fetching;
                        Some(SetupOutcome::Fetch {
                            base_url,
                            api_key: self.api_key.text().trim().to_owned(),
                            format: self.format,
                            provider_id: self.provider_id.clone(),
                            generation: self.generation,
                        })
                    }
                    Err(error) => {
                        self.step = SetupStep::FetchError {
                            message: error.to_string(),
                        };
                        None
                    }
                }
            }
            Err(message) => {
                self.step = SetupStep::FetchError { message };
                None
            }
        }
    }

    pub fn device_login_url(&self) -> &str {
        if self.device_verification_uri.is_empty() {
            "https://portal.nousresearch.com/oauth/device"
        } else {
            self.device_verification_uri.as_str()
        }
    }
}

pub fn handle_setup_key(state: &mut SetupModalState, key: &KeyEvent) -> SetupOutcome {
    if key.kind == KeyEventKind::Release {
        return SetupOutcome::Unchanged;
    }
    if matches!(key.code, KeyCode::Esc)
        || matches!(key.code, KeyCode::F(2))
        || (matches!(key.code, KeyCode::Char(','))
            && (key.modifiers.contains(KeyModifiers::CONTROL)
                || key.modifiers.contains(KeyModifiers::SUPER)))
    {
        return match &state.step {
            SetupStep::PickProvider
            | SetupStep::Fetching
            | SetupStep::Saving
            | SetupStep::DeviceAuth => SetupOutcome::Close,
            SetupStep::BaseUrl => {
                state.step = SetupStep::PickProvider;
                state.filter.reset();
                state.selected = 0;
                SetupOutcome::Changed
            }
            SetupStep::ApiKey | SetupStep::FetchError { .. } => {
                if state.picker_only {
                    return SetupOutcome::Close;
                }
                state.step = if state.provider_id == CUSTOM_PROVIDER_ID {
                    SetupStep::BaseUrl
                } else {
                    SetupStep::PickProvider
                };
                SetupOutcome::Changed
            }
            SetupStep::PickModel => {
                if state.picker_only {
                    return SetupOutcome::Close;
                }
                state.step = SetupStep::ApiKey;
                SetupOutcome::Changed
            }
            SetupStep::PickEffort => {
                state.step = SetupStep::PickModel;
                state.pending_model = None;
                state.selected = 0;
                SetupOutcome::Changed
            }
            SetupStep::CustomModel => {
                state.step = SetupStep::PickModel;
                SetupOutcome::Changed
            }
        };
    }

    match &state.step {
        SetupStep::Fetching | SetupStep::Saving => SetupOutcome::Unchanged,
        SetupStep::DeviceAuth => handle_device_auth(state, key),
        SetupStep::PickProvider => handle_pick_provider(state, key),
        SetupStep::BaseUrl => handle_base_url(state, key),
        SetupStep::ApiKey => handle_api_key(state, key),
        SetupStep::FetchError { .. } => {
            if matches!(key.code, KeyCode::Enter) {
                if state.picker_only
                    && let Some(fetch) = state.begin_live_fetch()
                {
                    return fetch;
                }
                state.step = SetupStep::ApiKey;
                SetupOutcome::Changed
            } else {
                SetupOutcome::Unchanged
            }
        }
        SetupStep::PickModel => handle_pick_model(state, key),
        SetupStep::PickEffort => handle_pick_effort(state, key),
        SetupStep::CustomModel => handle_custom_model(state, key),
    }
}

pub fn handle_setup_paste(state: &mut SetupModalState, text: &str) -> SetupOutcome {
    match state.step {
        SetupStep::PickProvider | SetupStep::PickModel => match state.filter.insert_paste(text) {
            LineEditOutcome::Unhandled => SetupOutcome::Unchanged,
            LineEditOutcome::TextChanged => {
                state.selected = 0;
                SetupOutcome::Changed
            }
            _ => SetupOutcome::Changed,
        },
        SetupStep::PickEffort => SetupOutcome::Unchanged,
        SetupStep::BaseUrl => match state.base_url.insert_paste(text) {
            LineEditOutcome::Unhandled => SetupOutcome::Unchanged,
            _ => SetupOutcome::Changed,
        },
        SetupStep::ApiKey => match state.api_key.insert_paste(text) {
            LineEditOutcome::Unhandled => SetupOutcome::Unchanged,
            _ => SetupOutcome::Changed,
        },
        SetupStep::CustomModel => match state.custom_model.insert_paste(text) {
            LineEditOutcome::Unhandled => SetupOutcome::Unchanged,
            _ => SetupOutcome::Changed,
        },
        SetupStep::Fetching
        | SetupStep::FetchError { .. }
        | SetupStep::Saving
        | SetupStep::DeviceAuth => SetupOutcome::Unchanged,
    }
}

fn handle_device_auth(_state: &mut SetupModalState, key: &KeyEvent) -> SetupOutcome {
    match key.code {
        KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('O') => SetupOutcome::OpenNousLoginUrl,
        KeyCode::Char('c') | KeyCode::Char('C')
            if !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            SetupOutcome::CopyNousLoginUrl
        }
        _ => SetupOutcome::Unchanged,
    }
}

fn handle_pick_provider(state: &mut SetupModalState, key: &KeyEvent) -> SetupOutcome {
    let rows = state.provider_rows();
    match key.code {
        KeyCode::Up => {
            if state.selected > 0 {
                state.selected -= 1;
            }
            SetupOutcome::Changed
        }
        KeyCode::Down => {
            if !rows.is_empty() && state.selected + 1 < rows.len() {
                state.selected += 1;
            }
            SetupOutcome::Changed
        }
        KeyCode::Enter => match rows.get(state.selected).copied() {
            Some(ProviderRow::Builtin(profile)) => {
                state.provider_id = profile.id.to_owned();
                state.format = profile.format;
                state.base_url.set_text(profile.base_url);
                state.filter.reset();
                state.selected = 0;
                if profile.id == "nous" {
                    if supercharge_shell::agent::nous_oauth::resolve_access_token().is_ok() {
                        state.generation = state.generation.saturating_add(1);
                        state.step = SetupStep::Fetching;
                        return match normalize_provider_base_url(state.base_url.text()) {
                            Ok(base_url) => SetupOutcome::Fetch {
                                base_url,
                                api_key: String::new(),
                                format: state.format,
                                provider_id: state.provider_id.clone(),
                                generation: state.generation,
                            },
                            Err(error) => {
                                state.step = SetupStep::FetchError {
                                    message: error.to_string(),
                                };
                                SetupOutcome::Changed
                            }
                        };
                    }
                    state.generation = state.generation.saturating_add(1);
                    state.device_status = "Starting Nous Portal login…".into();
                    state.step = SetupStep::DeviceAuth;
                    return SetupOutcome::StartNousDeviceLogin {
                        generation: state.generation,
                    };
                }
                state.step = SetupStep::ApiKey;
                SetupOutcome::Changed
            }
            Some(ProviderRow::Custom) => {
                state.provider_id = CUSTOM_PROVIDER_ID.to_owned();
                state.format = SetupWireFormat::Openai;
                state.filter.reset();
                state.selected = 0;
                state.step = SetupStep::BaseUrl;
                SetupOutcome::Changed
            }
            None => SetupOutcome::Unchanged,
        },
        _ => match state.filter.handle_key(key) {
            LineEditOutcome::TextChanged => {
                state.selected = 0;
                SetupOutcome::Changed
            }
            LineEditOutcome::Unhandled => SetupOutcome::Unchanged,
            _ => SetupOutcome::Changed,
        },
    }
}

fn handle_base_url(state: &mut SetupModalState, key: &KeyEvent) -> SetupOutcome {
    if matches!(key.code, KeyCode::Enter) {
        match normalize_provider_base_url(state.base_url.text()) {
            Ok(url) => {
                state.base_url.set_text(url);
                state.step = SetupStep::ApiKey;
                SetupOutcome::Changed
            }
            Err(error) => {
                state.step = SetupStep::FetchError {
                    message: error.to_string(),
                };
                SetupOutcome::Changed
            }
        }
    } else {
        match state.base_url.handle_key(key) {
            LineEditOutcome::Unhandled => SetupOutcome::Unchanged,
            _ => SetupOutcome::Changed,
        }
    }
}

fn handle_api_key(state: &mut SetupModalState, key: &KeyEvent) -> SetupOutcome {
    if key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char('r') | KeyCode::Char('R'))
    {
        state.reveal_api_key = !state.reveal_api_key;
        return SetupOutcome::Changed;
    }
    if matches!(key.code, KeyCode::Enter) {
        match normalize_provider_base_url(state.base_url.text()) {
            Ok(base_url) => {
                state.generation = state.generation.saturating_add(1);
                state.step = SetupStep::Fetching;
                SetupOutcome::Fetch {
                    base_url,
                    api_key: state.api_key.text().trim().to_owned(),
                    format: state.format,
                    provider_id: state.provider_id.clone(),
                    generation: state.generation,
                }
            }
            Err(error) => {
                state.step = SetupStep::FetchError {
                    message: error.to_string(),
                };
                SetupOutcome::Changed
            }
        }
    } else {
        match state.api_key.handle_key(key) {
            LineEditOutcome::Unhandled => SetupOutcome::Unchanged,
            _ => SetupOutcome::Changed,
        }
    }
}

fn handle_pick_model(state: &mut SetupModalState, key: &KeyEvent) -> SetupOutcome {
    let filtered_len = state.filtered_models().len();
    match key.code {
        KeyCode::Up => {
            if state.selected > 0 {
                state.selected -= 1;
            }
            SetupOutcome::Changed
        }
        KeyCode::Down => {
            if filtered_len > 0 && state.selected + 1 < filtered_len {
                state.selected += 1;
            }
            SetupOutcome::Changed
        }
        KeyCode::Enter => {
            let Some(model) = state.selected_model() else {
                state.step = SetupStep::FetchError {
                    message: "no model selected".to_owned(),
                };
                return SetupOutcome::Changed;
            };
            if model.id == CUSTOM_MODEL_ID {
                if state.custom_model.text().trim().is_empty() {
                    if let Some(profile) = find_setup_provider(&state.provider_id) {
                        state.custom_model.set_text(profile.default_model);
                    }
                }
                state.step = SetupStep::CustomModel;
                return SetupOutcome::Changed;
            }
            if state.picker_only && model.supports_thinking {
                state.effort_levels = supercharge_shell::agent::provider_setup::reasoning_effort_ids_for_model(
                    &model.id,
                )
                .iter()
                .map(|s| (*s).to_owned())
                .collect();
                if state.effort_levels.is_empty() {
                    state.effort_levels = ["high", "medium", "low"]
                        .into_iter()
                        .map(str::to_owned)
                        .collect();
                }
                state.pending_model = Some(model);
                state.selected = 0;
                state.filter.reset();
                state.step = SetupStep::PickEffort;
                return SetupOutcome::Changed;
            }
            if state.picker_only {
                return SetupOutcome::SwitchSession {
                    model_id: model.id,
                    effort: None,
                };
            }
            save_selected_model(state, model.id, model.name)
        }
        _ => match state.filter.handle_key(key) {
            LineEditOutcome::TextChanged => {
                state.selected = 0;
                SetupOutcome::Changed
            }
            LineEditOutcome::Unhandled => SetupOutcome::Unchanged,
            _ => SetupOutcome::Changed,
        },
    }
}

fn handle_pick_effort(state: &mut SetupModalState, key: &KeyEvent) -> SetupOutcome {
    let level_count = state.effort_levels.len();
    match key.code {
        KeyCode::Up => {
            if state.selected > 0 {
                state.selected -= 1;
            }
            SetupOutcome::Changed
        }
        KeyCode::Down => {
            if level_count > 0 && state.selected + 1 < level_count {
                state.selected += 1;
            }
            SetupOutcome::Changed
        }
        KeyCode::Enter => {
            let Some(model) = state.pending_model.take() else {
                state.step = SetupStep::PickModel;
                return SetupOutcome::Changed;
            };
            let effort = state.effort_levels.get(state.selected).cloned();
            if state.picker_only {
                return SetupOutcome::SwitchSession {
                    model_id: model.id,
                    effort,
                };
            }
            save_selected_model(state, model.id, model.name)
        }
        _ => SetupOutcome::Unchanged,
    }
}

fn handle_custom_model(state: &mut SetupModalState, key: &KeyEvent) -> SetupOutcome {
    if matches!(key.code, KeyCode::Enter) {
        let model_id = state.custom_model.text().trim().to_owned();
        if model_id.is_empty() {
            return SetupOutcome::Unchanged;
        }
        save_selected_model(state, model_id.clone(), model_id)
    } else {
        match state.custom_model.handle_key(key) {
            LineEditOutcome::Unhandled => SetupOutcome::Unchanged,
            _ => SetupOutcome::Changed,
        }
    }
}

fn save_selected_model(
    state: &mut SetupModalState,
    model_id: String,
    model_name: String,
) -> SetupOutcome {
    match normalize_provider_base_url(state.base_url.text()) {
        Ok(base_url) => {
            state.generation = state.generation.saturating_add(1);
            state.step = SetupStep::Saving;
            SetupOutcome::Save {
                setup: ProviderSetup {
                    base_url,
                    api_key: state.api_key.text().trim().to_owned(),
                    model_id,
                    model_name,
                    format: state.format,
                    provider_id: state.provider_id.clone(),
                },
                generation: state.generation,
            }
        }
        Err(error) => {
            state.step = SetupStep::FetchError {
                message: error.to_string(),
            };
            SetupOutcome::Changed
        }
    }
}

pub fn render_setup_modal(buf: &mut Buffer, area: Rect, state: &mut SetupModalState) {
    let theme = Theme::current();
    let shortcuts = shortcuts_for(&state.step);
    let prefix = if state.picker_only {
        "Switch model"
    } else {
        match state.kind {
            SetupKind::Chat => "Provider setup",
            SetupKind::Image => "Image provider setup",
        }
    };
    let title = match &state.step {
        SetupStep::PickProvider => format!("{prefix}  ·  choose provider"),
        SetupStep::BaseUrl => format!("{prefix}  ·  base URL"),
        SetupStep::ApiKey => format!("{prefix}  ·  API key"),
        SetupStep::DeviceAuth => format!("{prefix}  ·  Nous Portal login"),
        SetupStep::Fetching => format!("{prefix}  ·  fetching models"),
        SetupStep::FetchError { .. } => format!("{prefix}  ·  error"),
        SetupStep::PickModel => format!("{prefix}  ·  choose model"),
        SetupStep::PickEffort => format!("{prefix}  ·  thinking effort"),
        SetupStep::CustomModel => format!("{prefix}  ·  custom model id"),
        SetupStep::Saving => format!("{prefix}  ·  saving"),
    };
    let title = title.as_str();
    let config = ModalWindowConfig {
        title,
        tabs: None,
        shortcuts: &shortcuts,
        sizing: ModalSizing::medium().with_compact(false),
        fold_info: None,
    };
    let Some(content) = mw::render_modal_window(buf, area, &mut state.window, &config, &theme)
    else {
        return;
    };
    let inner = content.content;
    match &state.step {
        SetupStep::PickProvider => render_provider_list(buf, inner, &theme, state),
        SetupStep::BaseUrl => render_prompt(
            buf,
            inner,
            &theme,
            "OpenAI-compatible base URL",
            "Example: https://api.openai.com/v1  or  http://127.0.0.1:11434/v1",
            state.base_url.text(),
            true,
        ),
        SetupStep::CustomModel => render_prompt(
            buf,
            inner,
            &theme,
            "Model id sent to the API",
            "Example: gpt-4o  or  anthropic/claude-sonnet-4-6",
            state.custom_model.text(),
            true,
        ),
        SetupStep::DeviceAuth => {
            let uri = if state.device_verification_uri.is_empty() {
                "https://portal.nousresearch.com/oauth/device"
            } else {
                state.device_verification_uri.as_str()
            };
            let code = if state.device_user_code.is_empty() {
                "…"
            } else {
                state.device_user_code.as_str()
            };
            render_status(
                buf,
                inner,
                &theme,
                &format!(
                    "To continue:\n  1. Open: {uri}\n  2. If prompted, enter code: {code}\n\n{}\n\nEnter/o opens the link  ·  c copies it",
                    state.device_status
                ),
            );
        }
        SetupStep::ApiKey => {
            let display = if state.reveal_api_key {
                state.api_key.text().to_owned()
            } else {
                "*".repeat(state.api_key.text().chars().count())
            };
            render_prompt(
                buf,
                inner,
                &theme,
                "API key (leave blank for local endpoints)",
                "Ctrl+R reveals the key",
                &display,
                true,
            );
        }
        SetupStep::Fetching => {
            render_status(buf, inner, &theme, "Fetching models from the provider…")
        }
        SetupStep::Saving => render_status(buf, inner, &theme, "Saving provider configuration…"),
        SetupStep::FetchError { message } => {
            render_status(buf, inner, &theme, message);
        }
        SetupStep::PickModel => render_model_list(buf, inner, &theme, state),
        SetupStep::PickEffort => render_effort_list(buf, inner, &theme, state),
    }
}

fn shortcuts_for(step: &SetupStep) -> Vec<Shortcut<'static>> {
    match step {
        SetupStep::PickProvider => vec![
            Shortcut {
                label: "Enter continue",
                clickable: false,
                id: 1,
            },
            Shortcut {
                label: "Esc close",
                clickable: false,
                id: 2,
            },
        ],
        SetupStep::BaseUrl | SetupStep::CustomModel => vec![
            Shortcut {
                label: "Enter continue",
                clickable: false,
                id: 1,
            },
            Shortcut {
                label: "Esc close",
                clickable: false,
                id: 2,
            },
        ],
        SetupStep::DeviceAuth => vec![
            Shortcut {
                label: "o open",
                clickable: false,
                id: 1,
            },
            Shortcut {
                label: "c copy",
                clickable: false,
                id: 3,
            },
            Shortcut {
                label: "Esc cancel",
                clickable: false,
                id: 2,
            },
        ],
        SetupStep::ApiKey => vec![
            Shortcut {
                label: "Enter fetch models",
                clickable: false,
                id: 1,
            },
            Shortcut {
                label: "Esc back",
                clickable: false,
                id: 2,
            },
        ],
        SetupStep::PickModel => vec![
            Shortcut {
                label: "Enter save",
                clickable: false,
                id: 1,
            },
            Shortcut {
                label: "Esc back",
                clickable: false,
                id: 2,
            },
        ],
        SetupStep::PickEffort => vec![
            Shortcut {
                label: "Enter set effort",
                clickable: false,
                id: 1,
            },
            Shortcut {
                label: "Esc back",
                clickable: false,
                id: 2,
            },
        ],
        SetupStep::FetchError { .. } => vec![
            Shortcut {
                label: "Enter retry",
                clickable: false,
                id: 1,
            },
            Shortcut {
                label: "Esc back",
                clickable: false,
                id: 2,
            },
        ],
        SetupStep::Fetching | SetupStep::Saving => vec![Shortcut {
            label: "Esc cancel",
            clickable: false,
            id: 2,
        }],
    }
}

fn render_prompt(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    label: &str,
    hint: &str,
    value: &str,
    show_cursor: bool,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    Line::from(Span::styled(
        label,
        Style::default()
            .fg(theme.text_primary)
            .add_modifier(Modifier::BOLD),
    ))
    .render(Rect::new(area.x, area.y, area.width, 1), buf);
    if area.height > 2 {
        Line::from(Span::styled(hint, Style::default().fg(theme.gray_dim)))
            .render(Rect::new(area.x, area.y + 1, area.width, 1), buf);
    }
    if area.height > 4 {
        let field_y = area.y + 3;
        let shown = if value.is_empty() { " " } else { value };
        let truncated = truncate_to_width(shown, area.width.saturating_sub(1) as usize);
        Line::from(Span::styled(
            format!(" {truncated}"),
            Style::default().fg(theme.text_primary).bg(theme.bg_hover),
        ))
        .render(Rect::new(area.x, field_y, area.width, 1), buf);
        if show_cursor {
            let cursor_x = area
                .x
                .saturating_add(1 + truncated.width() as u16)
                .min(area.x + area.width.saturating_sub(1));
            if let Some(cell) = buf.cell_mut((cursor_x, field_y)) {
                cell.set_style(cell.style().add_modifier(Modifier::REVERSED));
            }
        }
    }
}

fn render_status(buf: &mut Buffer, area: Rect, theme: &Theme, message: &str) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let wrapped = textwrap::wrap(message, area.width as usize);
    for (index, line) in wrapped.into_iter().take(area.height as usize).enumerate() {
        Line::from(Span::styled(
            line.to_string(),
            Style::default().fg(theme.text_primary),
        ))
        .render(Rect::new(area.x, area.y + index as u16, area.width, 1), buf);
    }
}

fn render_provider_list(buf: &mut Buffer, area: Rect, theme: &Theme, state: &SetupModalState) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let filter = if state.filter.text().is_empty() {
        "type to filter providers".to_owned()
    } else {
        format!("/{}", state.filter.text())
    };
    Line::from(Span::styled(filter, Style::default().fg(theme.gray_dim)))
        .render(Rect::new(area.x, area.y, area.width, 1), buf);
    let list_y = area.y.saturating_add(2);
    let list_height = area.height.saturating_sub(2);
    if list_height == 0 {
        return;
    }
    let rows = state.provider_rows();
    if rows.is_empty() {
        Line::from(Span::styled(
            "No providers match that filter",
            Style::default().fg(theme.gray_dim),
        ))
        .render(Rect::new(area.x, list_y, area.width, 1), buf);
        return;
    }
    let selected = state.selected.min(rows.len().saturating_sub(1));
    let start = selected.saturating_sub(list_height as usize / 2);
    for (row, item) in rows
        .iter()
        .skip(start)
        .take(list_height as usize)
        .enumerate()
    {
        let index = start + row;
        let label = match item {
            ProviderRow::Builtin(profile) => format!("{}  {}", profile.name, profile.description),
            ProviderRow::Custom => "Custom / paste a base URL".to_owned(),
        };
        let truncated = truncate_to_width(&label, area.width as usize);
        let style = if index == selected {
            Style::default()
                .fg(theme.fuzzy_accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary)
        };
        Line::from(Span::styled(truncated, style))
            .render(Rect::new(area.x, list_y + row as u16, area.width, 1), buf);
    }
}

fn render_model_list(buf: &mut Buffer, area: Rect, theme: &Theme, state: &SetupModalState) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let filter = if state.filter.text().is_empty() {
        "type to filter models".to_owned()
    } else {
        format!("/{}", state.filter.text())
    };
    Line::from(Span::styled(filter, Style::default().fg(theme.gray_dim)))
        .render(Rect::new(area.x, area.y, area.width, 1), buf);
    let list_y = area.y.saturating_add(2);
    let list_height = area.height.saturating_sub(2);
    if list_height == 0 {
        return;
    }
    let models = state.filtered_models();
    if models.is_empty() {
        Line::from(Span::styled(
            "No models match that filter",
            Style::default().fg(theme.gray_dim),
        ))
        .render(Rect::new(area.x, list_y, area.width, 1), buf);
        return;
    }
    let selected = state.selected.min(models.len().saturating_sub(1));
    let start = selected.saturating_sub(list_height as usize / 2);
    for (row, model) in models
        .iter()
        .skip(start)
        .take(list_height as usize)
        .enumerate()
    {
        let index = start + row;
        let mut label = if model.name == model.id {
            model.id.clone()
        } else {
            format!("{}  {}", model.id, model.name)
        };
        if model.supports_thinking {
            label.push_str("  · thinking");
        }
        let truncated = truncate_to_width(&label, area.width as usize);
        let style = if index == selected {
            Style::default()
                .fg(theme.fuzzy_accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary)
        };
        Line::from(Span::styled(truncated, style))
            .render(Rect::new(area.x, list_y + row as u16, area.width, 1), buf);
    }
}

fn render_effort_list(buf: &mut Buffer, area: Rect, theme: &Theme, state: &SetupModalState) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let model_name = state
        .pending_model
        .as_ref()
        .map(|m| m.name.as_str())
        .unwrap_or("model");
    Line::from(Span::styled(
        format!("Thinking effort for {model_name}"),
        Style::default().fg(theme.gray_dim),
    ))
    .render(Rect::new(area.x, area.y, area.width, 1), buf);
    let list_y = area.y.saturating_add(2);
    let selected = state
        .selected
        .min(state.effort_levels.len().saturating_sub(1));
    for (index, level) in state.effort_levels.iter().enumerate() {
        let style = if index == selected {
            Style::default()
                .fg(theme.fuzzy_accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.text_primary)
        };
        Line::from(Span::styled(level.as_str(), style)).render(
            Rect::new(area.x, list_y + index as u16, area.width, 1),
            buf,
        );
    }
}

fn truncate_to_width(text: &str, width: usize) -> String {
    if text.width() <= width {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0usize;
    for ch in text.chars() {
        let next = unicode_width::UnicodeWidthChar::width(ch).unwrap_or(1);
        if used + next + 1 > width {
            break;
        }
        out.push(ch);
        used += next;
    }
    out.push('…');
    out
}

pub fn chrome_close_requested(state: &mut SetupModalState, key: &KeyEvent) -> bool {
    let config = ModalWindowConfig {
        title: "",
        tabs: None,
        shortcuts: &[],
        sizing: ModalSizing::default(),
        fold_info: None,
    };
    matches!(
        mw::handle_modal_key(&mut state.window, key, &config),
        ModalWindowOutcome::CloseRequested
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyEventState;

    fn enter() -> KeyEvent {
        KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn char_key(ch: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(ch),
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn builtin_provider_skips_base_url() {
        let mut state = SetupModalState::new();
        assert!(matches!(state.step, SetupStep::PickProvider));
        assert!(matches!(
            handle_setup_key(&mut state, &enter()),
            SetupOutcome::Changed
        ));
        assert!(matches!(state.step, SetupStep::ApiKey));
        assert_eq!(state.provider_id, "pollinations");
        assert_eq!(state.base_url.text(), "https://gen.pollinations.ai/v1");
    }

    #[test]
    fn wizard_fetches_after_url_and_key() {
        let mut state = SetupModalState::new();
        state.step = SetupStep::BaseUrl;
        state.provider_id = CUSTOM_PROVIDER_ID.to_owned();
        for ch in "https://api.openai.com/v1".chars() {
            let _ = handle_setup_key(&mut state, &char_key(ch));
        }
        assert!(matches!(
            handle_setup_key(&mut state, &enter()),
            SetupOutcome::Changed
        ));
        assert!(matches!(state.step, SetupStep::ApiKey));
        for ch in "sk-test".chars() {
            let _ = handle_setup_key(&mut state, &char_key(ch));
        }
        match handle_setup_key(&mut state, &enter()) {
            SetupOutcome::Fetch {
                base_url,
                api_key,
                format,
                provider_id,
                ..
            } => {
                assert_eq!(base_url, "https://api.openai.com/v1");
                assert_eq!(api_key, "sk-test");
                assert_eq!(format, SetupWireFormat::Openai);
                assert_eq!(provider_id, CUSTOM_PROVIDER_ID);
            }
            other => panic!("expected fetch, got {other:?}"),
        }
    }

    #[test]
    fn pick_model_saves_selected_id() {
        let mut state = SetupModalState::new();
        state.base_url.set_text("https://api.openai.com/v1");
        state.api_key.set_text("sk-test");
        state.apply_fetched_models(
            0,
            Ok(vec![
                ProviderModel {
                    id: "gpt-4o".to_owned(),
                    name: "GPT-4o".to_owned(),
                    supports_thinking: false,
                },
                ProviderModel {
                    id: "o3-mini".to_owned(),
                    name: "o3-mini".to_owned(),
                    supports_thinking: true,
                },
            ]),
        );
        assert!(matches!(state.step, SetupStep::PickModel));
        let _ = handle_setup_key(
            &mut state,
            &KeyEvent {
                code: KeyCode::Down,
                modifiers: KeyModifiers::NONE,
                kind: KeyEventKind::Press,
                state: KeyEventState::NONE,
            },
        );
        match handle_setup_key(&mut state, &enter()) {
            SetupOutcome::Save { setup, .. } => {
                assert_eq!(setup.model_id, "o3-mini");
                assert_eq!(setup.base_url, "https://api.openai.com/v1");
                assert_eq!(setup.format, SetupWireFormat::Openai);
            }
            other => panic!("expected save, got {other:?}"),
        }
    }
}
