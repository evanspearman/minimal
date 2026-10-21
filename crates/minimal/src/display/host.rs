//! The box half of `min session display`: `minimal-display-host`, run over
//! an exec channel, which starts the desktop and its VNC server and prints
//! `ready` once the server accepts connections.
//!
//! The host's contract (the `minimal-display` package):
//! `minimal-display-host [--size WxH] [-- CMD...]` serves VNC on
//! [`VNC_SOCKET`], prints exactly one line, `ready`, on stdout, logs to
//! stderr, and stops everything when its stdin reaches EOF.

use std::collections::VecDeque;

use russh::ChannelMsg;
use tokio::sync::mpsc;

/// Where the host's VNC server listens, inside the box.
pub(crate) const VNC_SOCKET: &str = "/run/minimal-display/vnc.sock";

/// The program the box runs.
const HOST_PROGRAM: &str = "minimal-display-host";

/// How much of a stdout line the client keeps while looking for `ready`.
const MAX_LINE: usize = 4096;

/// The host's argv for an optional `WxH` size and the command it shows.
pub(crate) fn host_argv(size: Option<(u16, u16)>, command: &[String]) -> Vec<String> {
    let mut argv = vec![HOST_PROGRAM.to_string()];
    if let Some((width, height)) = size {
        argv.push("--size".to_string());
        argv.push(format!("{width}x{height}"));
    }
    if !command.is_empty() {
        argv.push("--".to_string());
        argv.extend(command.iter().cloned());
    }
    argv
}

/// What the exec channel delivered, reduced to what the client uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HostMsg {
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    /// The host's exit status.
    Exit(u32),
}

/// Forwards the exec channel's messages to `tx` until the channel closes.
///
/// Runs for the whole display session: the host's output has to be read or
/// the channel's window fills and the host blocks on its next log line.
pub(crate) async fn pump(mut channel: russh::ChannelReadHalf, tx: mpsc::Sender<HostMsg>) {
    while let Some(msg) = channel.wait().await {
        let msg = match msg {
            ChannelMsg::Data { data } => HostMsg::Stdout(data.to_vec()),
            ChannelMsg::ExtendedData { data, ext: 1 } => HostMsg::Stderr(data.to_vec()),
            ChannelMsg::ExitStatus { exit_status } => HostMsg::Exit(exit_status),
            _ => continue,
        };
        if tx.send(msg).await.is_err() {
            return;
        }
    }
}

/// The end of the host's stderr, kept for the message that explains why
/// the display stopped.
#[derive(Debug, Default, Clone)]
pub(crate) struct Tail {
    bytes: VecDeque<u8>,
}

impl Tail {
    /// The most bytes kept.
    const CAPACITY: usize = 4096;
    /// The most lines shown.
    const LINES: usize = 10;

    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.bytes.extend(bytes);
        let excess = self.bytes.len().saturating_sub(Self::CAPACITY);
        self.bytes.drain(..excess);
    }

    /// The last few lines, each indented, or `None` if the host said
    /// nothing on stderr.
    pub(crate) fn render(&self) -> Option<String> {
        let (front, back) = self.bytes.as_slices();
        let text = String::from_utf8_lossy(&[front, back].concat()).into_owned();
        let lines: Vec<&str> = text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .collect();
        if lines.is_empty() {
            return None;
        }
        let shown = &lines[lines.len().saturating_sub(Self::LINES)..];
        Some(
            shown
                .iter()
                .map(|l| format!("  {l}"))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }
}

/// Why the host never got as far as `ready`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotReady {
    /// The host exited, with this status if the daemon sent one.
    Exited(Option<u32>),
}

/// Reads the host's output until it prints `ready`, keeping its stderr in
/// `tail`. The caller bounds how long this may take.
pub(crate) async fn wait_ready(
    rx: &mut mpsc::Receiver<HostMsg>,
    tail: &mut Tail,
) -> Result<(), NotReady> {
    let mut line = Vec::new();
    let mut status = None;
    while let Some(msg) = rx.recv().await {
        match msg {
            HostMsg::Stdout(bytes) => {
                for byte in bytes {
                    if byte == b'\n' {
                        if line == b"ready" {
                            return Ok(());
                        }
                        line.clear();
                    } else if line.len() < MAX_LINE {
                        line.push(byte);
                    }
                }
            }
            HostMsg::Stderr(bytes) => tail.push(&bytes),
            HostMsg::Exit(code) => status = Some(code),
        }
    }
    Err(NotReady::Exited(status))
}

/// After `ready`: keeps reading the host's output, so it never blocks on a
/// full channel, and returns its exit status once the channel closes.
pub(crate) async fn watch(mut rx: mpsc::Receiver<HostMsg>, mut tail: Tail) -> (Option<u32>, Tail) {
    let mut status = None;
    while let Some(msg) = rx.recv().await {
        match msg {
            HostMsg::Stdout(_) => {}
            HostMsg::Stderr(bytes) => tail.push(&bytes),
            HostMsg::Exit(code) => status = Some(code),
        }
    }
    (status, tail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(msgs: Vec<HostMsg>) -> mpsc::Receiver<HostMsg> {
        let (tx, rx) = mpsc::channel(msgs.len().max(1));
        for msg in msgs {
            tx.try_send(msg).unwrap();
        }
        rx
    }

    #[test]
    fn the_argv_names_the_size_and_the_command_after_a_separator() {
        assert_eq!(host_argv(None, &[]), ["minimal-display-host"]);
        assert_eq!(
            host_argv(
                Some((1280, 800)),
                &["foot".into(), "-e".into(), "htop".into()]
            ),
            [
                "minimal-display-host",
                "--size",
                "1280x800",
                "--",
                "foot",
                "-e",
                "htop"
            ]
        );
    }

    #[tokio::test]
    async fn ready_split_across_chunks_is_found() {
        let mut rx = feed(vec![
            HostMsg::Stdout(b"starting\nre".to_vec()),
            HostMsg::Stdout(b"ady\n".to_vec()),
        ]);
        let mut tail = Tail::default();
        assert_eq!(wait_ready(&mut rx, &mut tail).await, Ok(()));
    }

    #[tokio::test]
    async fn ready_must_be_the_whole_line() {
        let mut rx = feed(vec![HostMsg::Stdout(b"not ready\nalready\n".to_vec())]);
        let mut tail = Tail::default();
        assert_eq!(
            wait_ready(&mut rx, &mut tail).await,
            Err(NotReady::Exited(None))
        );
    }

    #[tokio::test]
    async fn an_exit_before_ready_carries_the_status_and_the_stderr() {
        let mut rx = feed(vec![
            HostMsg::Stderr(b"minimal-display-host: niri is not installed.\n".to_vec()),
            HostMsg::Exit(127),
        ]);
        let mut tail = Tail::default();
        assert_eq!(
            wait_ready(&mut rx, &mut tail).await,
            Err(NotReady::Exited(Some(127)))
        );
        assert_eq!(
            tail.render().as_deref(),
            Some("  minimal-display-host: niri is not installed.")
        );
    }

    #[test]
    fn the_tail_keeps_the_last_ten_lines() {
        let mut tail = Tail::default();
        for n in 0..25 {
            tail.push(format!("line {n}\n").as_bytes());
        }
        let shown = tail.render().unwrap();
        assert_eq!(shown.lines().count(), 10);
        assert!(shown.starts_with("  line 15"), "{shown}");
        assert!(shown.ends_with("  line 24"), "{shown}");
    }

    #[test]
    fn the_tail_is_bounded_in_bytes() {
        let mut tail = Tail::default();
        tail.push(&vec![b'x'; Tail::CAPACITY * 3]);
        assert_eq!(tail.bytes.len(), Tail::CAPACITY);
    }

    #[test]
    fn an_empty_tail_renders_as_nothing() {
        let mut tail = Tail::default();
        assert_eq!(tail.render(), None);
        tail.push(b"\n\n");
        assert_eq!(tail.render(), None);
    }

    #[tokio::test]
    async fn watch_drains_until_close_and_keeps_the_status() {
        let rx = feed(vec![
            HostMsg::Stderr(b"wayvnc: bye\n".to_vec()),
            HostMsg::Stdout(b"noise".to_vec()),
            HostMsg::Exit(1),
        ]);
        let (status, tail) = watch(rx, Tail::default()).await;
        assert_eq!(status, Some(1));
        assert_eq!(tail.render().as_deref(), Some("  wayvnc: bye"));
    }
}
