//! Incremental SSE (Server-Sent Events) decoder for the Flaps sync stream.
//!
//! Parses `text/event-stream` frames from a byte buffer without any external
//! crate dependency. Only the `data:` field is processed; `event:`, `id:` and
//! `retry:` lines are silently ignored. Comment lines (starting with `:`) are
//! also ignored (keep-alive).
//!
//! The decoder is intentionally stateful: partial frames across chunk boundaries
//! are accumulated until a blank line terminates the event.

use serde::Deserialize;

/// A parsed SSE notification from `GET /sync/v1/events`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SseNotification {
    /// Environment key whose ruleset changed.
    pub(crate) environment: String,
    /// Monotone version of the new ruleset.
    pub(crate) version: u64,
}

/// Serde helper for the SSE `data:` JSON payload.
#[derive(Debug, Deserialize)]
struct EventPayload {
    environment: String,
    version: u64,
}

/// Incremental SSE decoder.
///
/// Feed byte chunks via [`SseDecoder::push`]; collect emitted
/// [`SseNotification`]s from the returned `Vec`.
#[derive(Debug, Default)]
pub(crate) struct SseDecoder {
    /// Incomplete line accumulated from the last chunk.
    line_buf: String,
    /// `data:` value accumulated for the current event.
    data_buf: Option<String>,
}

impl SseDecoder {
    /// Creates a new decoder.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk of bytes into the decoder and returns any completed
    /// notifications parsed from the chunk.
    ///
    /// Invalid JSON payloads are silently ignored (the event is discarded but
    /// the stream continues). Partial frames are buffered until a blank line
    /// terminates the event.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Vec<SseNotification> {
        let Ok(text) = std::str::from_utf8(chunk) else {
            return Vec::new();
        };

        let mut notifications = Vec::new();

        // Split on '\n' gives N+1 segments for N newlines. The last segment
        // is always a partial (or empty) remainder with no trailing '\n' yet -
        // it must be held in `line_buf` until the next chunk completes it.
        let segments: Vec<&str> = text.split('\n').collect();
        let (complete, remainder) = segments.split_at(segments.len().saturating_sub(1));

        for raw_seg in complete {
            // Prepend any carry-over from the previous chunk.
            let line = if self.line_buf.is_empty() {
                (*raw_seg).to_owned()
            } else {
                let mut s = std::mem::take(&mut self.line_buf);
                s.push_str(raw_seg);
                s
            };

            // Strip optional trailing CR (CRLF line endings).
            let line = line.trim_end_matches('\r');

            self.dispatch_line(line, &mut notifications);
        }

        // The last segment has no '\n' yet - accumulate it.
        if let Some(tail) = remainder.first() {
            self.line_buf.push_str(tail);
        }

        notifications
    }

    /// Processes one complete SSE line (already stripped of `\r`).
    fn dispatch_line(&mut self, line: &str, notifications: &mut Vec<SseNotification>) {
        if line.is_empty() {
            // Blank line = end of event dispatch.
            if let Some(data) = self.data_buf.take() {
                if let Ok(payload) = serde_json::from_str::<EventPayload>(&data) {
                    notifications.push(SseNotification {
                        environment: payload.environment,
                        version: payload.version,
                    });
                }
                // Invalid JSON -> silently skip this event.
            }
        } else if let Some(value) = line.strip_prefix("data:") {
            // RFC 8895: optional leading space after the colon.
            let value = value.strip_prefix(' ').unwrap_or(value);
            self.data_buf = Some(value.to_owned());
        }
        // Lines starting with `:` (keep-alive), `event:`, `id:`, `retry:` -> ignore.
    }

    /// Signals the end of the byte stream (no trailing newline).
    ///
    /// In most implementations the server always sends a trailing `\n` so this
    /// is a no-op, but it is provided for completeness.
    #[allow(dead_code)]
    pub(crate) fn flush(&mut self) -> Vec<SseNotification> {
        // If there is a non-empty line buffer but no trailing newline, we cannot
        // complete the event. Discard and reset.
        self.line_buf.clear();
        self.data_buf = None;
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(input: &[u8]) -> Vec<SseNotification> {
        let mut dec = SseDecoder::new();
        dec.push(input)
    }

    #[test]
    fn single_event_parsed() {
        let frame = b"data:{\"environment\":\"prod\",\"version\":1}\n\n";
        let notifs = decode(frame);
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].environment, "prod");
        assert_eq!(notifs[0].version, 1);
    }

    #[test]
    fn keep_alive_comment_ignored() {
        let frame = b":\n\ndata:{\"environment\":\"prod\",\"version\":2}\n\n";
        let notifs = decode(frame);
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].version, 2);
    }

    #[test]
    fn comment_only_produces_no_notification() {
        let frame = b":\n\n";
        let notifs = decode(frame);
        assert!(notifs.is_empty());
    }

    #[test]
    fn invalid_json_silently_ignored() {
        let frame = b"data:not-valid-json\n\ndata:{\"environment\":\"staging\",\"version\":5}\n\n";
        let notifs = decode(frame);
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].version, 5);
    }

    #[test]
    fn partial_frame_then_complete() {
        let mut dec = SseDecoder::new();
        // First chunk: partial line (no newline yet).
        let part1 = b"data:{\"environment\":\"dev\",\"versio";
        let notifs1 = dec.push(part1);
        assert!(
            notifs1.is_empty(),
            "partial frame must not emit a notification"
        );

        // Second chunk: rest of the line + event terminator.
        let part2 = b"n\":7}\n\n";
        let notifs2 = dec.push(part2);
        assert_eq!(notifs2.len(), 1);
        assert_eq!(notifs2[0].version, 7);
    }

    #[test]
    fn multiple_events_in_one_chunk() {
        let frame = b"data:{\"environment\":\"a\",\"version\":1}\n\ndata:{\"environment\":\"b\",\"version\":2}\n\n";
        let notifs = decode(frame);
        assert_eq!(notifs.len(), 2);
        assert_eq!(notifs[0].version, 1);
        assert_eq!(notifs[1].version, 2);
    }

    #[test]
    fn crlf_line_endings_handled() {
        let frame = b"data:{\"environment\":\"prod\",\"version\":3}\r\n\r\n";
        let notifs = decode(frame);
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].version, 3);
    }

    #[test]
    fn data_with_leading_space_parsed() {
        // RFC 8895 allows a single leading space after the colon.
        let frame = b"data: {\"environment\":\"prod\",\"version\":9}\n\n";
        let notifs = decode(frame);
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].version, 9);
    }

    #[test]
    fn unknown_field_lines_ignored() {
        let frame =
            b"event: update\nid: 1\ndata:{\"environment\":\"prod\",\"version\":4}\nretry: 1000\n\n";
        let notifs = decode(frame);
        assert_eq!(notifs.len(), 1);
        assert_eq!(notifs[0].version, 4);
    }

    #[test]
    fn empty_input_produces_nothing() {
        let notifs = decode(b"");
        assert!(notifs.is_empty());
    }
}
