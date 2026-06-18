use std::path::PathBuf;

use ratatui::layout::Rect;
use ratatui::prelude::Frame;
use ratatui_image::picker::Picker;

use crate::app::TuiApp;

const MASCOT_WIDTH: u16 = 20;
const MASCOT_HEIGHT: u16 = 7;
const MASCOT_RIGHT_OVERHANG: u16 = 5;
const ANIMATION_SPEED: u64 = 15; // ticks per frame (~250ms at 16fps)

pub(crate) struct MascotFrames {
    pub protocols: Vec<ratatui_image::protocol::StatefulProtocol>,
}

impl MascotFrames {
    pub fn load(picker: &Picker) -> Option<Self> {
        let dirs = [mascot_dir(), downloads_dir()];

        for dir in dirs.iter().flatten() {
            let mut frames: Vec<(usize, std::path::PathBuf)> = Vec::new();

            let entries = match std::fs::read_dir(dir) {
                Ok(e) => e,
                Err(_) => continue,
            };

            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if !name.starts_with("lain") || !name.ends_with(".png") {
                    continue;
                }
                if let Some(num) = name
                    .trim_start_matches("lain")
                    .trim_start_matches('-')
                    .trim_start_matches('_')
                    .chars()
                    .take_while(|c| c.is_ascii_digit())
                    .collect::<String>()
                    .parse::<usize>()
                    .ok()
                {
                    frames.push((num, entry.path()));
                }
            }

            frames.sort_by_key(|(n, _)| *n);
            frames.truncate(8);

            let protocols: Vec<_> = frames
                .iter()
                .filter_map(|(_, path)| {
                    let bytes = std::fs::read(path).ok()?;
                    let dyn_img = image::load_from_memory(&bytes).ok()?;
                    let protocol = picker.new_resize_protocol(dyn_img);
                    Some(protocol)
                })
                .collect();

            if protocols.len() >= 2 {
                tracing::info!(
                    dir = %dir.display(),
                    frames = protocols.len(),
                    "mascot frames loaded"
                );
                return Some(MascotFrames { protocols });
            }
        }

        tracing::debug!("mascot frames not found");
        None
    }

    pub fn frame_count(&self) -> usize {
        self.protocols.len()
    }
}

fn mascot_dir() -> Option<PathBuf> {
    config_dir().map(|d| d.join("mascot"))
}

fn downloads_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    Some(PathBuf::from(home).join("Downloads"))
}

fn config_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".config").join("navi");
    Some(dir)
}

/// Returns true if mascot should be visible.
pub(crate) fn mascot_visible(app: &TuiApp) -> bool {
    app.mascot_frames.is_some()
}

/// Returns the overlay area for the mascot, anchored to the input's right edge.
pub(super) fn mascot_overlay_area(app: &TuiApp, input_area: Rect) -> Option<Rect> {
    if !mascot_visible(app) {
        return None;
    }

    if input_area.width < 16 || input_area.height == 0 {
        return None;
    }

    let mascot_width = input_area.width.min(MASCOT_WIDTH);
    let overlay_bottom = input_area.y.saturating_add(input_area.height);
    let mascot_height = overlay_bottom.min(MASCOT_HEIGHT);
    let overlay_right = input_area
        .x
        .saturating_add(input_area.width)
        .saturating_add(MASCOT_RIGHT_OVERHANG);
    let mascot_area = Rect::new(
        overlay_right.saturating_sub(mascot_width),
        overlay_bottom.saturating_sub(mascot_height),
        mascot_width,
        mascot_height,
    );

    Some(mascot_area)
}

/// Render the mascot in the given area.
pub(super) fn render_mascot(frame: &mut Frame<'_>, app: &mut TuiApp, area: Rect) {
    if app.mascot_frames.is_none() {
        return;
    }

    let frame_count = app.mascot_frames.as_ref().unwrap().frame_count();
    let frame_index = if app.is_loading {
        app.mascot_frame_index % frame_count
    } else {
        0
    };

    if let Some(protocol) = app
        .mascot_frames
        .as_mut()
        .unwrap()
        .protocols
        .get_mut(frame_index)
    {
        let image_widget = ratatui_image::StatefulImage::new();
        frame.render_stateful_widget(image_widget, area, protocol);
    }
}

/// Advance mascot animation if loading.
pub(crate) fn advance_mascot_animation(app: &mut TuiApp) {
    if !app.is_loading || app.mascot_frames.is_none() {
        return;
    }

    app.mascot_tick_counter += 1;
    if app.mascot_tick_counter >= ANIMATION_SPEED {
        app.mascot_tick_counter = 0;
        let frame_count = app.mascot_frames.as_ref().unwrap().frame_count();
        app.mascot_frame_index = (app.mascot_frame_index + 1) % frame_count;
    }
}

/// Reset mascot animation to first frame.
pub(crate) fn reset_mascot_animation(app: &mut TuiApp) {
    app.mascot_frame_index = 0;
    app.mascot_tick_counter = 0;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mascot_visible_requires_frames() {
        let mut app = crate::tests::test_app("");
        app.is_loading = false;
        app.mascot_frames = None;
        assert!(!mascot_visible(&app));

        app.is_loading = true;
        assert!(!mascot_visible(&app));

        app.is_loading = false;
        assert!(!mascot_visible(&app));
    }

    #[test]
    fn overlay_area_returns_none_without_frames() {
        let app = crate::tests::test_app("");
        let area = Rect::new(0, 0, 80, 12);
        assert!(mascot_overlay_area(&app, area).is_none());
    }

    #[test]
    fn overlay_area_returns_none_when_no_frames_even_if_loading() {
        let mut app = crate::tests::test_app("");
        app.is_loading = true;
        let area = Rect::new(0, 0, 30, 12);
        assert!(mascot_overlay_area(&app, area).is_none());
    }

    #[test]
    fn advance_mascot_animation_respects_speed() {
        let mut app = crate::tests::test_app("");
        app.is_loading = true;
        // No mascot frames, so animation doesn't advance
        advance_mascot_animation(&mut app);
        assert_eq!(app.mascot_tick_counter, 0);
        assert_eq!(app.mascot_frame_index, 0);
    }
}
