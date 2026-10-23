use std::io::BufRead;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LineStatus {
    Eof,
    Complete { oversized: bool },
    Unterminated { oversized: bool },
}

pub(super) fn read_bounded_line<R: BufRead>(
    reader: &mut R,
    line: &mut Vec<u8>,
    limit: usize,
) -> std::io::Result<LineStatus> {
    let mut total = 0usize;
    let mut oversized = false;
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return Ok(if total == 0 {
                LineStatus::Eof
            } else {
                LineStatus::Unterminated { oversized }
            });
        }
        let bytes_to_consume = available
            .iter()
            .position(|byte| *byte == b'\n')
            .map_or(available.len(), |newline| newline + 1);
        if !oversized && line.len().saturating_add(bytes_to_consume) <= limit {
            line.extend_from_slice(&available[..bytes_to_consume]);
        } else {
            oversized = true;
            line.clear();
        }
        let complete = available[bytes_to_consume - 1] == b'\n';
        reader.consume(bytes_to_consume);
        total = total.saturating_add(bytes_to_consume);
        if complete {
            return Ok(LineStatus::Complete { oversized });
        }
    }
}
