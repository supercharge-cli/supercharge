//! Additional truecolor palettes for the CLI.

use ratatui::style::Color;

use super::Theme;

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

const fn tint(base: Color, accent: Color) -> Color {
    let (Color::Rgb(br, bg, bb), Color::Rgb(ar, ag, ab)) = (base, accent) else {
        return base;
    };
    rgb(
        ((br as u16 * 7 + ar as u16) / 8) as u8,
        ((bg as u16 * 7 + ag as u16) / 8) as u8,
        ((bb as u16 * 7 + ab as u16) / 8) as u8,
    )
}

#[derive(Clone, Copy)]
struct Palette {
    bg: Color,
    raised: Color,
    dark: Color,
    highlight: Color,
    hover: Color,
    fg: Color,
    subdued: Color,
    muted: Color,
    dim: Color,
    bright: Color,
    accent: Color,
    secondary: Color,
    green: Color,
    red: Color,
    yellow: Color,
    cyan: Color,
}

impl Theme {
    const fn from_palette(p: Palette, light: bool) -> Self {
        let mut t = if light {
            Self::voltage_light()
        } else {
            Self::voltage_dark()
        };
        t.bg_base = p.bg;
        t.bg_light = p.raised;
        t.bg_dark = p.dark;
        t.bg_highlight = p.highlight;
        t.bg_hover = p.hover;
        t.bg_terminal = p.dark;
        t.bg_visual = tint(p.highlight, p.accent);
        t.text_primary = p.fg;
        t.text_secondary = p.subdued;
        t.gray = p.muted;
        t.gray_dim = p.dim;
        t.gray_bright = p.bright;
        t.accent_user = p.accent;
        t.accent_assistant = p.secondary;
        t.accent_thinking = p.muted;
        t.accent_tool = p.cyan;
        t.accent_system = p.secondary;
        t.accent_error = p.red;
        t.accent_success = p.green;
        t.accent_running = p.cyan;
        t.accent_skill = p.accent;
        t.command = p.yellow;
        t.path = p.secondary;
        t.running = p.cyan;
        t.warning = p.yellow;
        t.fuzzy_accent = p.accent;
        t.accent_plan = p.yellow;
        t.accent_verify = p.secondary;
        t.accent_remember = p.green;
        t.selection_border = p.accent;
        t.hover_border = p.secondary;
        t.prompt_border = p.muted;
        t.prompt_border_active = p.accent;
        t.accent_model = p.accent;
        t.scrollbar_bg = p.dark;
        t.scrollbar_fg = p.muted;
        t.diff_delete_bg = tint(p.bg, p.red);
        t.diff_delete_fg = p.red;
        t.diff_insert_bg = tint(p.bg, p.green);
        t.diff_insert_fg = p.green;
        t.diff_equal_fg = p.muted;
        t.diff_gutter_fg = p.muted;
        t.paste_bg = p.dark;
        t.paste_fg = p.subdued;
        t.paste_dim = p.dim;
        t.md_heading_h1 = p.accent;
        t.md_heading_h2 = p.secondary;
        t.md_heading_h3 = p.cyan;
        t.md_heading_h4 = p.yellow;
        t.md_heading_h5 = p.bright;
        t.md_heading_h6 = p.muted;
        t.md_code = p.secondary;
        t.md_task_checked = p.green;
        t.md_task_unchecked = p.subdued;
        t.md_muted = p.muted;
        t.md_code_bg = p.dark;
        t.md_text = p.fg;
        t.link_fg = p.accent;
        t
    }

    pub const fn catppuccin_mocha() -> Self {
        Self::from_palette(
            Palette {
                bg: rgb(30, 30, 46),
                raised: rgb(49, 50, 68),
                dark: rgb(24, 24, 37),
                highlight: rgb(69, 71, 90),
                hover: rgb(58, 60, 78),
                fg: rgb(205, 214, 244),
                subdued: rgb(186, 194, 222),
                muted: rgb(147, 153, 178),
                dim: rgb(108, 112, 134),
                bright: rgb(166, 173, 200),
                accent: rgb(203, 166, 247),
                secondary: rgb(245, 194, 231),
                green: rgb(166, 227, 161),
                red: rgb(243, 139, 168),
                yellow: rgb(249, 226, 175),
                cyan: rgb(137, 220, 235),
            },
            false,
        )
    }

    pub const fn gruvbox_dark() -> Self {
        Self::from_palette(
            Palette {
                bg: rgb(40, 40, 40),
                raised: rgb(60, 56, 54),
                dark: rgb(29, 32, 33),
                highlight: rgb(80, 73, 69),
                hover: rgb(70, 65, 60),
                fg: rgb(235, 219, 178),
                subdued: rgb(213, 196, 161),
                muted: rgb(168, 153, 132),
                dim: rgb(146, 131, 116),
                bright: rgb(189, 174, 147),
                accent: rgb(250, 189, 47),
                secondary: rgb(214, 93, 14),
                green: rgb(184, 187, 38),
                red: rgb(251, 73, 52),
                yellow: rgb(250, 189, 47),
                cyan: rgb(142, 192, 124),
            },
            false,
        )
    }

    pub const fn nord() -> Self {
        Self::from_palette(
            Palette {
                bg: rgb(46, 52, 64),
                raised: rgb(59, 66, 82),
                dark: rgb(36, 41, 51),
                highlight: rgb(67, 76, 94),
                hover: rgb(62, 70, 87),
                fg: rgb(236, 239, 244),
                subdued: rgb(216, 222, 233),
                muted: rgb(160, 170, 188),
                dim: rgb(126, 140, 160),
                bright: rgb(190, 199, 214),
                accent: rgb(136, 192, 208),
                secondary: rgb(180, 142, 173),
                green: rgb(163, 190, 140),
                red: rgb(191, 97, 106),
                yellow: rgb(235, 203, 139),
                cyan: rgb(143, 188, 187),
            },
            false,
        )
    }

    pub const fn solarized_dark() -> Self {
        Self::from_palette(
            Palette {
                bg: rgb(0, 43, 54),
                raised: rgb(7, 54, 66),
                dark: rgb(0, 32, 40),
                highlight: rgb(16, 70, 79),
                hover: rgb(11, 62, 73),
                fg: rgb(238, 232, 213),
                subdued: rgb(147, 161, 161),
                muted: rgb(131, 148, 150),
                dim: rgb(101, 123, 131),
                bright: rgb(181, 195, 188),
                accent: rgb(38, 139, 210),
                secondary: rgb(42, 161, 152),
                green: rgb(133, 153, 0),
                red: rgb(220, 50, 47),
                yellow: rgb(181, 137, 0),
                cyan: rgb(42, 161, 152),
            },
            false,
        )
    }

    pub const fn dracula() -> Self {
        Self::from_palette(
            Palette {
                bg: rgb(40, 42, 54),
                raised: rgb(52, 55, 70),
                dark: rgb(33, 34, 44),
                highlight: rgb(68, 71, 90),
                hover: rgb(60, 63, 79),
                fg: rgb(248, 248, 242),
                subdued: rgb(220, 221, 226),
                muted: rgb(164, 166, 180),
                dim: rgb(112, 116, 139),
                bright: rgb(190, 192, 205),
                accent: rgb(189, 147, 249),
                secondary: rgb(255, 121, 198),
                green: rgb(80, 250, 123),
                red: rgb(255, 85, 85),
                yellow: rgb(241, 250, 140),
                cyan: rgb(139, 233, 253),
            },
            false,
        )
    }

    pub const fn one_dark() -> Self {
        Self::from_palette(
            Palette {
                bg: rgb(40, 44, 52),
                raised: rgb(51, 56, 65),
                dark: rgb(33, 37, 43),
                highlight: rgb(62, 68, 78),
                hover: rgb(56, 62, 71),
                fg: rgb(220, 223, 228),
                subdued: rgb(190, 195, 203),
                muted: rgb(143, 151, 163),
                dim: rgb(105, 114, 126),
                bright: rgb(169, 176, 186),
                accent: rgb(97, 175, 239),
                secondary: rgb(198, 120, 221),
                green: rgb(152, 195, 121),
                red: rgb(224, 108, 117),
                yellow: rgb(229, 192, 123),
                cyan: rgb(86, 182, 194),
            },
            false,
        )
    }

    pub const fn solarized_light() -> Self {
        Self::from_palette(
            Palette {
                bg: rgb(253, 246, 227),
                raised: rgb(238, 232, 213),
                dark: rgb(242, 235, 217),
                highlight: rgb(224, 218, 199),
                hover: rgb(232, 226, 207),
                fg: rgb(0, 43, 54),
                subdued: rgb(88, 110, 117),
                muted: rgb(101, 123, 131),
                dim: rgb(131, 148, 150),
                bright: rgb(73, 94, 102),
                accent: rgb(38, 112, 168),
                secondary: rgb(108, 113, 196),
                green: rgb(90, 116, 0),
                red: rgb(189, 54, 49),
                yellow: rgb(150, 108, 0),
                cyan: rgb(0, 132, 122),
            },
            true,
        )
    }
}
