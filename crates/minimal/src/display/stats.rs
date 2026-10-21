//! TEMPORARY diagnostics for the pointer stalls seen on macOS: counts per
//! second of what the window received and what went out, logged at debug
//! (`RUST_LOG=minimal::display=debug`). Remove once the stall is understood.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A gap between consecutive mouse moves worth logging on its own.
pub(crate) const MOVE_GAP: Duration = Duration::from_millis(150);

#[derive(Default)]
pub(crate) struct Stats {
    /// Mouse moves the window system delivered.
    pub(crate) moves: AtomicU64,
    /// Pointer messages written to the server.
    pub(crate) pointers_sent: AtomicU64,
    /// Writes to the server, and the slowest one in microseconds.
    pub(crate) writes: AtomicU64,
    pub(crate) max_write_us: AtomicU64,
    /// Frame updates decoded.
    pub(crate) frames: AtomicU64,
    /// Redraws, and the slowest one (blit and present) in microseconds.
    pub(crate) redraws: AtomicU64,
    pub(crate) max_redraw_us: AtomicU64,
}

impl Stats {
    pub(crate) fn max(slot: &AtomicU64, elapsed: Duration) {
        let us = u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX);
        slot.fetch_max(us, Ordering::Relaxed);
    }

    /// Logs and resets the counters once a second, until aborted.
    pub(crate) async fn report(&self) {
        let start = Instant::now();
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.tick().await;
        loop {
            tick.tick().await;
            let take = |a: &AtomicU64| a.swap(0, Ordering::Relaxed);
            tracing::debug!(
                t = start.elapsed().as_secs(),
                moves = take(&self.moves),
                pointers_sent = take(&self.pointers_sent),
                writes = take(&self.writes),
                max_write_ms = take(&self.max_write_us) / 1000,
                frames = take(&self.frames),
                redraws = take(&self.redraws),
                max_redraw_ms = take(&self.max_redraw_us) / 1000,
                "display stats"
            );
        }
    }
}
