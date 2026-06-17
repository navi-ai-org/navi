import sys
path = 'crates/navi-tui/src/app.rs'
with open(path, 'r') as f:
    s = f.read()
old = '''    pub(crate) fn filtered_providers(&self) -> Vec<ProviderConfig> {
        let filter = self.provider_filter.trim().to_lowercase();
        let providers = provider_catalog(&self.loaded_config.config);
        if filter.is_empty() {
            return providers;
        }
        providers
            .into_iter()
            .filter(|p| {
                p.id.to_lowercase().contains(&filter)
                    || p.label.to_lowercase().contains(&filter)
                    || p.description.to_lowercase().contains(&filter)
            })
            .collect()
    }
}'''
EM = "\u2014"
new = '''    pub(crate) fn filtered_providers(&self) -> Vec<crate::providers::ProviderListRow> {
        use crate::providers::ProviderListRow;

        let filter = self.provider_filter.trim().to_lowercase();
        let providers = provider_catalog(&self.loaded_config.config);
        let total = providers.len();

        let index_of = |id: &str| -> Option<usize> {
            let canonical = navi_sdk::canonical_provider_id(id);
            providers
                .iter()
                .position(|p| navi_sdk::canonical_provider_id(&p.id) == canonical)
        };

        if !filter.is_empty() {
            return providers
                .into_iter()
                .enumerate()
                .filter(|(_, p)| {
                    p.id.to_lowercase().contains(&filter)
                        || p.label.to_lowercase().contains(&filter)
                        || p.description.to_lowercase().contains(&filter)
                })
                .map(|(index, _)| ProviderListRow::Provider { index })
                .collect();
        }

        let mut rows: Vec<ProviderListRow> = Vec::new();
        let mut emitted: Vec<bool> = vec![false; total];

        let recents: Vec<usize> = self
            .loaded_config
            .config
            .tui
            .recent_provider_ids
            .iter()
            .filter_map(|id| index_of(id))
            .filter(|idx| {
                if emitted[*idx] {
                    false
                } else {
                    emitted[*idx] = true;
                    true
                }
            })
            .collect();
        if !recents.is_empty() {{
            rows.push(ProviderListRow::Header {{
                label: format!("{em} Recent {em}"),
            }});
            for idx in &recents {{
                rows.push(ProviderListRow::Provider {{ index: *idx }});
            }}
        }}

        let connected: Vec<usize> = providers
            .iter()
            .enumerate()
            .filter_map(|(index, p)| {{
                if !self.authenticated_providers.contains(p.id.as_str()) {{
                    return None;
                }}
                if emitted[index] {{
                    return None;
                }}
                emitted[index] = true;
                Some(index)
            }})
            .collect();
        if !connected.is_empty() {{
            rows.push(ProviderListRow::Header {{
                label: format!("{em} Connected {em}"),
            }});
            for idx in connected {{
                rows.push(ProviderListRow::Provider {{ index: idx }});
            }}
        }}

        let others: Vec<usize> = (0..total).filter(|i| !emitted[*i]).collect();
        if !others.is_empty() {{
            rows.push(ProviderListRow::Header {{
                label: format!("{em} Other providers {em}"),
            }});
            for idx in others {{
                rows.push(ProviderListRow::Provider {{ index: idx }});
            }}
        }}

        rows
    }
}'''
if old not in s:
    print('OLD NOT FOUND', file=sys.stderr)
    sys.exit(1)
s2 = s.replace(old, new, 1)
if s2 == s:
    print('NO CHANGE', file=sys.stderr)
    sys.exit(2)
with open(path, 'w') as f:
    f.write(s2)
print('OK', len(s), '->', len(s2))
