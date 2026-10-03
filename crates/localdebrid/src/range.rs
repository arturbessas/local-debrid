//! `Range: bytes=...` parsing (RFC 9110 §14). Only the first range of a multi-range
//! request is honoured; players never send more than one.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    /// Inclusive.
    pub end: u64,
}

impl ByteRange {
    pub fn len(&self) -> u64 {
        self.end - self.start + 1
    }

    pub fn is_empty(&self) -> bool {
        false
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RangeError {
    /// The header is syntactically valid but outside the resource: answer 416.
    Unsatisfiable,
}

/// Returns `Ok(None)` when the whole resource should be sent (no header, or a header we
/// must ignore per the RFC because it is malformed or not in bytes).
pub fn parse(header: Option<&str>, len: u64) -> Result<Option<ByteRange>, RangeError> {
    let Some(header) = header else {
        return Ok(None);
    };
    let Some(spec) = header.trim().strip_prefix("bytes=") else {
        return Ok(None);
    };
    let Some(first) = spec.split(',').next().map(str::trim) else {
        return Ok(None);
    };
    let Some((start, end)) = first.split_once('-') else {
        return Ok(None);
    };
    let (start, end) = (start.trim(), end.trim());

    if start.is_empty() {
        let Ok(suffix) = end.parse::<u64>() else {
            return Ok(None);
        };
        if suffix == 0 || len == 0 {
            return Err(RangeError::Unsatisfiable);
        }
        let start = len.saturating_sub(suffix);
        return Ok(Some(ByteRange { start, end: len - 1 }));
    }

    let Ok(start) = start.parse::<u64>() else {
        return Ok(None);
    };
    let end = if end.is_empty() {
        None
    } else {
        match end.parse::<u64>() {
            Ok(e) => Some(e),
            Err(_) => return Ok(None),
        }
    };
    if let Some(e) = end
        && e < start
    {
        return Ok(None);
    }
    if start >= len {
        return Err(RangeError::Unsatisfiable);
    }
    let end = end.map_or(len - 1, |e| e.min(len - 1));
    Ok(Some(ByteRange { start, end }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(start: u64, end: u64) -> Result<Option<ByteRange>, RangeError> {
        Ok(Some(ByteRange { start, end }))
    }

    #[test]
    fn no_header_means_full_body() {
        assert_eq!(parse(None, 100), Ok(None));
    }

    #[test]
    fn closed_open_and_suffix_ranges() {
        assert_eq!(parse(Some("bytes=0-9"), 100), r(0, 9));
        assert_eq!(parse(Some("bytes=90-"), 100), r(90, 99));
        assert_eq!(parse(Some("bytes=-10"), 100), r(90, 99));
        assert_eq!(parse(Some("bytes=-1000"), 100), r(0, 99));
        assert_eq!(parse(Some("bytes=0-"), 100), r(0, 99));
    }

    #[test]
    fn end_is_clamped_to_resource() {
        assert_eq!(parse(Some("bytes=50-5000"), 100), r(50, 99));
    }

    #[test]
    fn start_beyond_end_is_unsatisfiable() {
        assert_eq!(parse(Some("bytes=100-"), 100), Err(RangeError::Unsatisfiable));
        assert_eq!(parse(Some("bytes=-0"), 100), Err(RangeError::Unsatisfiable));
    }

    #[test]
    fn malformed_headers_are_ignored() {
        assert_eq!(parse(Some("items=0-1"), 100), Ok(None));
        assert_eq!(parse(Some("bytes=abc"), 100), Ok(None));
        assert_eq!(parse(Some("bytes=9-1"), 100), Ok(None));
        assert_eq!(parse(Some("bytes=x-1"), 100), Ok(None));
    }

    #[test]
    fn multi_range_uses_first() {
        assert_eq!(parse(Some("bytes=0-1, 5-6"), 100), r(0, 1));
    }

    #[test]
    fn length() {
        assert_eq!(ByteRange { start: 0, end: 0 }.len(), 1);
        assert_eq!(ByteRange { start: 10, end: 19 }.len(), 10);
    }
}
