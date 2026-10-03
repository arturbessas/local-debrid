//! Keeps the most recent log lines in memory so the dashboard can show them on a phone
//! where stdout is not visible.

use std::collections::VecDeque;
use std::io::Write;
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use tracing_subscriber::fmt::MakeWriter;

const CAPACITY: usize = 500;

#[derive(Clone, Default)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<String>>>,
}

impl LogBuffer {
    pub fn global() -> &'static LogBuffer {
        static GLOBAL: OnceLock<LogBuffer> = OnceLock::new();
        GLOBAL.get_or_init(LogBuffer::default)
    }

    pub fn push(&self, line: String) {
        let mut lines = self.lines.lock();
        if lines.len() == CAPACITY {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    pub fn tail(&self, n: usize) -> Vec<String> {
        let lines = self.lines.lock();
        lines.iter().skip(lines.len().saturating_sub(n)).cloned().collect()
    }
}

pub struct LineWriter {
    buffer: LogBuffer,
    pending: Vec<u8>,
}

impl Write for LineWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.pending.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Drop for LineWriter {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.pending);
        let line = text.trim_end();
        if !line.is_empty() {
            self.buffer.push(line.to_string());
        }
    }
}

impl<'a> MakeWriter<'a> for LogBuffer {
    type Writer = LineWriter;

    fn make_writer(&'a self) -> Self::Writer {
        LineWriter {
            buffer: self.clone(),
            pending: Vec::new(),
        }
    }
}

/// Installs a subscriber that logs to stderr and to [`LogBuffer::global`]. Safe to call
/// more than once.
pub fn init_logging(default_filter: &str) {
    use tracing_subscriber::prelude::*;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_filter));
    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_target(false)
                .with_writer(LogBuffer::global().clone()),
        )
        .try_init();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_the_latest_lines() {
        let buffer = LogBuffer::default();
        for i in 0..CAPACITY + 10 {
            buffer.push(i.to_string());
        }
        let tail = buffer.tail(3);
        assert_eq!(tail, vec![(CAPACITY + 7).to_string(), (CAPACITY + 8).to_string(), (CAPACITY + 9).to_string()]);
        assert_eq!(buffer.tail(10_000).len(), CAPACITY);
    }
}
