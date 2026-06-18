use crossterm::event::{KeyCode, KeyModifiers};
use ratatui::layout::Rect;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HitAction {
    Key {
        code: KeyCode,
        modifiers: KeyModifiers,
    },
    CloseModal,
    ReopenQuestion,
    QuestionOption(usize),
    QuestionText,
    QuestionDeny,
    QuestionSend,
    Command(usize),
    Model(usize),
    ModelProviderRefresh(String),
    ProviderApiKey(usize),
    ProviderOAuth(usize),
    OAuthOpen,
    Session(usize),
    Skill(usize),
    Setting(usize),
    PluginInstallOrUpdate(usize),
    PluginRefresh,
    McpServer(usize),
    ToolApprove,
    ToolDeny,
    PluginApprove,
    PluginDeny,
    ThemeSelect(usize),
    ThemePicker,
    ChatMessage(usize),
    ToolResult(String),
    ToolGroup(Vec<String>),
    MessageAction(usize),
    ScrollTo {
        target: ScrollTarget,
        offset: usize,
    },
    RemoveImage(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ScrollTarget {
    Commands,
    Models,
    Providers,
    Sessions,
    Skills,
    Plugins,
    PluginApproval,
    QuestionOptions,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HitRegion {
    pub id: String,
    pub rect: Rect,
    pub z: i16,
    pub label: String,
    pub action: HitAction,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct InteractionRegistry {
    regions: Vec<HitRegion>,
    next_id: u64,
}

impl InteractionRegistry {
    pub(crate) fn clear(&mut self) {
        self.regions.clear();
        self.next_id = 0;
    }

    pub(crate) fn register(
        &mut self,
        rect: Rect,
        z: i16,
        label: impl Into<String>,
        action: HitAction,
    ) -> String {
        let id = format!("hit-{}", self.next_id);
        self.next_id = self.next_id.saturating_add(1);
        if rect.width == 0 || rect.height == 0 {
            return id;
        }
        self.regions.push(HitRegion {
            id: id.clone(),
            rect,
            z,
            label: label.into(),
            action,
        });
        id
    }

    pub(crate) fn hit(&self, col: u16, row: u16) -> Option<HitRegion> {
        self.regions
            .iter()
            .rev()
            .filter(|region| contains(region.rect, col, row))
            .max_by_key(|region| region.z)
            .cloned()
    }
}

fn contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x
        && col < rect.x.saturating_add(rect.width)
        && row >= rect.y
        && row < rect.y.saturating_add(rect.height)
}

pub(crate) fn line_rect(area: Rect, row_offset: usize) -> Rect {
    if row_offset >= area.height as usize {
        return Rect::new(area.x, area.y.saturating_add(area.height), area.width, 0);
    }
    Rect::new(area.x, area.y + row_offset as u16, area.width, 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hit_returns_topmost_matching_region() {
        let mut registry = InteractionRegistry::default();
        registry.register(Rect::new(0, 0, 10, 10), 1, "low", HitAction::ReopenQuestion);
        registry.register(Rect::new(2, 2, 3, 3), 5, "high", HitAction::ReopenQuestion);

        assert!(matches!(
            registry.hit(3, 3).map(|hit| hit.action),
            Some(HitAction::ReopenQuestion)
        ));
        assert!(matches!(
            registry.hit(8, 8).map(|hit| hit.action),
            Some(HitAction::ReopenQuestion)
        ));
    }
}
