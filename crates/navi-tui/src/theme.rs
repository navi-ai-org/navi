use ratatui::style::{Color, Modifier, Style};
use std::cell::RefCell;
use std::time::Duration;

/// Built-in color themes for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ThemeId {
    Default,
    Lain,
    Terminal,
    Slate,
    Ember,
    Paper,
    OscuraNight,
}

impl ThemeId {
    pub(crate) const ALL: [ThemeId; 7] = [
        ThemeId::Default,
        ThemeId::Lain,
        ThemeId::Terminal,
        ThemeId::Slate,
        ThemeId::Ember,
        ThemeId::Paper,
        ThemeId::OscuraNight,
    ];

    pub(crate) fn label(self) -> &'static str {
        match self {
            ThemeId::Default => "Default",
            ThemeId::Lain => "Lain",
            ThemeId::Terminal => "Terminal",
            ThemeId::Slate => "Slate",
            ThemeId::Ember => "Ember",
            ThemeId::Paper => "Paper",
            ThemeId::OscuraNight => "Oscura Night",
        }
    }

    pub(crate) fn from_config(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "default" | "system" | "auto" => ThemeId::Default,
            "terminal" | "green" => ThemeId::Terminal,
            "slate" | "blue" => ThemeId::Slate,
            "ember" | "red" => ThemeId::Ember,
            "paper" | "light" => ThemeId::Paper,
            "oscura-night" | "oscura_night" | "oscura" | "night" => ThemeId::OscuraNight,
            _ => ThemeId::Default,
        }
    }

    pub(crate) fn config_value(self) -> &'static str {
        match self {
            ThemeId::Default => "default",
            ThemeId::Lain => "lain",
            ThemeId::Terminal => "terminal",
            ThemeId::Slate => "slate",
            ThemeId::Ember => "ember",
            ThemeId::Paper => "paper",
            ThemeId::OscuraNight => "oscura-night",
        }
    }

    #[cfg(test)]
    pub(crate) fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|id| *id == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    pub(crate) fn palette(self) -> ThemePalette {
        match self {
            ThemeId::Default => ThemePalette::default_terminal(),
            ThemeId::Lain => ThemePalette::lain(),
            ThemeId::Terminal => ThemePalette::terminal(),
            ThemeId::Slate => ThemePalette::slate(),
            ThemeId::Ember => ThemePalette::ember(),
            ThemeId::Paper => ThemePalette::paper(),
            ThemeId::OscuraNight => ThemePalette::oscura_night(),
        }
    }
}

pub(crate) fn filtered_theme_options(filter: &str) -> Vec<(usize, ThemeId)> {
    if filter.is_empty() {
        return ThemeId::ALL.iter().copied().enumerate().collect();
    }

    let filter = filter.to_lowercase();
    ThemeId::ALL
        .iter()
        .copied()
        .enumerate()
        .filter(|(_, theme)| theme.label().to_lowercase().contains(&filter))
        .collect()
}

#[derive(Debug, Clone)]
pub(crate) struct ThemePalette {
    pub accent: Color,
    pub red: Color,
    pub pink: Color,
    pub signal: Color,
    pub text: Color,
    pub muted: Color,
    pub panel: Color,
    pub bg: Color,
    pub ghost: Color,
    pub user_accent: Color,
    pub code_keyword: Color,
    pub code_string: Color,
    pub code_comment: Color,
    pub code_number: Color,
    pub code_punct: Color,
    pub code_type: Color,
    pub code_func: Color,
    pub code_const: Color,
    pub code_operator: Color,
    pub selection_fg: Color,
    pub selection_bg: Color,
}

impl ThemePalette {
    fn default_terminal() -> Self {
        Self {
            accent: Color::Cyan,
            red: Color::Red,
            pink: Color::Magenta,
            signal: Color::White,
            text: Color::Reset,
            muted: Color::Gray,
            panel: Color::Reset,
            bg: Color::Reset,
            ghost: Color::DarkGray,
            user_accent: Color::Cyan,
            code_keyword: Color::Blue,
            code_string: Color::Green,
            code_comment: Color::DarkGray,
            code_number: Color::Cyan,
            code_punct: Color::Reset,
            code_type: Color::Cyan,
            code_func: Color::White,
            code_const: Color::Yellow,
            code_operator: Color::Magenta,
            selection_fg: Color::Black,
            selection_bg: Color::White,
        }
    }

    fn lain() -> Self {
        Self {
            accent: rgb(178, 132, 255),
            red: rgb(255, 112, 194),
            pink: rgb(196, 154, 255),
            signal: rgb(236, 232, 255),
            text: rgb(224, 226, 232),
            muted: rgb(170, 174, 188),
            panel: rgb(16, 18, 22),
            bg: rgb(0, 0, 0),
            ghost: rgb(67, 72, 84),
            user_accent: rgb(154, 124, 205),
            code_keyword: rgb(116, 214, 232),
            code_string: rgb(196, 154, 255),
            code_comment: rgb(140, 146, 160),
            code_number: rgb(141, 211, 255),
            code_punct: rgb(180, 188, 202),
            code_type: rgb(100, 213, 235),
            code_func: rgb(218, 204, 255),
            code_const: rgb(255, 204, 128),
            code_operator: rgb(255, 150, 210),
            selection_fg: rgb(0, 0, 0),
            selection_bg: rgb(236, 232, 255),
        }
    }

    fn terminal() -> Self {
        Self {
            accent: rgb(124, 255, 178),
            red: rgb(255, 92, 92),
            pink: rgb(143, 232, 173),
            signal: rgb(210, 255, 228),
            text: rgb(230, 255, 240),
            muted: rgb(150, 188, 164),
            panel: rgb(11, 18, 14),
            bg: rgb(5, 8, 6),
            ghost: rgb(36, 58, 44),
            user_accent: rgb(124, 255, 178),
            code_keyword: rgb(124, 255, 178),
            code_string: rgb(198, 255, 214),
            code_comment: rgb(110, 148, 122),
            code_number: rgb(160, 220, 255),
            code_punct: rgb(160, 210, 178),
            code_type: rgb(111, 214, 255),
            code_func: rgb(190, 255, 210),
            code_const: rgb(255, 199, 112),
            code_operator: rgb(255, 140, 180),
            selection_fg: rgb(5, 8, 6),
            selection_bg: rgb(210, 255, 228),
        }
    }

    fn slate() -> Self {
        Self {
            accent: rgb(143, 179, 255),
            red: rgb(255, 120, 120),
            pink: rgb(180, 200, 255),
            signal: rgb(220, 228, 242),
            text: rgb(237, 239, 242),
            muted: rgb(176, 184, 198),
            panel: rgb(23, 27, 34),
            bg: rgb(14, 17, 22),
            ghost: rgb(48, 56, 68),
            user_accent: rgb(143, 179, 255),
            code_keyword: rgb(143, 179, 255),
            code_string: rgb(198, 210, 255),
            code_comment: rgb(132, 140, 156),
            code_number: rgb(160, 200, 255),
            code_punct: rgb(186, 196, 218),
            code_type: rgb(120, 200, 255),
            code_func: rgb(190, 210, 255),
            code_const: rgb(255, 199, 112),
            code_operator: rgb(255, 165, 195),
            selection_fg: rgb(14, 17, 22),
            selection_bg: rgb(220, 228, 242),
        }
    }

    fn ember() -> Self {
        Self {
            accent: rgb(196, 49, 49),
            red: rgb(255, 92, 72),
            pink: rgb(217, 208, 195),
            signal: rgb(234, 220, 210),
            text: rgb(234, 234, 234),
            muted: rgb(170, 160, 150),
            panel: rgb(17, 17, 17),
            bg: rgb(8, 8, 8),
            ghost: rgb(55, 55, 55),
            user_accent: rgb(196, 49, 49),
            code_keyword: rgb(255, 120, 96),
            code_string: rgb(217, 208, 195),
            code_comment: rgb(142, 142, 142),
            code_number: rgb(255, 180, 120),
            code_punct: rgb(178, 168, 158),
            code_type: rgb(255, 160, 120),
            code_func: rgb(255, 200, 160),
            code_const: rgb(255, 199, 112),
            code_operator: rgb(255, 120, 120),
            selection_fg: rgb(8, 8, 8),
            selection_bg: rgb(234, 220, 210),
        }
    }

    fn paper() -> Self {
        Self {
            accent: rgb(52, 99, 235),
            red: rgb(200, 60, 60),
            pink: rgb(90, 110, 200),
            signal: rgb(40, 50, 70),
            text: rgb(24, 28, 36),
            muted: rgb(82, 90, 108),
            panel: rgb(244, 246, 250),
            bg: rgb(252, 252, 253),
            ghost: rgb(210, 216, 228),
            user_accent: rgb(52, 99, 235),
            code_keyword: rgb(52, 99, 235),
            code_string: rgb(26, 110, 72),
            code_comment: rgb(130, 138, 152),
            code_number: rgb(16, 100, 140),
            code_punct: rgb(90, 100, 120),
            code_type: rgb(0, 120, 140),
            code_func: rgb(80, 60, 180),
            code_const: rgb(150, 90, 0),
            code_operator: rgb(180, 60, 100),
            selection_fg: rgb(252, 252, 253),
            selection_bg: rgb(40, 50, 70),
        }
    }

    /// Deep night: navy-black base, moonlight accent, cool syntax.
    fn oscura_night() -> Self {
        Self {
            accent: rgb(157, 175, 220),
            red: rgb(255, 108, 118),
            pink: rgb(178, 188, 228),
            signal: rgb(208, 216, 238),
            text: rgb(226, 230, 242),
            muted: rgb(158, 166, 190),
            panel: rgb(16, 18, 28),
            bg: rgb(8, 9, 16),
            ghost: rgb(38, 42, 58),
            user_accent: rgb(157, 175, 220),
            code_keyword: rgb(136, 178, 255),
            code_string: rgb(184, 196, 228),
            code_comment: rgb(118, 128, 152),
            code_number: rgb(118, 198, 218),
            code_punct: rgb(168, 178, 206),
            code_type: rgb(156, 208, 255),
            code_func: rgb(198, 208, 244),
            code_const: rgb(255, 208, 138),
            code_operator: rgb(210, 175, 255),
            selection_fg: rgb(8, 9, 16),
            selection_bg: rgb(208, 216, 238),
        }
    }
}

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

pub(crate) const NOTIFICATION_TTL: Duration = Duration::from_secs(2);

pub(crate) const NAVI_COMPACT_LOGO: &[&str] = &[
    r"███╗   ██╗ █████╗ ██╗   ██╗██╗",
    r"████╗  ██║██╔══██╗██║   ██║██║",
    r"██╔██╗ ██║███████║██║   ██║██║",
    r"██║╚██╗██║██╔══██║╚██╗ ██╔╝██║",
    r"██║ ╚████║██║  ██║ ╚████╔╝ ██║",
    r"╚═╝  ╚═══╝╚═╝  ╚═╝  ╚═══╝  ╚═╝",
];

thread_local! {
    static ACTIVE: RefCell<ThemePalette> = RefCell::new(ThemePalette::lain());
}

/// Run UI rendering with the given palette active (call from the root `render`).
pub(crate) fn with_palette<F, R>(palette: &ThemePalette, f: F) -> R
where
    F: FnOnce() -> R,
{
    ACTIVE.with(|slot| {
        *slot.borrow_mut() = palette.clone();
        f()
    })
}

fn p() -> ThemePalette {
    ACTIVE.with(|slot| slot.borrow().clone())
}

pub(crate) fn accent() -> Color {
    p().accent
}
pub(crate) fn red() -> Color {
    p().red
}
pub(crate) fn pink() -> Color {
    p().pink
}
pub(crate) fn signal() -> Color {
    p().signal
}
pub(crate) fn text() -> Color {
    p().text
}
pub(crate) fn muted() -> Color {
    p().muted
}
pub(crate) fn panel() -> Color {
    p().panel
}
/// Surface color for modals. Returns `Color::Reset` if the theme panel color is unset to preserve transparency.
pub(crate) fn modal_bg() -> Color {
    p().panel
}
/// Foreground for modal surfaces.
pub(crate) fn modal_fg() -> Color {
    text()
}
pub(crate) fn modal_style() -> Style {
    Style::default().fg(modal_fg()).bg(modal_bg())
}

pub(crate) fn bg() -> Color {
    p().bg
}
pub(crate) fn ghost() -> Color {
    p().ghost
}
pub(crate) fn user_accent() -> Color {
    p().user_accent
}
pub(crate) fn code_keyword() -> Color {
    p().code_keyword
}
pub(crate) fn code_string() -> Color {
    p().code_string
}
pub(crate) fn code_comment() -> Color {
    p().code_comment
}
pub(crate) fn code_number() -> Color {
    p().code_number
}
pub(crate) fn code_punct() -> Color {
    p().code_punct
}
pub(crate) fn code_type() -> Color {
    p().code_type
}
pub(crate) fn code_func() -> Color {
    p().code_func
}
pub(crate) fn code_const() -> Color {
    p().code_const
}
pub(crate) fn code_operator() -> Color {
    p().code_operator
}

pub(crate) fn selection_fg() -> Color {
    p().selection_fg
}

pub(crate) fn selection_bg() -> Color {
    p().selection_bg
}

pub(crate) fn code_block_bg() -> Color {
    Color::Rgb(34, 36, 54)
}

pub(crate) fn active_item_style() -> Style {
    let palette = p();
    Style::default()
        .fg(palette.selection_fg)
        .bg(palette.selection_bg)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn inactive_item_style() -> Style {
    Style::default().fg(modal_fg()).bg(modal_bg())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn theme_cycle_covers_all() {
        let mut id = ThemeId::Default;
        for _ in 0..ThemeId::ALL.len() {
            id = id.next();
        }
        assert_eq!(id, ThemeId::Default);
    }

    #[test]
    fn config_aliases_resolve() {
        assert_eq!(ThemeId::from_config("green"), ThemeId::Terminal);
        assert_eq!(ThemeId::from_config("oscura-night"), ThemeId::OscuraNight);
        assert_eq!(ThemeId::from_config("oscura"), ThemeId::OscuraNight);
        assert_eq!(ThemeId::from_config("unknown"), ThemeId::Default);
    }
}
