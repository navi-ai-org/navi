#[derive(Default)]
pub(crate) struct SseDecoder {
    buffer: String,
}

impl SseDecoder {
    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        let mut events = Vec::new();

        while let Some(index) = self.buffer.find("\n\n") {
            let raw = self.buffer[..index].to_string();
            self.buffer.drain(..index + 2);
            if let Some(data) = sse_data(&raw) {
                events.push(data);
            }
        }

        events
    }
}

fn sse_data(raw: &str) -> Option<String> {
    let data = raw
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .collect::<Vec<_>>()
        .join("\n");

    (!data.is_empty()).then_some(data)
}
