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
