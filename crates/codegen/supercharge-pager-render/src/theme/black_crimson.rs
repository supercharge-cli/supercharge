//! Black Crimson — near-black canvas with crimson accents.

use ratatui::style::{Color, Modifier};

use super::tokyonight::Theme;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

#[allow(dead_code)]
mod palette {
    use super::*;

    pub const BG: Color = rgb(0, 0, 0); // #000000
    pub const BG_DARK: Color = rgb(10, 10, 10); // #0A0A0A
    pub const BG_BASE: Color = rgb(8, 8, 8); // #080808
    pub const BG_HIGHLIGHT: Color = rgb(32, 12, 16); // #200C10 — crimson-tinted raise
    pub const BG_HOVER: Color = rgb(28, 10, 14); // #1C0A0E

    pub const FG: Color = rgb(244, 244, 244); // #F4F4F4
    pub const FG_DARK: Color = rgb(196, 196, 196); // #C4C4C4
    pub const GUTTER: Color = rgb(72, 72, 72); // #484848
    pub const MUTED: Color = rgb(120, 112, 112); // #787070
    pub const BRIGHT: Color = rgb(168, 160, 160); // #A8A0A0

    pub const CRIMSON: Color = rgb(220, 20, 60); // #DC143C
    pub const CRIMSON_BRIGHT: Color = rgb(248, 80, 96); // #F85060
    pub const CRIMSON_DIM: Color = rgb(176, 24, 48); // #B01830
    pub const ROSE: Color = rgb(251, 113, 133); // #FB7185
    pub const GOLD: Color = rgb(245, 158, 11); // #F59E0B
    pub const GREEN: Color = rgb(74, 222, 128); // #4ADE80
    pub const CYAN: Color = rgb(103, 232, 249); // #67E8F9
    pub const RED_DARK: Color = rgb(48, 8, 12);
    pub const GREEN_DARK: Color = rgb(8, 32, 16);
}
use palette::*;

impl Theme {
    pub const fn black_crimson() -> Self {
        Self {
            bg_base: BG_BASE,
            bg_light: BG_HIGHLIGHT,
            bg_dark: BG_DARK,
            bg_highlight: BG_HIGHLIGHT,
            bg_hover: BG_HOVER,
            bg_terminal: BG,

            accent_user: CRIMSON_BRIGHT,
            accent_assistant: CRIMSON,
            accent_thinking: MUTED,
            accent_tool: BRIGHT,
            accent_system: ROSE,
            accent_error: CRIMSON_BRIGHT,
            accent_success: GREEN,
            accent_running: CRIMSON_DIM,
            accent_skill: ROSE,

            text_primary: FG,
            text_secondary: FG_DARK,

            gray_dim: GUTTER,
            gray: MUTED,
            gray_bright: BRIGHT,

            command: GOLD,
            path: ROSE,
            running: CYAN,
            warning: GOLD,

            fuzzy_accent: CRIMSON_BRIGHT,

            accent_plan: GOLD,
            accent_verify: CRIMSON,
            accent_remember: GREEN,

            selection_border: CRIMSON,
            hover_border: CRIMSON_DIM,
            prompt_border: rgb(72, 24, 32),
            prompt_border_active: CRIMSON_BRIGHT,

            accent_model: ROSE,

            scrollbar_bg: BG_DARK,
            scrollbar_fg: rgb(120, 36, 48),

            diff_delete_bg: RED_DARK,
            diff_delete_fg: CRIMSON_BRIGHT,
            diff_insert_bg: GREEN_DARK,
            diff_insert_fg: GREEN,
            diff_equal_fg: MUTED,
            diff_gutter_fg: MUTED,

            bg_visual: rgb(40, 12, 18),

            paste_bg: BG_DARK,
            paste_fg: FG_DARK,
            paste_dim: GUTTER,

            md_heading_h1: CRIMSON_BRIGHT,
            md_heading_h1_mod: Modifier::BOLD,
            md_heading_h2: CRIMSON,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: ROSE,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: GOLD,
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: BRIGHT,
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: MUTED,
            md_heading_h6_mod: Modifier::empty(),
            md_code: ROSE,
            md_task_checked: GREEN,
            md_task_unchecked: FG_DARK,
            md_muted: MUTED,
            md_code_bg: rgb(16, 8, 10),
            md_text: FG,
            link_fg: CRIMSON_BRIGHT,
        }
    }
}
