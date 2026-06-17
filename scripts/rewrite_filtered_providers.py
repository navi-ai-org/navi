EM = "\u2014"
path = 'crates/navi-tui/src/app.rs'
with open(path, 'r') as f:
    s = f.read()

# Find the start of the function and the closing `}\n}` that ends it.
start_marker = '    pub(crate) fn filtered_providers(&self) -> Vec<crate::providers::ProviderListRow> {'
end_marker = '        rows\n    }\n}\n'
# end_marker appears multiple places; we want the last one (end of file).
start = s.index(start_marker)
# Find the matching closing brace: scan forward to find `    }\n}` after start
# (the original closing of the impl block).
import re
# Simpler: take from start to the next `\n}\n\nfn detect_git_branch`
tail_start = s.index('\nfn detect_git_branch', start)
prefix = s[:start]
suffix = s[tail_start:]

new_fn = f'''    pub(crate) fn filtered_providers(&self) -> Vec<crate::providers::ProviderListRow> {{
        use crate::providers::ProviderListRow;

        let filter = self.provider_filter.trim().to_lowercase();
        let providers = provider_catalog(&self.loaded_config.config);
        let total = providers.len();

        let index_of = |id: &str| -> Option<usize> {{
            let canonical = navi_sdk::canonical_provider_id(id);
            providers
                .iter()
                .position(|p| navi_sdk::canonical_provider_id(&p.id) == canonical)
        }};

        if !filter.is_empty() {{
            return providers
                .into_iter()
                .enumerate()
                .filter(|(_, p)| {{
                    p.id.to_lowercase().contains(&filter)
                        || p.label.to_lowercase().contains(&filter)
                        || p.description.to_lowercase().contains(&filter)
                }})
                .map(|(index, _)| ProviderListRow::Provider {{ index }})
                .collect();
        }}

        let mut rows: Vec<ProviderListRow> = Vec::new();
        let mut emitted: Vec<bool> = vec![false; total];

        let recents: Vec<usize> = self
            .loaded_config
            .config
            .tui
            .recent_provider_ids
            .iter()
            .filter_map(|id| index_of(id))
            .filter(|idx| {{
                if emitted[*idx] {{
                    false
                }} else {{
                    emitted[*idx] = true;
                    true
                }}
            }})
            .collect();
        if !recents.is_empty() {{
            rows.push(ProviderListRow::Header {{
                label: format!("{EM} Recent {EM}"),
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
                label: format!("{EM} Connected {EM}"),
            }});
            for idx in connected {{
                rows.push(ProviderListRow::Provider {{ index: idx }});
            }}
        }}

        let others: Vec<usize> = (0..total).filter(|i| !emitted[*i]).collect();
        if !others.is_empty() {{
            rows.push(ProviderListRow::Header {{
                label: format!("{EM} Other providers {EM}"),
            }});
            for idx in others {{
                rows.push(ProviderListRow::Provider {{ index: idx }});
            }}
        }}

        rows
    }}
}}
'''

result = prefix + new_fn + suffix
with open(path, 'w') as f:
    f.write(result)
print('rewrote', len(s), '->', len(result))
