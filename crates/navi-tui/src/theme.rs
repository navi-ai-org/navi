use ratatui::style::Color;
use std::time::Duration;

pub(crate) const ACCENT: Color = Color::Rgb(176, 34, 255);
pub(crate) const RED: Color = Color::Rgb(218, 64, 255);
pub(crate) const PINK: Color = Color::Rgb(194, 31, 255);
pub(crate) const SIGNAL: Color = Color::Rgb(236, 218, 255);
pub(crate) const TEXT: Color = Color::Rgb(245, 239, 255);
pub(crate) const MUTED: Color = Color::Rgb(150, 128, 166);
pub(crate) const PANEL: Color = Color::Rgb(19, 13, 26);
pub(crate) const BG: Color = Color::Rgb(9, 5, 13);
pub(crate) const GHOST: Color = Color::Rgb(58, 38, 74);
pub(crate) const USER_ACCENT: Color = Color::Rgb(176, 34, 255);
pub(crate) const CODE_KEYWORD: Color = Color::Rgb(220, 96, 255);
pub(crate) const CODE_STRING: Color = Color::Rgb(205, 166, 255);
pub(crate) const CODE_COMMENT: Color = Color::Rgb(124, 100, 146);
pub(crate) const CODE_NUMBER: Color = Color::Rgb(160, 220, 255);
pub(crate) const CODE_PUNCT: Color = Color::Rgb(185, 145, 220);
pub(crate) const CODE_TYPE: Color = Color::Rgb(111, 214, 255);
pub(crate) const CODE_FUNC: Color = Color::Rgb(190, 146, 255);
pub(crate) const CODE_CONST: Color = Color::Rgb(255, 199, 112);
pub(crate) const CODE_OPERATOR: Color = Color::Rgb(255, 118, 214);
pub(crate) const NOTIFICATION_TTL: Duration = Duration::from_secs(2);

pub(crate) const NAVI_COMPACT_LOGO: &[&str] = &[
    r"███╗   ██╗ █████╗ ██╗   ██╗██╗",
    r"████╗  ██║██╔══██╗██║   ██║██║",
    r"██╔██╗ ██║███████║██║   ██║██║",
    r"██║╚██╗██║██╔══██║╚██╗ ██╔╝██║",
    r"██║ ╚████║██║  ██║ ╚████╔╝ ██║",
    r"╚═╝  ╚═══╝╚═╝  ╚═╝  ╚═══╝  ╚═╝",
];
