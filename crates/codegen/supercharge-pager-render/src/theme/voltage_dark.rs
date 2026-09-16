//! VoltageDark theme: neutral gray base with TokyoNight accent colors.
//!
//! The canonical palette is defined in RGB (`Color::Rgb`).
//! At startup [`Theme::quantized`] downgrades every color to the terminal's detected capability level (256-color, 16-color, etc.).

use ratatui::style::{Color, Modifier};

use super::tokyonight::Theme;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

// Grayscale ramp anchored at bg #141414 / fg #f3f3f3. Accents are TokyoNight Night hex.
#[allow(dead_code)]
mod palette {
    use super::*;

    // ── Backgrounds ─────────────────────────────────────────────────────
    pub const BG: Color = rgb(6, 14, 26); //  #060e1a — terminal bg
    pub const BG_DARK: Color = rgb(8, 19, 34); //  #081322 — darkest navy
    pub const BG_STORM_DARK: Color = rgb(11, 27, 47); //  #0b1b2f — dark navy
    pub const BG_STORM: Color = rgb(16, 36, 59); //  #10243b — main dark blue
    pub const BG_HIGHLIGHT: Color = rgb(23, 59, 99); //  #173b63 — blue highlight

    // ── Text / grays ────────────────────────────────────────────────────
    pub const FG: Color = rgb(243, 247, 255); // #f3f7ff — primary text
    pub const FG_DARK: Color = rgb(190, 211, 235); // #bed3eb — secondary text
    pub const FG_GUTTER: Color = rgb(62, 96, 132); //  #3e6084 — dim blue
    pub const COMMENT: Color = rgb(112, 145, 181); //  #7091b5 — muted blue
    pub const DARK3: Color = rgb(87, 123, 161); //  #577ba1 — medium blue
    pub const DARK5: Color = rgb(145, 181, 220); // #91b5dc — bright blue-gray

    // ── Accent colors (TokyoNight Night) ─────────────────────────────────
    pub const BLUE: Color = rgb(122, 162, 247); // #7aa2f7
    pub const BLUE0: Color = rgb(61, 89, 161); // #3d59a1
    pub const BLUE1: Color = rgb(58, 149, 171); // #3A95AB
    pub const CYAN: Color = rgb(125, 207, 255); // #7dcfff
    pub const GREEN: Color = rgb(158, 206, 106); // #9ece6a
    pub const GREEN1: Color = rgb(115, 218, 202); // #73daca
    pub const MAGENTA: Color = rgb(86, 180, 255); // #56b4ff — Voltage electric blue
    pub const ORANGE: Color = rgb(255, 158, 100); // #ff9e64
    pub const PURPLE: Color = rgb(157, 124, 216); // #9d7cd8
    pub const RED: Color = rgb(247, 118, 142); // #f7768e
    pub const RED1: Color = rgb(219, 75, 75); // #db4b4b
    pub const TEAL: Color = rgb(26, 188, 156); // #1abc9c
    pub const YELLOW: Color = rgb(224, 175, 104); // #e0af68

    pub const RED_DARK: Color = rgb(66, 14, 20); // #420e14, quantizes to 256-color red, not gray
    pub const GREEN_DARK: Color = rgb(6, 56, 6); // #063806, quantizes to 256-color green, not gray
}
use palette::*;

impl Theme {
    pub const fn voltage_dark() -> Self {
        Self {
            bg_base: BG_STORM,
            bg_light: BG_HIGHLIGHT,
            bg_dark: rgb(28, 28, 28), // lighter than bg_base for visible code blocks
            bg_highlight: BG_HIGHLIGHT,
            bg_hover: rgb(44, 44, 44),
            bg_terminal: BG,

            accent_user: FG_DARK,
            accent_assistant: MAGENTA,
            accent_thinking: MAGENTA,
            accent_tool: DARK5,
            accent_system: BLUE,
            accent_error: RED,
            accent_success: GREEN,
            accent_running: MAGENTA,
            accent_skill: BLUE,

            text_primary: FG,
            text_secondary: FG_DARK,

            gray_dim: rgb(88, 88, 88), // #585858, slightly brighter than FG_GUTTER
            gray: COMMENT,
            gray_bright: DARK5,

            command: YELLOW,
            path: ORANGE,
            running: CYAN,
            warning: YELLOW,

            fuzzy_accent: BLUE,

            accent_plan: rgb(255, 219, 141), // #FFDB8D, golden

            accent_verify: rgb(187, 154, 247), // #bb9af7, violet

            accent_remember: Color::Rgb(139, 195, 74), // #8BC34A, Material Design light green

            selection_border: rgb(50, 108, 170),
            prompt_border: rgb(29, 66, 108), // #1d426c — dim blue prompt chrome
            prompt_border_active: rgb(65, 145, 220), // #4191dc — brighter when focused
            hover_border: rgb(24, 56, 94),

            accent_model: TEAL,

            scrollbar_bg: BG_STORM_DARK,
            scrollbar_fg: BG_HIGHLIGHT,

            diff_delete_bg: RED_DARK,
            diff_delete_fg: RED,
            diff_insert_bg: GREEN_DARK,
            diff_insert_fg: GREEN,
            diff_equal_fg: COMMENT,
            diff_gutter_fg: COMMENT,

            bg_visual: rgb(29, 67, 108),

            paste_bg: BG_STORM_DARK,
            paste_fg: FG_DARK,
            paste_dim: FG_GUTTER,

            md_heading_h1: TEAL,
            md_heading_h1_mod: Modifier::BOLD,
            md_heading_h2: BLUE,
            md_heading_h2_mod: Modifier::BOLD,
            md_heading_h3: PURPLE,
            md_heading_h3_mod: Modifier::BOLD,
            md_heading_h4: DARK5, // bright gray
            md_heading_h4_mod: Modifier::BOLD,
            md_heading_h5: COMMENT, // medium gray
            md_heading_h5_mod: Modifier::BOLD,
            md_heading_h6: DARK3, // medium gray, unbold
            md_heading_h6_mod: Modifier::empty(),
            md_code: BLUE1,
            md_task_checked: GREEN,
            md_task_unchecked: FG_DARK, // text_secondary
            md_muted: COMMENT,
            md_code_bg: rgb(12, 29, 50),
            md_text: FG_DARK,
            link_fg: rgb(104, 194, 255), // #68c2ff — electric blue for dark bg
        }
    }
}
