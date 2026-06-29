pub fn slugify(input: &str) -> String {
    input
        .trim()
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_whitespace() { '-' } else { ch })
        .collect()
}
