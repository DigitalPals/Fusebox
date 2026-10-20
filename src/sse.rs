//! Minimal incremental Server-Sent-Events decoder / encoder.

#[derive(Debug, Clone, Default)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
}

pub const LIMIT_ERROR: &str = "upstream SSE event exceeds configured stream limit";

pub struct SseDecoder {
    buf: Vec<u8>,
    limit: usize,
    failed: bool,
    event: Option<String>,
    data: String,
    has_data: bool,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::with_limit(16 << 20)
    }
}
impl SseDecoder {
    pub fn with_limit(limit: usize) -> Self {
        Self { buf: Vec::new(), limit, failed: false, event: None, data: String::new(), has_data: false }
    }
    pub fn failed(&self) -> bool {
        self.failed
    }
    fn fail(&mut self) {
        self.failed = true;
        self.buf = Vec::new();
        self.data = String::new();
        self.event = None;
    }
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if self.failed {
            return out;
        }
        for piece in chunk.split_inclusive(|&b| b == b'\n') {
            if self.buf.len().saturating_add(piece.len()) > self.limit {
                self.fail();
                break;
            }
            self.buf.extend_from_slice(piece);
            if piece.last() != Some(&b'\n') {
                continue;
            }
            let line = String::from_utf8_lossy(&self.buf[..self.buf.len() - 1]);
            if self.data.len().saturating_add(line.len()) > self.limit {
                self.fail();
                break;
            }
            Self::line(&mut self.event, &mut self.data, &mut self.has_data, line.trim_end_matches('\r'), &mut out);
            self.buf.clear();
        }
        out
    }

    /// Flush a trailing event without a terminating blank line.
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut out = Vec::new();
        if self.failed {
            return out;
        }
        if self.data.len().saturating_add(self.buf.len()) > self.limit {
            self.fail();
            return out;
        }
        if !self.buf.is_empty() {
            let line = String::from_utf8_lossy(&self.buf);
            Self::line(&mut self.event, &mut self.data, &mut self.has_data, line.trim_end_matches('\r'), &mut out);
            self.buf.clear();
        }
        Self::dispatch(&mut self.event, &mut self.data, &mut self.has_data, &mut out);
        out
    }

    fn line(event: &mut Option<String>, data: &mut String, has_data: &mut bool, line: &str, out: &mut Vec<SseEvent>) {
        if line.is_empty() {
            Self::dispatch(event, data, has_data, out);
            return;
        }
        if line.starts_with(':') {
            return;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => *event = Some(value.to_string()),
            "data" => {
                if *has_data {
                    data.push('\n');
                }
                data.push_str(value);
                *has_data = true;
            }
            _ => {}
        }
    }

    fn dispatch(event: &mut Option<String>, data: &mut String, has_data: &mut bool, out: &mut Vec<SseEvent>) {
        if !*has_data {
            *event = None;
            return;
        }
        out.push(SseEvent { event: event.take(), data: std::mem::take(data) });
        *has_data = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_split_chunks() {
        let mut d = SseDecoder::default();
        assert!(d.push(b"event: a\nda").is_empty());
        let evs = d.push(b"ta: {\"x\":1}\n\ndata: two\r\n\r\n");
        assert_eq!(evs.len(), 2);
        assert_eq!(evs[0].event.as_deref(), Some("a"));
        assert_eq!(evs[0].data, "{\"x\":1}");
        assert_eq!(evs[1].event, None);
        assert_eq!(evs[1].data, "two");
    }

    #[test]
    fn flushes_trailing_event() {
        let mut d = SseDecoder::default();
        assert!(d.push(b"data: tail").is_empty());
        let evs = d.finish();
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].data, "tail");
    }

    #[test]
    fn every_fragment_boundary_preserves_multiline_unicode_and_empty_data() {
        let wire = ": keepalive\r\nevent: ignored\r\n\r\nevent: answer\r\ndata: café 🦀\r\ndata:\r\ndata:  spaced\r\n\r\ndata:\n\ndata: tail";
        for chunk_size in 1..=wire.len() {
            let mut decoder = SseDecoder::default();
            let mut events = Vec::new();
            for chunk in wire.as_bytes().chunks(chunk_size) {
                events.extend(decoder.push(chunk));
            }
            events.extend(decoder.finish());
            assert_eq!(events.len(), 3, "chunk size {chunk_size}");
            assert_eq!(events[0].event.as_deref(), Some("answer"));
            assert_eq!(events[0].data, "café 🦀\n\n spaced");
            assert_eq!(events[1].event, None);
            assert_eq!(events[1].data, "");
            assert_eq!(events[2].data, "tail");
            assert!(decoder.finish().is_empty());
            assert_eq!(decoder.push(b"data: reused\n\n")[0].data, "reused");
        }
    }

    #[test]
    fn large_fragmented_event_and_lossy_utf8_preserve_data() {
        let data = "x".repeat(4 << 20);
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"data: ").is_empty());
        for chunk in data.as_bytes().chunks(1024) {
            assert!(decoder.push(chunk).is_empty());
        }
        let events = decoder.push(b"\n\ndata: \xff\n\n");
        assert_eq!(events[0].data, data);
        assert_eq!(events[1].data, "\u{fffd}");
    }
}
