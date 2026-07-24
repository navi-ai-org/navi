#[derive(Default)]
pub(crate) struct SseDecoder {
    buffer: String,
    seen_sse_frame: bool,
}

impl SseDecoder {
    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(bytes));
        let mut events = Vec::new();

        loop {
            if let Some(index) = self.buffer.find("\n\n") {
                let raw = self.buffer[..index].to_string();
                self.buffer.drain(..index + 2);
                if let Some(data) = sse_data(&raw) {
                    self.seen_sse_frame = true;
                    events.push(data);
                }
                continue;
            }

            if self.seen_sse_frame {
                break;
            }

            let next_newline = self.buffer.find('\n');
            match next_newline {
                Some(index) => {
                    let line = self.buffer[..index].to_string();
                    if is_sse_field_line(line.trim()) {
                        break;
                    }
                    self.buffer.drain(..index + 1);
                    let trimmed = line.trim();
                    if !trimmed.is_empty() {
                        events.push(trimmed.to_string());
                    }
                }
                None => break,
            }
        }

        events
    }

    #[cfg(test)]
    pub(crate) fn drain(&mut self) -> Vec<String> {
        let raw = std::mem::take(&mut self.buffer);
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Vec::new();
        }

        if let Some(data) = sse_data(trimmed) {
            self.seen_sse_frame = true;
            return vec![data];
        }

        if self.seen_sse_frame || is_sse_field_line(trimmed) {
            Vec::new()
        } else {
            vec![trimmed.to_string()]
        }
    }
}

fn is_sse_field_line(line: &str) -> bool {
    line.starts_with("data:")
        || line.starts_with("event:")
        || line.starts_with("id:")
        || line.starts_with("retry:")
        || line.starts_with(':')
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
