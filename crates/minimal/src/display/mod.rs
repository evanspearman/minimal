//! `min session display`: a native window onto a Wayland desktop running
//! inside a session's box.
//!
//! The box runs `minimal-display-host` (the `minimal-display` package),
//! which starts a headless compositor showing the desktop and a VNC server
//! on a unix socket. `min` starts the host over an exec channel, reaches
//! the socket over a `direct-streamlocal` channel, and draws what the
//! server sends into a window, sending keys and the pointer back.
//!
//! The command runs in two phases, because winit's event loop must own the
//! process's main thread (macOS requires it) and the CLI's async runtime
//! runs its commands on that thread too:
//!
//! 1. [`prepare`], async, inside the command's `cmd` span: start the host,
//!    wait for it to be ready, connect, and finish the VNC handshake. Any
//!    failure here is an ordinary command error, before a window exists.
//! 2. [`run_on_main_thread`], once the runtime has given the main thread
//!    back: run the window until it closes, with the network on the
//!    runtime's worker threads, then stop the host and flush telemetry.

mod host;
mod keysym;
pub(crate) mod rfb;
mod stats;
mod window;
mod zrle;

use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, anyhow};
use nix::sys::signal::Signal;
use tokio::io::{ReadHalf, WriteHalf};
use tokio::signal::unix::{SignalKind, signal};
use tokio::sync::mpsc;
use tracing::Instrument as _;
use winit::event_loop::{EventLoop, EventLoopProxy};

use self::host::{HostMsg, NotReady, Tail};
use self::rfb::{Outbound, Reader, RfbError, ServerInit};
use self::stats::Stats;
use self::window::{App, End, SharedFrame, UserEvent};

/// How long the host has to start its desktop, from the exec request on.
/// The box may still be coming up when the request arrives.
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// How long the host has to stop once its stdin closes.
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// How long a lost stream waits for the host's exit, so a host that died
/// is reported as that rather than as a lost connection.
const HOST_EXIT_GRACE: Duration = Duration::from_secs(1);

type RfbStream = russh::ChannelStream<russh::client::Msg>;

/// Everything phase 1 hands to phase 2.
pub struct Pending {
    /// The command's `cmd` span, kept open for the window's lifetime.
    root: tracing::Span,
    /// The session as messages name it.
    label: String,
    /// The session-scoped connection both channels ride on.
    _connection: minimal_client::Client,
    /// The host's stdin: closing it stops the host.
    host_input: russh::ChannelWriteHalf<russh::client::Msg>,
    host_output: mpsc::Receiver<HostMsg>,
    host_tail: Tail,
    init: ServerInit,
    rfb_read: ReadHalf<RfbStream>,
    rfb_write: WriteHalf<RfbStream>,
    signals: Signals,
}

impl std::fmt::Debug for Pending {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pending")
            .field("label", &self.label)
            .field("init", &self.init)
            .finish_non_exhaustive()
    }
}

/// The signals that end a display session, registered once in phase 1 so
/// none is lost between the phases.
struct Signals {
    interrupt: Option<tokio::signal::unix::Signal>,
    terminate: Option<tokio::signal::unix::Signal>,
    hangup: Option<tokio::signal::unix::Signal>,
}

impl Signals {
    fn install() -> Self {
        Self {
            interrupt: signal(SignalKind::interrupt()).ok(),
            terminate: signal(SignalKind::terminate()).ok(),
            hangup: signal(SignalKind::hangup()).ok(),
        }
    }

    /// The next of the three to arrive.
    async fn next(&mut self) -> Signal {
        async fn on(stream: &mut Option<tokio::signal::unix::Signal>) {
            if let Some(s) = stream.as_mut()
                && s.recv().await.is_some()
            {
                return;
            }
            std::future::pending::<()>().await;
        }
        tokio::select! {
            () = on(&mut self.interrupt) => Signal::SIGINT,
            () = on(&mut self.terminate) => Signal::SIGTERM,
            () = on(&mut self.hangup) => Signal::SIGHUP,
        }
    }
}

/// Phase 1: starts the display in the session and connects to it.
///
/// `label` names the session in messages; `size` is the desktop's size in
/// pixels and `command` what it shows, both passed to the host as given.
pub(crate) async fn prepare(
    sock: &std::path::Path,
    session: boxes::SessionId,
    label: String,
    size: Option<(u16, u16)>,
    command: &[String],
) -> Result<Pending, anyhow::Error> {
    let mut signals = Signals::install();
    let mut connection = minimal_client::Client::connect_scoped(sock, session)
        .await
        .context("Failed to open the session-scoped connection")?;

    let argv = host::host_argv(size, command);
    let request = minimald_rpc::exec::ExecRequest::Argv(argv).encode();
    let channel = connection
        .open_session_exec_channel(
            session,
            &request,
            &minimald_rpc::taskenv::TaskEnv::default(),
        )
        .await
        .with_context(|| format!("Could not start the display in session {label}"))?;
    let (host_read, host_input) = channel.split();
    let (tx, mut host_output) = mpsc::channel(64);
    tokio::spawn(host::pump(host_read, tx).in_current_span());

    let mut host_tail = Tail::default();
    tokio::select! {
        ready = tokio::time::timeout(
            READY_TIMEOUT,
            host::wait_ready(&mut host_output, &mut host_tail),
        ) => match ready {
            Ok(Ok(())) => {}
            Ok(Err(NotReady::Exited(status))) => {
                return Err(anyhow!(not_ready_message(&label, status, &host_tail)));
            }
            Err(_) => {
                // Dropping the channel below stops the host; say so first.
                return Err(anyhow!(with_tail(
                    format!(
                        "The display in session {label} was not ready after {} seconds, \
                         so min stopped it.",
                        READY_TIMEOUT.as_secs()
                    ),
                    &host_tail,
                )));
            }
        },
        signal = signals.next() => {
            // The connection drops with this function, which stops the host.
            return Err(crate::cmd::Handoff::Raise(signal).into());
        }
    }

    let connect_failed = || format!("Could not connect to the display in session {label}");
    let mut stream = connection
        .open_direct_streamlocal(host::VNC_SOCKET)
        .await
        .with_context(connect_failed)?
        .into_stream();
    let init = rfb::handshake(&mut stream)
        .await
        .with_context(connect_failed)?;
    let (rfb_read, rfb_write) = tokio::io::split(stream);

    Ok(Pending {
        root: tracing::Span::current(),
        label,
        _connection: connection,
        host_input,
        host_output,
        host_tail,
        init,
        rfb_read,
        rfb_write,
        signals,
    })
}

/// The message for a host that exited before it was ready.
fn not_ready_message(label: &str, status: Option<u32>, tail: &Tail) -> String {
    let head = match status {
        // The nsenter shim exits 127 when the program it runs is missing, and
        // the host exits 127 when the program it was asked to show is.
        Some(127) => format!(
            "Session {label} cannot start its display: minimal-display-host, or the \
             program it runs, is not installed in the box."
        ),
        Some(code) => format!(
            "The display in session {label} stopped before it was ready (exit status {code})."
        ),
        None => format!("The display in session {label} stopped before it was ready."),
    };
    with_tail(head, tail)
}

/// `head`, then the host's last stderr lines if it wrote any.
fn with_tail(head: String, tail: &Tail) -> String {
    match tail.render() {
        Some(lines) => format!("{head}\nIts last output:\n{lines}"),
        None => head,
    }
}

/// The message and exit status for how a display session ended, after the
/// host has been stopped. `None` for the message means nothing is printed.
fn outcome(
    label: &str,
    end: &End,
    host_status: Option<u32>,
    tail: &Tail,
) -> (Option<String>, ExitCode) {
    match end {
        End::Closed | End::Signal(_) => (None, ExitCode::SUCCESS),
        End::HostExited => match host_status {
            Some(0) => (None, ExitCode::SUCCESS),
            Some(code) => (
                Some(with_tail(
                    format!("The display in session {label} stopped (exit status {code})."),
                    tail,
                )),
                ExitCode::FAILURE,
            ),
            None => (
                Some(with_tail(
                    format!("The display in session {label} stopped."),
                    tail,
                )),
                ExitCode::FAILURE,
            ),
        },
        End::StreamLost(cause) => (
            Some(format!(
                "Lost the connection to the display in session {label}: {cause}"
            )),
            ExitCode::FAILURE,
        ),
        End::WindowFailed(cause) => (
            Some(format!("Could not open a window: {cause}.")),
            ExitCode::FAILURE,
        ),
    }
}

/// Merges consecutive pointer moves: only where the pointer ended up
/// matters, and a slow link should not queue the path. A pointer event that
/// changes the buttons is a click and is never merged away, so it lands
/// where it happened.
fn coalesce(messages: &mut Vec<Outbound>) {
    // Each kept message, and whether it is a move a later move may replace.
    let mut kept: Vec<(Outbound, bool)> = Vec::with_capacity(messages.len());
    let mut buttons_before = None;
    for message in messages.drain(..) {
        let is_move = match message {
            Outbound::Pointer { buttons, .. } => {
                let is_move = buttons_before == Some(buttons);
                buttons_before = Some(buttons);
                is_move
            }
            _ => false,
        };
        if is_move && matches!(kept.last(), Some((_, true))) {
            kept.pop();
        }
        kept.push((message, is_move));
    }
    messages.extend(kept.into_iter().map(|(message, _)| message));
}

/// Decodes the server's updates, publishes each to the window, and asks for
/// the next. Ends the window when the stream does.
async fn read_loop(
    mut reader: Reader<ReadHalf<RfbStream>>,
    shared: Arc<SharedFrame>,
    proxy: EventLoopProxy<UserEvent>,
    outbound: mpsc::UnboundedSender<Outbound>,
    stats: Arc<Stats>,
) {
    let cause = loop {
        match reader.next_update().await {
            Ok(update) => {
                stats
                    .frames
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let fb = reader.framebuffer();
                if shared.publish(fb) || update.resized {
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "a closed event loop is already ending the session"
                    )]
                    let _ = proxy.send_event(UserEvent::Frame {
                        resized: update.resized,
                    });
                }
                let request = Outbound::Request {
                    incremental: true,
                    width: fb.width(),
                    height: fb.height(),
                };
                if outbound.send(request).is_err() {
                    return;
                }
            }
            Err(RfbError::Io(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break "the box closed it".to_string();
            }
            Err(e) => break e.to_string(),
        }
    };
    // A stream that ends because the host exited is reported as the host's
    // exit, which arrives on its own event: give it a moment to.
    tokio::time::sleep(HOST_EXIT_GRACE).await;
    #[expect(
        clippy::let_underscore_must_use,
        reason = "a closed event loop is already ending the session"
    )]
    let _ = proxy.send_event(UserEvent::End(End::StreamLost(cause)));
}

/// Sends the window's input and the reader's update requests to the server.
async fn write_loop(
    mut stream: WriteHalf<RfbStream>,
    mut outbound: mpsc::UnboundedReceiver<Outbound>,
    stats: Arc<Stats>,
) {
    let mut batch = Vec::new();
    while outbound.recv_many(&mut batch, 256).await > 0 {
        coalesce(&mut batch);
        let pointers = batch
            .iter()
            .filter(|m| matches!(m, Outbound::Pointer { .. }))
            .count();
        let started = std::time::Instant::now();
        if rfb::send(&mut stream, &batch).await.is_err() {
            return;
        }
        Stats::max(&stats.max_write_us, started.elapsed());
        stats
            .writes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        stats
            .pointers_sent
            .fetch_add(pointers as u64, std::sync::atomic::Ordering::Relaxed);
        batch.clear();
    }
}

/// Phase 2: runs the window on the calling thread, which must be the
/// process's main thread, until the display session ends; then stops the
/// host, flushes telemetry, and reports how it ended.
///
/// `runtime` is the runtime phase 1 ran on; the network tasks run on its
/// worker threads while this thread runs the window.
pub fn run_on_main_thread(pending: Pending, runtime: &tokio::runtime::Runtime) -> ExitCode {
    let Pending {
        root,
        label,
        _connection,
        host_input,
        host_output,
        host_tail,
        init,
        rfb_read,
        rfb_write,
        mut signals,
    } = pending;
    let entered = root.clone().entered();

    let shared = Arc::new(SharedFrame::new(init.width, init.height));
    let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();
    let (end, tasks, host_watch) = match build_event_loop() {
        Ok(event_loop) => {
            let proxy = event_loop.create_proxy();
            let stats = Arc::new(Stats::default());
            let mut tasks = Vec::new();
            let reporter = stats.clone();
            tasks.push(
                runtime.spawn(async move { reporter.report().await }.instrument(root.clone())),
            );
            tasks.push(
                runtime.spawn(
                    read_loop(
                        Reader::new(rfb_read, &init),
                        shared.clone(),
                        proxy.clone(),
                        outbound_tx.clone(),
                        stats.clone(),
                    )
                    .instrument(root.clone()),
                ),
            );
            tasks.push(
                runtime.spawn(
                    write_loop(rfb_write, outbound_rx, stats.clone()).instrument(root.clone()),
                ),
            );
            let signal_proxy = proxy.clone();
            tasks.push(
                runtime.spawn(
                    async move {
                        let signal = signals.next().await;
                        #[expect(
                            clippy::let_underscore_must_use,
                            reason = "a closed event loop is already ending the session"
                        )]
                        let _ = signal_proxy.send_event(UserEvent::End(End::Signal(signal)));
                    }
                    .instrument(root.clone()),
                ),
            );
            let host_proxy = proxy;
            let host_watch = runtime.spawn(
                async move {
                    let watched = host::watch(host_output, host_tail).await;
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "a closed event loop is already ending the session"
                    )]
                    let _ = host_proxy.send_event(UserEvent::End(End::HostExited));
                    watched
                }
                .instrument(root.clone()),
            );
            let title = format!("{label} — min session display");
            let mut app = App::new(title, (init.width, init.height), shared, outbound_tx, stats);
            let end = match event_loop.run_app(&mut app) {
                Ok(()) => app.end(),
                Err(e) => End::WindowFailed(e.to_string()),
            };
            (end, tasks, Some(host_watch))
        }
        Err(e) => (End::WindowFailed(e.to_string()), Vec::new(), None),
    };

    // Stop the host by closing its stdin, then wait for it to go.
    let (host_status, tail) = runtime.block_on(
        async move {
            for task in &tasks {
                task.abort();
            }
            for task in tasks {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "an aborted task's JoinError is the expected outcome"
                )]
                let _ = task.await;
            }
            #[expect(
                clippy::let_underscore_must_use,
                reason = "a host that already exited has no stdin to close"
            )]
            let _ = host_input.eof().await;
            let Some(watch) = host_watch else {
                return (None, Tail::default());
            };
            match tokio::time::timeout(STOP_TIMEOUT, watch).await {
                Ok(Ok(watched)) => watched,
                _ => (None, Tail::default()),
            }
        }
        .instrument(root.clone()),
    );
    drop(_connection);
    drop(entered);
    drop(root);
    mlog::otel::shutdown(mlog::otel::CLI_EXIT_FLUSH);

    let (message, code) = outcome(&label, &end, host_status, &tail);
    if let Some(message) = message {
        eprintln!("error: {message}");
    }
    match end {
        End::Signal(signal @ (Signal::SIGTERM | Signal::SIGHUP)) => crate::cmd::die_of(signal),
        _ => code,
    }
}

fn build_event_loop() -> Result<EventLoop<UserEvent>, winit::error::EventLoopError> {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    // Without the default menu, Cmd+Q reaches the box (niri closes a window
    // on Super+Q) instead of quitting `min` behind the host's back.
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::EventLoopBuilderExtMacOS as _;
        builder.with_default_menu(false);
    }
    builder.build()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tail(text: &str) -> Tail {
        let mut tail = Tail::default();
        tail.push(text.as_bytes());
        tail
    }

    #[test]
    fn exit_127_before_ready_says_the_host_or_its_program_is_missing() {
        let message = not_ready_message(
            "dev (a1b2c3d4)",
            Some(127),
            &tail("minimal-display-host: niri is not installed.\n"),
        );
        assert_eq!(
            message,
            "Session dev (a1b2c3d4) cannot start its display: minimal-display-host, or the \
             program it runs, is not installed in the box.\n\
             Its last output:\n  minimal-display-host: niri is not installed."
        );
    }

    #[test]
    fn another_exit_before_ready_names_its_status() {
        assert_eq!(
            not_ready_message("dev", Some(1), &Tail::default()),
            "The display in session dev stopped before it was ready (exit status 1)."
        );
        assert_eq!(
            not_ready_message("dev", None, &Tail::default()),
            "The display in session dev stopped before it was ready."
        );
    }

    #[test]
    fn closing_the_window_or_ctrl_c_ends_quietly() {
        for end in [End::Closed, End::Signal(Signal::SIGINT)] {
            assert_eq!(
                outcome("dev", &end, Some(143), &tail("x\n")),
                (None, ExitCode::SUCCESS)
            );
        }
    }

    #[test]
    fn a_host_that_exits_cleanly_ends_quietly() {
        assert_eq!(
            outcome("dev", &End::HostExited, Some(0), &Tail::default()),
            (None, ExitCode::SUCCESS)
        );
    }

    #[test]
    fn a_host_that_fails_is_reported_with_its_output() {
        let (message, code) = outcome("dev", &End::HostExited, Some(1), &tail("cage: oops\n"));
        assert_eq!(
            message.as_deref(),
            Some(
                "The display in session dev stopped (exit status 1).\nIts last output:\n  cage: oops"
            )
        );
        assert_eq!(code, ExitCode::FAILURE);
    }

    #[test]
    fn a_lost_stream_and_a_failed_window_are_reported() {
        let (message, _) = outcome(
            "dev",
            &End::StreamLost("the box closed it".into()),
            None,
            &Tail::default(),
        );
        assert_eq!(
            message.as_deref(),
            Some("Lost the connection to the display in session dev: the box closed it")
        );
        let (message, _) = outcome(
            "dev",
            &End::WindowFailed("no display".into()),
            None,
            &Tail::default(),
        );
        assert_eq!(
            message.as_deref(),
            Some("Could not open a window: no display.")
        );
    }

    #[test]
    fn pointer_moves_coalesce_but_clicks_and_keys_do_not() {
        let at = |x, buttons| Outbound::Pointer { buttons, x, y: 0 };
        let key = Outbound::Key {
            down: true,
            keysym: 0x61,
        };
        let mut batch = vec![
            at(1, 0),
            at(2, 0),
            at(3, 0),
            at(3, 1), // the press
            at(4, 1),
            at(5, 1),
            key,
            at(6, 1),
            at(6, 0), // the release
        ];
        coalesce(&mut batch);
        assert_eq!(
            batch,
            [
                at(1, 0),
                at(3, 0),
                at(3, 1),
                at(5, 1),
                key,
                at(6, 1),
                at(6, 0)
            ]
        );
    }
}
