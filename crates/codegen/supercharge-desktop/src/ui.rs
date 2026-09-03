use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Instant;

use agent_client_protocol as acp;
use eframe::egui;

use crate::protocol::{LineKind, TranscriptLine, resolve_agent_bin};
use crate::runtime::{AppCommand, PendingPermission, UiEvent, spawn_runtime};

mod palette {
    use eframe::egui::Color32;

    pub const BG: Color32 = Color32::from_rgb(18, 18, 24);
    pub const PANEL: Color32 = Color32::from_rgb(24, 24, 32);
    pub const SIDEBAR: Color32 = Color32::from_rgb(14, 14, 19);
    pub const ACCENT: Color32 = Color32::from_rgb(255, 149, 84);
    pub const ACCENT_DIM: Color32 = Color32::from_rgb(120, 78, 50);
    pub const USER_BUBBLE: Color32 = Color32::from_rgb(39, 39, 51);
    pub const AGENT_BUBBLE: Color32 = Color32::from_rgb(28, 30, 38);
    pub const TEXT: Color32 = Color32::from_rgb(228, 230, 235);
    pub const TEXT_DIM: Color32 = Color32::from_rgb(139, 143, 153);
    pub const THOUGHT: Color32 = Color32::from_rgb(163, 158, 186);
    pub const TOOL: Color32 = Color32::from_rgb(126, 200, 255);
    pub const ERROR: Color32 = Color32::from_rgb(255, 118, 118);
    pub const BORDER: Color32 = Color32::from_rgb(44, 46, 56);
}

pub struct DesktopApp {
    cwd: String,
    input: String,
    transcript: Vec<TranscriptLine>,
    status: String,
    session_id: Option<String>,
    busy: bool,
    permission: Option<PendingPermission>,
    ui_rx: mpsc::Receiver<UiEvent>,
    cmd_tx: std::sync::mpsc::Sender<AppCommand>,
    last_paint: Instant,
    connected: bool,
    scrolled: bool,
}

impl DesktopApp {
    pub fn new(agent_bin: PathBuf, cwd: PathBuf) -> anyhow::Result<Self> {
        let (ui_tx, ui_rx) = mpsc::channel();
        let cmd_tx = spawn_runtime(agent_bin, cwd.clone(), ui_tx);
        Ok(Self {
            cwd: cwd.display().to_string(),
            input: String::new(),
            transcript: Vec::new(),
            status: "connecting…".into(),
            session_id: None,
            busy: false,
            permission: None,
            ui_rx,
            cmd_tx,
            last_paint: Instant::now(),
            connected: false,
            scrolled: false,
        })
    }

    fn drain_events(&mut self) {
        while let Ok(event) = self.ui_rx.try_recv() {
            match event {
                UiEvent::Status(s) => self.status = s,
                UiEvent::Line(line) => {
                    if line.kind == LineKind::Agent
                        && let Some(last) = self.transcript.last_mut()
                        && last.kind == LineKind::Agent
                    {
                        last.text.push_str(&line.text);
                        continue;
                    }
                    if line.kind == LineKind::Tool
                        && let Some(last) = self.transcript.last_mut()
                        && last.kind == LineKind::Tool
                        && last.tool_open.is_none()
                    {
                        // A tool call update that follows the call; merge into one chip.
                        last.text = if last.text.is_empty() {
                            line.text
                        } else {
                            format!("{}\n{}", last.text, line.text)
                        };
                        last.tool_open = line.tool_open;
                        continue;
                    }
                    self.transcript.push(line);
                }
                UiEvent::Permission(pending) => self.permission = Some(pending),
                UiEvent::Ready { session_id } => {
                    self.session_id = Some(session_id.clone());
                    self.connected = true;
                    self.status = format!("session {session_id}");
                    self.transcript.push(TranscriptLine {
                        kind: LineKind::System,
                        text: format!("Ready. Workspace: {}", self.cwd),
                        ..TranscriptLine::default()
                    });
                    self.scrolled = false;
                }
                UiEvent::TurnDone => self.busy = false,
                UiEvent::Error(err) => {
                    self.busy = false;
                    self.status = err.clone();
                    let mut line = TranscriptLine {
                        kind: LineKind::System,
                        text: err,
                        ..TranscriptLine::default()
                    };
                    line.error = true;
                    self.transcript.push(line);
                }
            }
        }
    }

    fn send_prompt(&mut self) {
        let text = self.input.trim().to_string();
        if text.is_empty() || self.busy || self.session_id.is_none() {
            return;
        }
        self.input.clear();
        self.busy = true;
        self.scrolled = false;
        let _ = self.cmd_tx.send(AppCommand::Prompt(text));
    }

    fn answer_permission(&mut self, outcome: acp::RequestPermissionOutcome) {
        if let Some(pending) = self.permission.take() {
            let _ = pending
                .reply
                .send(acp::RequestPermissionResponse::new(outcome));
        }
    }
}

impl eframe::App for DesktopApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        if self.busy && self.last_paint.elapsed() >= std::time::Duration::from_millis(100) {
            self.last_paint = Instant::now();
            ctx.request_repaint();
        }

        self.top_bar(ctx);
        self.status_bar(ctx);
        self.permission_card(ctx);

        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(palette::BG))
            .show(ctx, |ui| {
                self.transcript_view(ui);
            });

        self.composer(ctx);
    }
}

impl DesktopApp {
    fn top_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("top")
            .frame(
                egui::Frame::NONE
                    .fill(palette::PANEL)
                    .inner_margin(egui::Margin::symmetric(16, 10)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(
                        egui::RichText::new("⚡ Supercharge")
                            .color(palette::ACCENT)
                            .strong()
                            .size(17.0),
                    );
                    ui.label(
                        egui::RichText::new("Desktop")
                            .color(palette::TEXT_DIM)
                            .size(12.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (dot, label) = if self.connected {
                            (palette::ACCENT, "connected")
                        } else {
                            (palette::ERROR, "offline")
                        };
                        ui.label(
                            egui::RichText::new(format!("● {label}"))
                                .color(dot)
                                .size(12.0),
                        );
                        ui.separator();
                        ui.label(
                            egui::RichText::new(&self.status)
                                .color(palette::TEXT_DIM)
                                .size(12.0),
                        );
                    });
                });
            });
    }

    fn status_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("status")
            .frame(
                egui::Frame::NONE
                    .fill(palette::SIDEBAR)
                    .inner_margin(egui::Margin::symmetric(16, 4)),
            )
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    if self.busy {
                        ui.spinner();
                        ui.label(
                            egui::RichText::new("working…")
                                .color(palette::ACCENT)
                                .size(11.0),
                        );
                    } else {
                        ui.label(
                            egui::RichText::new("●")
                                .color(if self.connected {
                                    palette::ACCENT
                                } else {
                                    palette::TEXT_DIM
                                })
                                .size(11.0),
                        );
                        ui.label(
                            egui::RichText::new("idle")
                                .color(palette::TEXT_DIM)
                                .size(11.0),
                        );
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            egui::RichText::new(format!("cwd {}", self.cwd))
                                .color(palette::TEXT_DIM)
                                .size(11.0),
                        );
                    });
                });
            });
    }

    fn transcript_view(&mut self, ui: &mut egui::Ui) {
        let _ = egui::ScrollArea::vertical()
            .stick_to_bottom(true)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_space(8.0);
                if self.transcript.is_empty() && self.connected {
                    ui.add_space(ui.available_height() / 3.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("⚡")
                                .size(56.0)
                                .color(palette::ACCENT_DIM),
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new("Supercharge")
                                .size(22.0)
                                .color(palette::TEXT),
                        );
                        ui.label(
                            egui::RichText::new("Ask anything about your workspace")
                                .color(palette::TEXT_DIM),
                        );
                        ui.add_space(12.0);
                        ui.label(
                            egui::RichText::new("Enter to send · Shift+Enter for newline")
                                .size(11.0)
                                .color(palette::TEXT_DIM),
                        );
                    });
                }
                ui.add_space(8.0);
                for i in 0..self.transcript.len() {
                    let line = &mut self.transcript[i];
                    match line.kind {
                        LineKind::User => user_bubble(ui, line),
                        LineKind::Agent => agent_bubble(ui, line),
                        LineKind::Thought => thought_block(ui, line),
                        LineKind::Tool => tool_chip(ui, line),
                        LineKind::System => system_line(ui, line),
                    }
                    ui.add_space(4.0);
                }
                if self.busy {
                    ui.horizontal(|ui| {
                        ui.add_space(28.0);
                        ui.label(egui::RichText::new("● ● ●").color(palette::ACCENT_DIM));
                    });
                }
            });
    }

    fn composer(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("input")
            .frame(
                egui::Frame::NONE
                    .fill(palette::PANEL)
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(ctx, |ui| {
                let can_send = !self.busy && self.session_id.is_some() && !self.permission.is_some();
                let send = egui::Button::new(
                    egui::RichText::new("Send ➤").color(if can_send {
                        palette::TEXT
                    } else {
                        palette::TEXT_DIM
                    }),
                )
                .fill(if can_send {
                    palette::ACCENT
                } else {
                    palette::USER_BUBBLE
                })
                .corner_radius(6.0);
                let stop = egui::Button::new(
                    egui::RichText::new("■ Stop").color(palette::TEXT),
                )
                .fill(palette::USER_BUBBLE)
                .corner_radius(6.0);

                ui.horizontal(|ui| {
                    let edit = ui.add(
                        egui::TextEdit::multiline(&mut self.input)
                            .desired_rows(2)
                            .desired_width(ui.available_width() - 140.0)
                            .hint_text("Message Supercharge…")
                            .frame(true),
                    );
                    if edit.has_focus()
                        && ui.input(|i| i.key_pressed(egui::Key::Enter) && !i.modifiers.shift)
                    {
                        self.send_prompt();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::TOP), |ui| {
                        if self.busy {
                            if ui.add(stop).clicked() {
                                let _ = self.cmd_tx.send(AppCommand::CancelTurn);
                            }
                            ui.add_enabled(false, send);
                        } else {
                            if ui.add_enabled(can_send, send).clicked() {
                                self.send_prompt();
                            }
                        }
                    });
                });
            });
    }

    fn permission_card(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.permission.as_ref() else {
            return;
        };
        let title = pending.title.clone();
        let options = pending.options.clone();
        egui::Area::new(egui::Id::new("permission-overlay"))
            .fixed_pos(ctx.screen_rect().center() - egui::vec2(210.0, 90.0))
            .order(egui::Order::Middle)
            .interactable(true)
            .show(ctx, |ui| {
                egui::Frame::default()
                    .fill(palette::PANEL)
                    .stroke(egui::Stroke::new(1.0, palette::BORDER))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::same(18))
                    .shadow(egui::Shadow {
                        offset: [0, 8],
                        blur: 32,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(180),
                    })
                    .show(ui, |ui| {
                        ui.set_width(420.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new("Permission requested")
                                    .color(palette::ACCENT)
                                    .strong(),
                            );
                        });
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(&title)
                                .color(palette::TEXT)
                                .size(14.0),
                        );
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            for (id, name) in &options {
                                let primary = name.to_lowercase().contains("allow")
                                    && !name.to_lowercase().contains("always");
                                let btn = egui::Button::new(
                                    egui::RichText::new(name).color(if primary {
                                        palette::TEXT
                                    } else {
                                        palette::TEXT_DIM
                                    }),
                                )
                                .fill(if primary {
                                    palette::ACCENT
                                } else {
                                    palette::USER_BUBBLE
                                })
                                .corner_radius(6.0);
                                if ui.add(btn).clicked() {
                                    self.answer_permission(
                                        acp::RequestPermissionOutcome::Selected(
                                            acp::SelectedPermissionOutcome::new(
                                                acp::PermissionOptionId::new(id.clone()),
                                            ),
                                        ),
                                    );
                                }
                            }
                            if ui
                                .button(egui::RichText::new("Cancel").color(palette::TEXT_DIM))
                                .clicked()
                            {
                                self.answer_permission(acp::RequestPermissionOutcome::Cancelled);
                            }
                        });
                    });
            });
    }
}

fn user_bubble(ui: &mut egui::Ui, line: &mut TranscriptLine) {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            egui::Frame::default()
                .fill(palette::USER_BUBBLE)
                .corner_radius(10.0)
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_max_width(ui.available_width() - 48.0);
                    ui.label(egui::RichText::new(&line.text).color(palette::TEXT));
                });
        });
    });
}

fn agent_bubble(ui: &mut egui::Ui, line: &mut TranscriptLine) {
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("⚡")
                .color(palette::ACCENT)
                .size(15.0),
        );
        egui::Frame::default()
            .fill(palette::AGENT_BUBBLE)
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 64.0);
                ui.label(egui::RichText::new(&line.text).color(palette::TEXT));
            });
    });
}

fn thought_block(ui: &mut egui::Ui, line: &mut TranscriptLine) {
    if line.tool_open.is_none() {
        line.tool_open = Some(false);
    }
    let open = line.tool_open.unwrap_or(false);
    let collapsed_summary = if line.text.chars().count() > 70 {
        let done: String = line.text.chars().take(70).collect();
        format!("{done}…")
    } else {
        line.text.clone()
    };
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(if open { "▾" } else { "▸" })
                .color(palette::THOUGHT)
                .size(11.0),
        );
        if ui
            .add(egui::Button::new(
                egui::RichText::new(format!(
                    "Thinking  {}",
                    if open { "· open" } else { "" }
                ))
                .color(palette::THOUGHT)
                .size(11.0),
            )
            .fill(egui::Color32::TRANSPARENT))
            .clicked()
        {
            line.tool_open = Some(!open);
        }
    });
    if open {
        egui::Frame::default()
            .fill(egui::Color32::from_rgb(24, 23, 30))
            .stroke(egui::Stroke::new(1.0, palette::BORDER))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 64.0);
                ui.label(
                    egui::RichText::new(&line.text)
                        .color(palette::THOUGHT)
                        .size(12.0),
                );
            });
    } else {
        ui.label(
            egui::RichText::new(collapsed_summary)
                .color(palette::THOUGHT)
                .size(11.0)
                .weak(),
        );
    }
}

fn tool_chip(ui: &mut egui::Ui, line: &mut TranscriptLine) {
    if line.tool_open.is_none() {
        line.tool_open = Some(false);
    }
    let open = line.tool_open.unwrap_or(false);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("◆").color(palette::TOOL).size(10.0));
        if ui
            .add(
                egui::Button::new(
                    egui::RichText::new(format!(
                        "{}  {}",
                        if open { "▾" } else { "▸" },
                        first_line(&line.text)
                    ))
                    .color(palette::TOOL)
                    .size(12.0),
                )
                .fill(egui::Color32::TRANSPARENT),
            )
            .clicked()
        {
            line.tool_open = Some(!open);
        }
    });
    if open && line.text.contains('\n') {
        egui::Frame::default()
            .fill(egui::Color32::from_rgb(22, 26, 33))
            .stroke(egui::Stroke::new(1.0, palette::BORDER))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::same(10))
            .show(ui, |ui| {
                ui.set_max_width(ui.available_width() - 64.0);
                ui.label(
                    egui::RichText::new(&line.text)
                        .color(palette::TEXT_DIM)
                        .size(11.0)
                        .monospace(),
                );
            });
    }
}

fn system_line(ui: &mut egui::Ui, line: &mut TranscriptLine) {
    let color = if line.error {
        palette::ERROR
    } else {
        palette::TEXT_DIM
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("·").color(color).size(14.0));
        ui.label(
            egui::RichText::new(&line.text)
                .color(color)
                .size(11.0)
                .italics(),
        );
    });
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or("")
}

pub fn run(agent_bin: Option<PathBuf>, cwd: Option<PathBuf>) -> anyhow::Result<()> {
    let agent_bin = resolve_agent_bin(agent_bin.as_deref())?;
    let cwd = cwd.unwrap_or(std::env::current_dir()?);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_title("Supercharge Desktop"),
        ..Default::default()
    };
    eframe::run_native(
        "Supercharge Desktop",
        options,
        Box::new(move |cc| {
            configure_style(&cc.egui_ctx);
            Ok(Box::new(DesktopApp::new(agent_bin, cwd)?))
        }),
    )
    .map_err(|err| anyhow::anyhow!("{err}"))
}

fn configure_style(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = palette::BG;
    style.visuals.window_fill = palette::PANEL;
    style.visuals.extreme_bg_color = palette::BG;
    style.visuals.faint_bg_color = palette::USER_BUBBLE;
    style.visuals.widgets.noninteractive.bg_fill = palette::PANEL;
    style.visuals.widgets.inactive.weak_bg_fill = palette::USER_BUBBLE;
    style.visuals.widgets.hovered.weak_bg_fill = palette::USER_BUBBLE;
    style.visuals.widgets.active.weak_bg_fill = palette::USER_BUBBLE;
    style.visuals.widgets.noninteractive.fg_stroke =
        egui::Stroke::new(1.0, palette::TEXT_DIM);
    style.visuals.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, palette::TEXT);
    style.visuals.selection.bg_fill = palette::ACCENT_DIM;
    style.visuals.selection.stroke = egui::Stroke::new(1.0, palette::ACCENT);
    style.visuals.window_stroke = egui::Stroke::new(1.0, palette::BORDER);
    style.spacing.item_spacing = egui::vec2(8.0, 8.0);
    style.spacing.button_padding = egui::vec2(12.0, 6.0);
    ctx.set_style(style);
}
