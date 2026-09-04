//! Navy Cyan — light navy canvas with cyan accents (default CLI theme).

use ratatui::style::{Color, Modifier};

use super::tokyonight::Theme;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

#[allow(dead_code)]
mod palette {
    use super::*;

    pub const BG: Color = rgb(12, 22, 40); // #0C1628 — terminal bg
    pub const BG_DARK: Color = rgb(16, 30, 52); // #101E34
    pub const BG_BASE: Color = rgb(22, 42, 72); // #162A48 — light navy canvas
    pub const BG_HIGHLIGHT: Color = rgb(32, 62, 104); // #203E68
    pub const BG_HOVER: Color = rgb(28, 54, 92); // #1C365C

    pub const FG: Color = rgb(232, 246, 255); // #E8F6FF
    pub const FG_DARK: Color = rgb(176, 210, 232); // #B0D2E8
    pub const GUTTER: Color = rgb(72, 108, 148); // #486C94
    pub const MUTED: Color = rgb(120, 156, 188); // #789CBC
    pub const BRIGHT: Color = rgb(168, 204, 228); // #A8CCE4

    pub const CYAN: Color = rgb(94, 234, 212); // #5EEAD4
    pub const CYAN_BRIGHT: Color = rgb(103, 232, 249); // #67E8F9
    pub const CYAN_DIM: Color = rgb(34, 211, 238); // #22D3EE
    pub const NAVY: Color = rgb(96, 165, 250); // #60A5FA
    pub const TEAL: Color = rgb(45, 212, 191); // #2DD4BF
    pub const GREEN: Color = rgb(74, 222, 128); // #4ADE80
    pub const GOLD: Color = rgb(251, 191, 36); // #FBBF24
    pub const ORANGE: Color = rgb(251, 146, 60); // #FB923C
    pub const RED: Color = rgb(248, 113, 113); // #F87171
    pub const RED_DARK: Color = rgb(66, 14, 20);
    pub const GREEN_DARK: Color = rgb(6, 56, 28);
}
use palette::*;

impl Theme {
    pub const fn navy_cyan() -> Self {
        Self {
            bg_base: BG_BASE,
            bg_light: BG_HIGHLIGHT,
            bg_dark: BG_DARK,
            bg_highlight: BG_HIGHLIGHT,
            bg_hover: BG_HOVER,
            bg_terminal: BG,

            accent_user: CYAN_BRIGHT,
            accent_assistant: CYAN,
            accent_thinking: MUTED,
            accent_tool: BRIGHT,
            accent_system: CYAN_DIM,
            accent_error: RED,
            accent_success: GREEN,
            accent_running: CYAN_DIM,
            accent_skill: TEAL,

            text_primary: FG,
            text_secondary: FG_DARK,

            gray_dim: GUTTER,
            gray: MUTED,
            gray_bright: BRIGHT,

            command: GOLD,
            path: ORANGE,
            running: CYAN,
            warning: GOLD,

            fuzzy_accent: CYAN_BRIGHT,

            accent_plan: GOLD,
            accent_verify: NAVY,
            accent_remember: GREEN,

            selection_border: rgb(56, 148, 196),
            hover_border: rgb(40, 92, 140),
            prompt_border: rgb(36, 84, 128),
            prompt_border_active: CYAN_BRIGHT,

            accent_model: TEAL,

            scrollbar_bg: BG_DARK,
            scrollbar_fg: rgb(72, 140, 188),

            diff_delete_bg: RED_DARK,
            diff_delete_fg: RED,
            diff_insert_bg: GREEN_DARK,
            diff_insert_fg: GREEN,
            diff_equal_fg: MUTED,
            diff_gutter_fg: MUTED,

            bg_visual: rgb(28, 72, 116),

            paste_bg: BG_DARK,
            paste_fg: FG_DARK,
            paste_dim: GUTTER,

            md_heading_h1: CYAN_BRIGHT,
            md_heading_h1_mod: Modifier::BOLD,
            md_heading_h2: CYAN,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: NAVY,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: TEAL,
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: GOLD,
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: MUTED,
            md_heading_h6_mod: Modifier::empty(),
            md_code: CYAN_DIM,
            md_task_checked: GREEN,
            md_task_unchecked: FG_DARK,
            md_muted: MUTED,
            md_code_bg: rgb(14, 32, 56),
            md_text: FG,
            link_fg: CYAN_BRIGHT,
        }
    }
}
