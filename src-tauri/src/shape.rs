//! The window's shape follows its screen (design-system 6.1): wide for Home and Settings, tall for
//! the recording screen and a meeting's document, which sit beside a video call. The change glides
//! like the app's own motion (`dur.medium`, `ease.out`), natively on macOS and frame by frame on
//! Windows, where the system has no animated resize.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};
use tauri::{Manager, State, Window};

/// A logical size (points on macOS) of the page, inside the window's frame.
type Size = (f64, f64);
/// tauri.conf.json's window.
const WIDE: Size = (1040.0, 720.0);
/// The width of tauri.conf.json's `minWidth`.
const TALL: Size = (520.0, 880.0);
/// motion.ts `dur.medium`.
const GLIDE: Duration = Duration::from_millis(320);

pub struct Shape(Mutex<Shapes>);

struct Shapes {
    /// The shape the screen asks for, and whether it may glide there.
    want: bool,
    animate: bool,
    /// The shape the window has.
    tall: bool,
    /// Each shape's size as it was left, so a size the recorder chose comes back with its shape.
    sizes: [Size; 2],
    /// The size this module last gave the window. Still that size, the window was not resized by
    /// the recorder, so a size a short screen cut is not kept as theirs.
    placed: Option<Size>,
    /// Until then the window is between shapes, so its size is neither's.
    gliding_until: Instant,
}

impl Default for Shape {
    fn default() -> Self {
        Self(Mutex::new(Shapes {
            want: false,
            animate: true,
            tall: false,
            sizes: [WIDE, TALL],
            placed: None,
            gliding_until: Instant::now(),
        }))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// `size` around `from`'s center, inside `area`. Symmetric, so it holds for macOS's upward y too.
/// Half of an odd difference is cut toward zero both ways, so a round trip comes back to the same
/// place instead of the window creeping a point each time.
fn place(from: Rect, (w, h): Size, area: Rect) -> Rect {
    let (w, h) = (w.min(area.w), h.min(area.h));
    let x = (from.x + ((from.w - w) / 2.0).trunc()).clamp(area.x, area.x + area.w - w);
    let y = (from.y + ((from.h - h) / 2.0).trunc()).clamp(area.y, area.y + area.h - h);
    Rect { x, y, w, h }
}

/// Within a pixel: logical sizes come back from whole physical pixels.
fn near(a: Size, b: Size) -> bool {
    (a.0 - b.0).abs() < 1.0 && (a.1 - b.1).abs() < 1.0
}

/// The window takes its screen's shape, tall or wide. `animate`: false when a key changed the
/// screen or motion is reduced (design-system 5). Never fails the screen change: a window that
/// will not move is logged.
#[tauri::command]
pub fn set_window_shape(window: Window, shape: State<'_, Shape>, tall: bool, animate: bool) {
    let s = &mut *shape.0.lock().unwrap_or_else(PoisonError::into_inner);
    (s.want, s.animate) = (tall, animate);
    if let Err(e) = fit(&window, s) {
        log::warn!("창 모양을 바꾸지 못했습니다: {e}");
    }
}

/// The window came to the front: a shape it could not take while hidden, minimized, maximized or
/// full screen, it takes now.
pub fn refit(window: &Window) {
    let Some(shape) = window.try_state::<Shape>() else {
        return;
    };
    if let Err(e) = fit(
        window,
        &mut shape.0.lock().unwrap_or_else(PoisonError::into_inner),
    ) {
        log::warn!("창 모양을 바꾸지 못했습니다: {e}");
    }
}

/// A hidden, maximized, full-screen or minimized window keeps the size the recorder gave it.
fn fit(window: &Window, s: &mut Shapes) -> tauri::Result<()> {
    if s.tall == s.want
        || !window.is_visible()?
        || window.is_maximized()?
        || window.is_fullscreen()?
        || window.is_minimized()?
    {
        return Ok(());
    }
    let scale = window.scale_factor()?;
    let inner = window.inner_size()?.to_logical::<f64>(scale);
    let outer = window.outer_size()?.to_logical::<f64>(scale);
    let now = (inner.width, inner.height);
    if Instant::now() >= s.gliding_until && !s.placed.is_some_and(|p| near(p, now)) {
        s.sizes[usize::from(s.tall)] = now;
    }
    // The frame keeps its own size around the page (Windows' invisible resize borders).
    let (dw, dh) = (outer.width - inner.width, outer.height - inner.height);
    let (w, h) = s.sizes[usize::from(s.want)];
    let Some((w, h)) = glide(window, (w + dw, h + dh), s.animate)? else {
        return Ok(());
    };
    s.tall = s.want;
    s.placed = Some((w - dw, h - dh));
    // Some room past the glide's end: macOS paces it by the display. Never earlier than a glide
    // still running.
    let end = Instant::now() + if s.animate { GLIDE * 2 } else { Duration::ZERO };
    s.gliding_until = s.gliding_until.max(end);
    Ok(())
}

/// AppKit animates the frame itself: an animator proxy inside an animation group with our curve.
/// No glide is a group of no length, which also stops one still running. The frame's logical size
/// as placed, or `None` when the window is on no screen.
#[cfg(target_os = "macos")]
fn glide(window: &Window, size: Size, animate: bool) -> tauri::Result<Option<Size>> {
    use objc2::runtime::{AnyClass, AnyObject, MessageReceiver};
    use objc2::{msg_send, sel};
    use objc2_core_foundation::{CGPoint, CGRect, CGSize};

    let rect = |r: CGRect| Rect {
        x: r.origin.x,
        y: r.origin.y,
        w: r.size.width,
        h: r.size.height,
    };
    let ns = window.ns_window()?.cast::<AnyObject>();
    // SAFETY: the app's live NSWindow, messaged on the main thread (a command that is not async
    // and a window event both run there) with AppKit's selectors and their argument and return
    // types.
    unsafe {
        let screen: *mut AnyObject = msg_send![ns, screen];
        let group = AnyClass::get(c"NSAnimationContext");
        let (Some(group), false) = (group, screen.is_null()) else {
            return Ok(None);
        };
        let (frame, area): (CGRect, CGRect) =
            (msg_send![ns, frame], msg_send![screen, visibleFrame]);
        let to = place(rect(frame), size, rect(area));
        let _: () = msg_send![group, beginGrouping];
        let current: *mut AnyObject = msg_send![group, currentContext];
        let length = if animate { GLIDE.as_secs_f64() } else { 0.0 };
        let _: () = msg_send![current, setDuration: length];
        // motion.ts `ease.out`.
        if let Some(curve) = AnyClass::get(c"CAMediaTimingFunction") {
            let curve: *mut AnyObject = curve.send_message(
                sel!(functionWithControlPoints::::),
                (0.22f32, 1f32, 0.36f32, 1f32),
            );
            let _: () = msg_send![current, setTimingFunction: curve];
        }
        let animator: *mut AnyObject = msg_send![ns, animator];
        let frame = CGRect::new(CGPoint::new(to.x, to.y), CGSize::new(to.w, to.h));
        let _: () = msg_send![animator, setFrame: frame, display: true];
        let _: () = msg_send![group, endGrouping];
        Ok(Some((to.w, to.h)))
    }
}

/// Windows has no animated resize: a thread sets the whole frame at once each step, so the size and
/// the place never part. A newer glide stops an older one, and one step at a time, so an older
/// step can never land after a newer one. A recorder who maximizes or minimizes the window stops
/// it too. The frame's logical size as placed, or `None` when the window is on no monitor.
#[cfg(windows)]
fn glide(window: &Window, (w, h): Size, animate: bool) -> tauri::Result<Option<Size>> {
    use std::sync::atomic::{AtomicU64, Ordering};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        IsIconic, IsZoomed, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos,
    };

    static RUN: AtomicU64 = AtomicU64::new(0);
    // Steps only take it, never the window's own thread, which they wait on.
    static STEP: Mutex<()> = Mutex::new(());
    let Some(monitor) = window.current_monitor()? else {
        return Ok(None);
    };
    let (scale, at, now) = (
        window.scale_factor()?,
        window.outer_position()?,
        window.outer_size()?,
    );
    let area = monitor.work_area();
    let from = Rect {
        x: at.x.into(),
        y: at.y.into(),
        w: now.width.into(),
        h: now.height.into(),
    };
    let area = Rect {
        x: area.position.x.into(),
        y: area.position.y.into(),
        w: area.size.width.into(),
        h: area.size.height.into(),
    };
    let to = place(from, (w * scale, h * scale), area);
    // A raw pointer is not Send; the handle is only a number to the other thread.
    let hwnd = window.hwnd()?.0 as usize;
    let run = RUN.fetch_add(1, Ordering::SeqCst) + 1;
    std::thread::spawn(move || {
        let start = Instant::now();
        loop {
            let t = if animate {
                (start.elapsed().as_secs_f64() / GLIDE.as_secs_f64()).min(1.0)
            } else {
                1.0
            };
            let r = between(from, to, t);
            {
                let _step = STEP.lock().unwrap_or_else(PoisonError::into_inner);
                // SAFETY: our window's handle; these send their messages to the window's own
                // thread and wait, which also paces this loop. A closed window only fails them.
                unsafe {
                    let h = hwnd as _;
                    if RUN.load(Ordering::SeqCst) != run || IsIconic(h) != 0 || IsZoomed(h) != 0 {
                        break;
                    }
                    SetWindowPos(
                        h,
                        std::ptr::null_mut(),
                        r.x.round() as i32,
                        r.y.round() as i32,
                        r.w.round() as i32,
                        r.h.round() as i32,
                        SWP_NOZORDER | SWP_NOACTIVATE,
                    );
                }
            }
            if t >= 1.0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(8));
        }
    });
    Ok(Some((to.w / scale, to.h / scale)))
}

/// `t` (0 to 1) of the way from `a` to `b` on motion.ts `ease.out`, whose curve is CSS's stand-in
/// for this ease-out quint.
#[cfg(any(windows, test))]
fn between(a: Rect, b: Rect, t: f64) -> Rect {
    let k = 1.0 - (1.0 - t).powi(5);
    let at = |a: f64, b: f64| a + (b - a) * k;
    Rect {
        x: at(a.x, b.x),
        y: at(a.y, b.y),
        w: at(a.w, b.w),
        h: at(a.h, b.h),
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
fn glide(window: &Window, (w, h): Size, _animate: bool) -> tauri::Result<Option<Size>> {
    window.set_size(tauri::LogicalSize::new(w, h))?;
    Ok(Some((w, h)))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        w: 1440.0,
        h: 900.0,
    };

    #[test]
    fn a_new_shape_keeps_the_window_center() {
        let wide = Rect {
            x: 200.0,
            y: 90.0,
            w: 1040.0,
            h: 720.0,
        };
        let tall = place(wide, TALL, SCREEN);
        assert_eq!(
            tall,
            Rect {
                x: 460.0,
                y: 10.0,
                w: 520.0,
                h: 880.0
            }
        );
        // And back to where it was.
        assert_eq!(place(tall, WIDE, SCREEN), wide);
    }

    #[test]
    fn round_trips_never_creep() {
        // An odd height (macOS reports 721 for a 720 window) splits its difference unevenly.
        let screen = Rect {
            x: 0.0,
            y: 0.0,
            w: 1728.0,
            h: 1080.0,
        };
        let mut wide = Rect {
            x: 335.0,
            y: 211.0,
            w: 1040.0,
            h: 721.0,
        };
        for _ in 0..10 {
            wide = place(place(wide, TALL, screen), (1040.0, 721.0), screen);
        }
        assert_eq!(
            wide,
            Rect {
                x: 335.0,
                y: 211.0,
                w: 1040.0,
                h: 721.0
            }
        );
    }

    #[test]
    fn a_new_shape_stays_inside_the_screen() {
        // At the right edge, it grows to the left; on a short screen, it is only as tall as that.
        let edge = Rect {
            x: 920.0,
            y: 100.0,
            w: 520.0,
            h: 700.0,
        };
        assert_eq!(
            place(edge, WIDE, SCREEN),
            Rect {
                x: 400.0,
                y: 90.0,
                w: 1040.0,
                h: 720.0
            }
        );
        let short = Rect {
            x: 0.0,
            y: 40.0,
            w: 1366.0,
            h: 688.0,
        };
        assert_eq!(place(short, TALL, short).h, 688.0);
        assert_eq!(place(short, TALL, short).y, 40.0);
    }

    #[test]
    fn a_glide_starts_and_ends_on_its_frames() {
        let (a, b) = (
            Rect {
                x: 0.0,
                y: 0.0,
                w: 100.0,
                h: 100.0,
            },
            Rect {
                x: 50.0,
                y: 0.0,
                w: 300.0,
                h: 50.0,
            },
        );
        assert_eq!(between(a, b, 0.0), a);
        assert_eq!(between(a, b, 1.0), b);
        // Most of the way early, like every ease-out.
        assert!(between(a, b, 0.5).w > 275.0);
    }
}
