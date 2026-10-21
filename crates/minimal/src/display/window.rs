//! The window `min session display` shows the box's desktop in.
//!
//! The window starts at the desktop's size in physical pixels, so the
//! desktop shows 1:1. If the window is resized the desktop is scaled to
//! fit, keeping its aspect ratio, with black bars where it does not fill
//! the window. The pointer is the window system's own: the box's
//! compositor draws none into a headless output, and a local cursor moves
//! without waiting for the box.

use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc::UnboundedSender;
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::ModifiersState;
use winit::platform::modifier_supplement::KeyEventExtModifierSupplement as _;
use winit::window::{Window, WindowId};

use super::keysym::{KeyTracker, keysym};
use super::rfb::{Framebuffer, Outbound};

/// The desktop as the window last drew it, shared with the task that
/// decodes the server's updates.
pub(crate) struct SharedFrame {
    frame: Mutex<Framebuffer>,
    /// Set when a redraw has been asked for and not yet drawn, so a burst
    /// of updates asks the window for one redraw rather than one each.
    redraw_pending: AtomicBool,
}

impl SharedFrame {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        Self {
            frame: Mutex::new(Framebuffer::new(width, height)),
            redraw_pending: AtomicBool::new(false),
        }
    }

    /// Replaces the shared frame with `fb`. Returns `true` when the window
    /// has no redraw pending and should be asked for one.
    pub(crate) fn publish(&self, fb: &Framebuffer) -> bool {
        self.frame
            .lock()
            .expect("the frame lock is only held to copy pixels")
            .clone_from(fb);
        !self.redraw_pending.swap(true, Ordering::AcqRel)
    }
}

/// What the tasks tell the window.
#[derive(Debug)]
pub(crate) enum UserEvent {
    /// A new frame is in the [`SharedFrame`]; `resized` if its size changed.
    Frame { resized: bool },
    /// The display session is over, for this reason.
    End(End),
}

/// Why the window closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum End {
    /// The user closed the window.
    Closed,
    /// A signal arrived in the terminal `min` runs in.
    Signal(nix::sys::signal::Signal),
    /// The host in the box exited.
    HostExited,
    /// The VNC stream failed or ended while the host still ran.
    StreamLost(String),
    /// The window could not be created or drawn.
    WindowFailed(String),
}

/// Where the desktop sits in the window: its top-left corner and size, in
/// window pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Viewport {
    pub(crate) x: u32,
    pub(crate) y: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    /// The desktop's own size.
    remote: (u16, u16),
}

impl Viewport {
    /// The largest rect of the desktop's aspect ratio that fits the window,
    /// centred.
    pub(crate) fn fit(remote: (u16, u16), window: (u32, u32)) -> Self {
        let (rw, rh) = (f64::from(remote.0.max(1)), f64::from(remote.1.max(1)));
        let (ww, wh) = (f64::from(window.0), f64::from(window.1));
        let scale = (ww / rw).min(wh / rh);
        // Rounded and clamped to the window: the f64s are window sizes.
        let width = ((rw * scale).round() as u32).min(window.0);
        let height = ((rh * scale).round() as u32).min(window.1);
        Self {
            x: (window.0 - width) / 2,
            y: (window.1 - height) / 2,
            width,
            height,
            remote,
        }
    }

    /// The desktop pixel under a window position, or `None` outside it.
    pub(crate) fn to_remote(self, at: PhysicalPosition<f64>) -> Option<(u16, u16)> {
        if self.width == 0 || self.height == 0 {
            return None;
        }
        let dx = at.x - f64::from(self.x);
        let dy = at.y - f64::from(self.y);
        if dx < 0.0 || dy < 0.0 || dx >= f64::from(self.width) || dy >= f64::from(self.height) {
            return None;
        }
        let x = dx * f64::from(self.remote.0) / f64::from(self.width);
        let y = dy * f64::from(self.remote.1) / f64::from(self.height);
        // In range by the checks above.
        Some((
            (x as u16).min(self.remote.0.saturating_sub(1)),
            (y as u16).min(self.remote.1.saturating_sub(1)),
        ))
    }
}

/// Draws `fb` into a `window`-sized buffer through `viewport`: nearest
/// neighbour when scaled, row copies when not, black outside.
pub(crate) fn blit(fb: &Framebuffer, out: &mut [u32], window: (u32, u32), viewport: &Viewport) {
    let (ww, wh) = (window.0 as usize, window.1 as usize);
    let (fw, fh) = (usize::from(fb.width()), usize::from(fb.height()));
    let (vx, vy) = (viewport.x as usize, viewport.y as usize);
    let (vw, vh) = (viewport.width as usize, viewport.height as usize);
    let pixels = fb.pixels();
    out.fill(0);
    if vw == 0 || vh == 0 || fw == 0 || fh == 0 {
        return;
    }
    for row in 0..vh.min(wh.saturating_sub(vy)) {
        let src_y = (row * fh / vh).min(fh - 1);
        let src = &pixels[src_y * fw..src_y * fw + fw];
        let dst_start = (vy + row) * ww + vx;
        let dst = &mut out[dst_start..dst_start + vw.min(ww - vx)];
        if vw == fw {
            dst.copy_from_slice(&src[..dst.len()]);
        } else {
            for (col, pixel) in dst.iter_mut().enumerate() {
                *pixel = src[(col * fw / vw).min(fw - 1)];
            }
        }
    }
}

/// The pointer-button bit RFB uses for `button`, if it has one.
fn button_bit(button: MouseButton) -> Option<u8> {
    match button {
        MouseButton::Left => Some(1),
        MouseButton::Middle => Some(2),
        MouseButton::Right => Some(4),
        _ => None,
    }
}

/// Window pixels of smooth scrolling that make one wheel notch.
const PIXELS_PER_NOTCH: f64 = 30.0;

/// The window and its input, as a winit application.
pub(crate) struct App {
    title: String,
    shared: Arc<SharedFrame>,
    outbound: UnboundedSender<Outbound>,
    window: Option<Rc<Window>>,
    surface: Option<softbuffer::Surface<Rc<Window>, Rc<Window>>>,
    /// The desktop's size as last drawn, for mapping the pointer.
    remote: (u16, u16),
    keys: KeyTracker,
    modifiers: ModifiersState,
    buttons: u8,
    pointer: Option<(u16, u16)>,
    /// Smooth-scroll distance not yet sent as a notch.
    scroll: (f64, f64),
    end: Option<End>,
}

impl App {
    pub(crate) fn new(
        title: String,
        size: (u16, u16),
        shared: Arc<SharedFrame>,
        outbound: UnboundedSender<Outbound>,
    ) -> Self {
        Self {
            title,
            shared,
            outbound,
            window: None,
            surface: None,
            remote: size,
            keys: KeyTracker::default(),
            modifiers: ModifiersState::empty(),
            buttons: 0,
            pointer: None,
            scroll: (0.0, 0.0),
            end: None,
        }
    }

    /// Why the window closed; `Closed` if nothing else was recorded.
    pub(crate) fn end(self) -> End {
        self.end.unwrap_or(End::Closed)
    }

    fn finish(&mut self, event_loop: &ActiveEventLoop, end: End) {
        self.end.get_or_insert(end);
        event_loop.exit();
    }

    fn send(&self, message: Outbound) {
        // The writer has gone only once the stream has, and that ends the
        // window through its own event.
        #[expect(
            clippy::let_underscore_must_use,
            reason = "a closed writer is reported by the reader's end event"
        )]
        let _ = self.outbound.send(message);
    }

    fn viewport(&self) -> Option<Viewport> {
        let size = self.window.as_ref()?.inner_size();
        Some(Viewport::fit(self.remote, (size.width, size.height)))
    }

    fn send_pointer(&self) {
        if let Some((x, y)) = self.pointer {
            self.send(Outbound::Pointer {
                buttons: self.buttons,
                x,
                y,
            });
        }
    }

    /// One wheel notch: the button pressed and released.
    fn notch(&self, bit: u8) {
        if let Some((x, y)) = self.pointer {
            for buttons in [self.buttons | bit, self.buttons] {
                self.send(Outbound::Pointer { buttons, x, y });
            }
        }
    }

    fn scroll(&mut self, dx: f64, dy: f64) {
        self.scroll.0 += dx;
        self.scroll.1 += dy;
        while self.scroll.1 >= 1.0 {
            self.notch(8);
            self.scroll.1 -= 1.0;
        }
        while self.scroll.1 <= -1.0 {
            self.notch(16);
            self.scroll.1 += 1.0;
        }
        while self.scroll.0 <= -1.0 {
            self.notch(32);
            self.scroll.0 += 1.0;
        }
        while self.scroll.0 >= 1.0 {
            self.notch(64);
            self.scroll.0 -= 1.0;
        }
    }

    fn redraw(&mut self) -> Result<(), String> {
        self.shared.redraw_pending.store(false, Ordering::Release);
        let (Some(window), Some(surface)) = (&self.window, &mut self.surface) else {
            return Ok(());
        };
        let size = window.inner_size();
        let (Some(width), Some(height)) =
            (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
        else {
            return Ok(());
        };
        surface.resize(width, height).map_err(|e| e.to_string())?;
        let mut buffer = surface.buffer_mut().map_err(|e| e.to_string())?;
        {
            let frame = self
                .shared
                .frame
                .lock()
                .expect("the frame lock is only held to copy pixels");
            self.remote = (frame.width(), frame.height());
            let viewport = Viewport::fit(self.remote, (size.width, size.height));
            blit(&frame, &mut buffer, (size.width, size.height), &viewport);
        }
        buffer.present().map_err(|e| e.to_string())
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attributes = Window::default_attributes()
            .with_title(self.title.clone())
            .with_inner_size(PhysicalSize::new(
                u32::from(self.remote.0),
                u32::from(self.remote.1),
            ));
        let created = event_loop
            .create_window(attributes)
            .map_err(|e| e.to_string())
            .map(Rc::new)
            .and_then(|window| {
                let context =
                    softbuffer::Context::new(window.clone()).map_err(|e| e.to_string())?;
                let surface = softbuffer::Surface::new(&context, window.clone())
                    .map_err(|e| e.to_string())?;
                Ok((window, surface))
            });
        match created {
            Ok((window, surface)) => {
                window.request_redraw();
                self.window = Some(window);
                self.surface = Some(surface);
            }
            Err(e) => self.finish(event_loop, End::WindowFailed(e)),
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Frame { resized } => {
                let Some(window) = &self.window else { return };
                if resized {
                    let (width, height) = {
                        let frame = self
                            .shared
                            .frame
                            .lock()
                            .expect("the frame lock is only held to copy pixels");
                        (frame.width(), frame.height())
                    };
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "a window that keeps its size scales the desktop to fit"
                    )]
                    let _ = window
                        .request_inner_size(PhysicalSize::new(u32::from(width), u32::from(height)));
                }
                window.request_redraw();
            }
            UserEvent::End(end) => self.finish(event_loop, end),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => self.finish(event_loop, End::Closed),
            WindowEvent::Resized(_) => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.redraw() {
                    self.finish(event_loop, End::WindowFailed(e));
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => self.modifiers = modifiers.state(),
            WindowEvent::Focused(false) => {
                for release in self.keys.release_all() {
                    self.send(release);
                }
                if self.buttons != 0 {
                    self.buttons = 0;
                    self.send_pointer();
                }
            }
            WindowEvent::KeyboardInput { event, .. } => match event.state {
                ElementState::Pressed => {
                    // With Ctrl, Option or Cmd held, a key means its plain
                    // self (Option+e is a shortcut there, not a dead key);
                    // the box applies the modifiers it was told are down.
                    let held =
                        ModifiersState::CONTROL | ModifiersState::ALT | ModifiersState::SUPER;
                    let key = if self.modifiers.intersects(held) {
                        event.key_without_modifiers()
                    } else {
                        event.logical_key.clone()
                    };
                    if let Some(press) = self
                        .keys
                        .press(event.physical_key, keysym(&key, event.location))
                    {
                        self.send(press);
                    }
                }
                ElementState::Released => {
                    if let Some(release) = self.keys.release(event.physical_key) {
                        self.send(release);
                    }
                }
            },
            WindowEvent::CursorMoved { position, .. } => {
                if let Some(at) = self.viewport().and_then(|v| v.to_remote(position)) {
                    self.pointer = Some(at);
                    self.send_pointer();
                }
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if let Some(bit) = button_bit(button) {
                    match state {
                        ElementState::Pressed => self.buttons |= bit,
                        ElementState::Released => self.buttons &= !bit,
                    }
                    self.send_pointer();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => match delta {
                MouseScrollDelta::LineDelta(x, y) => self.scroll(f64::from(x), f64::from(y)),
                MouseScrollDelta::PixelDelta(p) => {
                    self.scroll(p.x / PIXELS_PER_NOTCH, p.y / PIXELS_PER_NOTCH);
                }
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f64, y: f64) -> PhysicalPosition<f64> {
        PhysicalPosition::new(x, y)
    }

    #[test]
    fn a_window_the_desktops_size_shows_it_one_to_one() {
        let v = Viewport::fit((1280, 720), (1280, 720));
        assert_eq!((v.x, v.y, v.width, v.height), (0, 0, 1280, 720));
        assert_eq!(v.to_remote(at(10.4, 719.9)), Some((10, 719)));
    }

    #[test]
    fn a_wider_window_letterboxes_left_and_right() {
        let v = Viewport::fit((100, 100), (300, 200));
        assert_eq!((v.x, v.y, v.width, v.height), (50, 0, 200, 200));
        assert_eq!(v.to_remote(at(49.0, 10.0)), None, "in the bar");
        assert_eq!(v.to_remote(at(50.0, 0.0)), Some((0, 0)));
        assert_eq!(v.to_remote(at(249.9, 199.9)), Some((99, 99)));
        assert_eq!(v.to_remote(at(250.0, 10.0)), None, "in the other bar");
    }

    #[test]
    fn a_taller_window_letterboxes_top_and_bottom() {
        let v = Viewport::fit((200, 100), (200, 300));
        assert_eq!((v.x, v.y, v.width, v.height), (0, 100, 200, 100));
    }

    #[test]
    fn a_zero_sized_window_maps_nothing() {
        let v = Viewport::fit((200, 100), (0, 0));
        assert_eq!(v.to_remote(at(0.0, 0.0)), None);
        let mut out: Vec<u32> = Vec::new();
        blit(&Framebuffer::new(200, 100), &mut out, (0, 0), &v);
    }

    fn checkered(w: u16, h: u16) -> Framebuffer {
        let mut fb = Framebuffer::new(w, h);
        for y in 0..h {
            for x in 0..w {
                fb.set(x, y, u32::from(y) * 100 + u32::from(x));
            }
        }
        fb
    }

    #[test]
    fn blit_copies_rows_one_to_one() {
        let fb = checkered(3, 2);
        let v = Viewport::fit((3, 2), (3, 2));
        let mut out = vec![0xdead; 6];
        blit(&fb, &mut out, (3, 2), &v);
        assert_eq!(out, fb.pixels());
    }

    #[test]
    fn blit_scales_up_by_nearest_neighbour_and_blacks_the_bars() {
        let fb = checkered(2, 1);
        // 2x1 into 6x2: 4x2 at x=1, bars at x=0 and x=5.
        let v = Viewport::fit((2, 1), (6, 2));
        assert_eq!((v.x, v.width, v.height), (1, 4, 2));
        let mut out = vec![0xdead; 12];
        blit(&fb, &mut out, (6, 2), &v);
        assert_eq!(&out[..6], &[0, 0, 0, 1, 1, 0]);
        assert_eq!(&out[6..], &[0, 0, 0, 1, 1, 0]);
    }

    #[test]
    fn blit_scales_down() {
        let fb = checkered(4, 4);
        let v = Viewport::fit((4, 4), (2, 2));
        let mut out = vec![0; 4];
        blit(&fb, &mut out, (2, 2), &v);
        assert_eq!(out, [0, 2, 200, 202]);
    }

    #[test]
    fn publishing_asks_for_one_redraw_until_it_is_drawn() {
        let shared = SharedFrame::new(1, 1);
        let fb = Framebuffer::new(1, 1);
        assert!(shared.publish(&fb));
        assert!(!shared.publish(&fb), "a redraw is already pending");
        shared.redraw_pending.store(false, Ordering::Release);
        assert!(shared.publish(&fb));
    }
}
