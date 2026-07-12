use crate::error::{AppError, AppResult};

const MAX_PENDING_BYTES: usize = 1024 * 1024;

/// Incremental SSE decoder. Network chunks remain bytes until a complete
/// line is available, so a UTF-8 code point split across chunks is preserved.
pub(crate) struct SseDecoder {
    pending: Vec<u8>,
    data_lines: Vec<String>,
}

impl SseDecoder {
    pub(crate) fn new() -> Self {
        Self {
            pending: Vec::new(),
            data_lines: Vec::new(),
        }
    }

    pub(crate) fn push(&mut self, chunk: &[u8]) -> AppResult<Vec<String>> {
        if self.pending.len().saturating_add(chunk.len()) > MAX_PENDING_BYTES {
            return Err(protocol_error("SSE event exceeds size limit"));
        }
        self.pending.extend_from_slice(chunk);

        let mut events = Vec::new();
        let mut consumed = 0;
        while let Some(relative_end) = self.pending[consumed..].iter().position(|b| *b == b'\n') {
            let end = consumed + relative_end;
            let line = self.pending[consumed..end].to_vec();
            consumed = end + 1;
            self.process_line(&line, &mut events)?;
        }
        if consumed > 0 {
            self.pending.drain(..consumed);
        }
        Ok(events)
    }

    pub(crate) fn finish(&mut self) -> AppResult<Vec<String>> {
        let mut events = Vec::new();
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.process_line(&line, &mut events)?;
        }
        self.dispatch(&mut events);
        Ok(events)
    }

    fn process_line(&mut self, raw_line: &[u8], events: &mut Vec<String>) -> AppResult<()> {
        let raw_line = raw_line.strip_suffix(b"\r").unwrap_or(raw_line);
        let line = std::str::from_utf8(raw_line)
            .map_err(|_| protocol_error("SSE stream contains invalid UTF-8"))?;
        if line.is_empty() {
            self.dispatch(events);
            return Ok(());
        }
        if line.starts_with(':') {
            return Ok(());
        }
        if let Some(value) = line.strip_prefix("data:") {
            self.data_lines
                .push(value.strip_prefix(' ').unwrap_or(value).to_string());
        }
        Ok(())
    }

    fn dispatch(&mut self, events: &mut Vec<String>) {
        if !self.data_lines.is_empty() {
            events.push(self.data_lines.join("\n"));
            self.data_lines.clear();
        }
    }
}

fn protocol_error(message: &str) -> AppError {
    AppError::Ai(format!("AI_STREAM_PROTOCOL_ERROR: {message}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_utf8_split_across_network_chunks() {
        let bytes = "data: {\"delta\":\"中文🙂\"}\n\n".as_bytes();
        let split = bytes.iter().position(|byte| *byte >= 0x80).unwrap() + 1;
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(&bytes[..split]).unwrap().is_empty());
        assert_eq!(
            decoder.push(&bytes[split..]).unwrap(),
            vec!["{\"delta\":\"中文🙂\"}"]
        );
    }

    #[test]
    fn accepts_crlf_no_space_and_multiline_data() {
        let mut decoder = SseDecoder::new();
        assert_eq!(
            decoder
                .push(b"event: message\r\ndata:first\r\ndata: second\r\n\r\n")
                .unwrap(),
            vec!["first\nsecond"]
        );
    }

    #[test]
    fn dispatches_eof_residue_and_rejects_invalid_utf8() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: tail").unwrap().is_empty());
        assert_eq!(decoder.finish().unwrap(), vec!["tail"]);

        let mut invalid = SseDecoder::new();
        assert!(invalid.push(b"data: \xff\n").is_err());
    }
}
