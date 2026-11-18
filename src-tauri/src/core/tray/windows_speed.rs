use crate::utils::speed::format_bytes_per_second;
use anyhow::{Context as _, Result, bail};
use parking_lot::Mutex;
use std::ptr::{null, null_mut};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::thread::{self, JoinHandle};
use std::time::Duration;
use windows_sys::Win32::{
    Foundation::{ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
    Graphics::Gdi::{
        AC_SRC_ALPHA, AC_SRC_OVER, ANTIALIASED_QUALITY, BLENDFUNCTION, BeginPaint, CLIP_DEFAULT_PRECIS, CreateFontW,
        CreateSolidBrush, DEFAULT_CHARSET, DEFAULT_PITCH, DT_LEFT, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER,
        DeleteObject, DrawTextW, EndPaint, FF_DONTCARE, FW_NORMAL, FillRect, GdiFlush, GetDC, HDC, InvalidateRect,
        MapWindowPoints, OUT_DEFAULT_PRECIS, PAINTSTRUCT, RGBQUAD, ReleaseDC, SelectObject, SetBkMode, SetTextColor,
        TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        Controls::{
            BPBF_TOPDOWNDIB, BeginBufferedPaint, BufferedPaintInit, BufferedPaintSetAlpha, BufferedPaintUnInit,
            EndBufferedPaint, GetBufferedPaintBits,
        },
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW, FindWindowW, GWL_EXSTYLE,
            GetClientRect, GetWindowLongPtrW, GetWindowRect, GetWindowTextW, IsWindow, MA_NOACTIVATE, MSG, PM_REMOVE,
            PeekMessageW, RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos,
            SetWindowTextW, ShowWindow, TranslateMessage, ULW_ALPHA, UpdateLayeredWindow, WM_CREATE, WM_ERASEBKGND,
            WM_MOUSEACTIVATE, WM_NCDESTROY, WM_PAINT, WNDCLASSW, WS_CHILD, WS_EX_LAYERED, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW,
        },
    },
};
use windows_sys::w;

#[derive(Default)]
struct DisplayState {
    speed: Mutex<(u64, u64)>,
    stopped: AtomicBool,
}

pub(super) struct TaskbarSpeedDisplay {
    state: Arc<DisplayState>,
    thread: Option<JoinHandle<()>>,
}

impl TaskbarSpeedDisplay {
    pub fn new() -> Result<Self> {
        let state = Arc::new(DisplayState::default());
        let shared = Arc::clone(&state);
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new().name("taskbar-speed".into()).spawn(move || {
            let mut window = match TaskbarWindow::new() {
                Ok(window) => window,
                Err(error) => {
                    let _ = ready_tx.send(Err(error));
                    return;
                }
            };
            if ready_tx.send(Ok(())).is_err() {
                return;
            }
            while !shared.stopped.load(Ordering::Acquire) {
                let attached = window.refresh(*shared.speed.lock()).is_ok();
                unsafe {
                    if !attached {
                        ShowWindow(window.window, SW_HIDE);
                    }
                    let mut message: MSG = std::mem::zeroed();
                    while PeekMessageW(&mut message, null_mut(), 0, 0, PM_REMOVE) != 0 {
                        TranslateMessage(&message);
                        DispatchMessageW(&message);
                    }
                }
                thread::sleep(Duration::from_millis(200));
            }
        })?;
        if let Err(error) = ready_rx.recv().context("Taskbar display thread stopped")? {
            let _ = thread.join();
            return Err(error);
        }
        Ok(Self {
            state,
            thread: Some(thread),
        })
    }

    pub fn set_speed(&self, up: u64, down: u64) {
        *self.state.speed.lock() = (up, down);
    }
}

impl Drop for TaskbarSpeedDisplay {
    fn drop(&mut self) {
        self.state.stopped.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Reservation {
    window: HWND,
    parent: HWND,
    original: RECT,
    reserved: RECT,
}

impl Drop for Reservation {
    fn drop(&mut self) {
        // Explorer may already have laid out a new task list; only undo our own resize.
        if rect_in(self.window, self.parent).is_some_and(|rect| same_rect(rect, self.reserved)) {
            position(self.window, self.original, SWP_NOZORDER | SWP_NOACTIVATE);
        }
    }
}

struct TaskbarWindow {
    window: HWND,
    parent: HWND,
    reservation: Option<Reservation>,
    text: String,
}

impl TaskbarWindow {
    fn new() -> Result<Self> {
        unsafe {
            let class = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: GetModuleHandleW(null()),
                lpszClassName: w!("ClashVergeTaskbarSpeed"),
                ..std::mem::zeroed()
            };
            if RegisterClassW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                bail!(
                    "Could not register the taskbar speed window class: {}",
                    std::io::Error::last_os_error()
                );
            }
        }
        let mut window = Self {
            window: null_mut(),
            parent: null_mut(),
            reservation: None,
            text: String::new(),
        };
        if let Err(error) = window.refresh((0, 0)) {
            bail!("Could not attach the speed display to the Windows taskbar: {error:#}");
        }
        Ok(window)
    }

    fn refresh(&mut self, speed: (u64, u64)) -> Result<()> {
        unsafe {
            let taskbar = FindWindowW(w!("Shell_TrayWnd"), null());
            if taskbar.is_null() {
                self.detach();
                bail!("Shell_TrayWnd was not found");
            }
            let parent = taskbar_parent(taskbar);
            if parent != self.parent || IsWindow(self.window) == 0 {
                self.detach();
                self.parent = parent;
                self.window = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | if parent == taskbar { WS_EX_LAYERED } else { 0 },
                    w!("ClashVergeTaskbarSpeed"),
                    w!(""),
                    WS_CHILD,
                    0,
                    0,
                    0,
                    0,
                    parent,
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                if self.window.is_null() {
                    return Err(std::io::Error::last_os_error()).context("CreateWindowExW failed");
                }
                self.text.clear();
            }

            let dpi = GetDpiForWindow(taskbar).max(96) as i32;
            let mut bounds: RECT = std::mem::zeroed();
            if GetClientRect(parent, &mut bounds) == 0 {
                return Err(std::io::Error::last_os_error()).context("GetClientRect failed for the taskbar parent");
            }
            if bounds.right <= 0 || bounds.bottom <= 0 {
                bail!(
                    "The taskbar parent has empty bounds ({}x{})",
                    bounds.right,
                    bounds.bottom
                );
            }
            let width = 72 * dpi / 96;
            let height = (32 * dpi / 96).min(bounds.bottom);
            let area = if parent == taskbar {
                let notify = FindWindowExW(taskbar, null_mut(), w!("TrayNotifyWnd"), null());
                let notify =
                    rect_in(notify, parent).context("Could not read notification area bounds (TrayNotifyWnd)")?;
                if notify.right <= notify.left || notify.bottom <= notify.top {
                    bail!("The notification area (TrayNotifyWnd) has empty bounds");
                }
                if notify.left < width {
                    bail!(
                        "Not enough space before the notification area ({}px available, {width}px required)",
                        notify.left
                    );
                }
                RECT {
                    left: notify.left - width,
                    right: notify.left,
                    top: (bounds.bottom - height) / 2,
                    bottom: (bounds.bottom + height) / 2,
                }
            } else {
                let mut list = FindWindowExW(parent, null_mut(), w!("MSTaskSwWClass"), null());
                if list.is_null() {
                    list = FindWindowExW(parent, null_mut(), w!("MSTaskListWClass"), null());
                }
                self.reserve(list, bounds, width, height)?
            };
            if !position(self.window, area, SWP_SHOWWINDOW | SWP_NOACTIVATE) {
                return Err(std::io::Error::last_os_error()).context("SetWindowPos failed for the speed display");
            }
            let text = format!(
                "↑ {}\n↓ {}",
                format_bytes_per_second(speed.0),
                format_bytes_per_second(speed.1)
            );
            if self.text != text {
                let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
                SetWindowTextW(self.window, wide.as_ptr());
                self.text = text;
            }
            if parent == taskbar {
                paint_layered(self.window)?;
            } else {
                InvalidateRect(self.window, null(), 0);
            }
            Ok(())
        }
    }

    fn reserve(&mut self, list: HWND, bounds: RECT, width: i32, height: i32) -> Result<RECT> {
        let current = rect_in(list, self.parent)
            .context("Could not read classic task-list bounds (MSTaskSwWClass/MSTaskListWClass)")?;
        let horizontal = bounds.right >= bounds.bottom;
        let unchanged = self.reservation.as_ref().is_some_and(|reservation| {
            reservation.window == list
                && same_rect(current, reservation.reserved)
                && if horizontal {
                    reservation.original.right - reservation.reserved.right == width
                } else {
                    reservation.original.bottom - reservation.reserved.bottom == height
                }
        });
        if !unchanged {
            self.reservation.take();
            let original = rect_in(list, self.parent).context("Could not read restored classic task-list bounds")?;
            let mut reserved = original;
            if horizontal {
                if original.right - original.left < width * 2 {
                    bail!("The classic task list is too narrow to reserve {width}px");
                }
                reserved.right -= width;
            } else {
                if original.bottom - original.top < height * 2 {
                    bail!("The classic task list is too short to reserve {height}px");
                }
                reserved.bottom -= height;
            }
            if !position(list, reserved, SWP_NOZORDER | SWP_NOACTIVATE) {
                return Err(std::io::Error::last_os_error()).context("SetWindowPos failed for the classic task list");
            }
            self.reservation = Some(Reservation {
                window: list,
                parent: self.parent,
                original,
                reserved,
            });
        }
        let reservation = self
            .reservation
            .as_ref()
            .context("The classic task-list reservation is missing")?;
        Ok(if horizontal {
            RECT {
                left: reservation.reserved.right,
                right: reservation.original.right,
                top: (bounds.bottom - height) / 2,
                bottom: (bounds.bottom + height) / 2,
            }
        } else {
            RECT {
                left: 0,
                right: bounds.right,
                top: reservation.reserved.bottom,
                bottom: reservation.original.bottom,
            }
        })
    }

    fn detach(&mut self) {
        self.reservation.take();
        unsafe {
            if IsWindow(self.window) != 0 {
                DestroyWindow(self.window);
            }
        }
        self.window = null_mut();
        self.parent = null_mut();
    }
}

impl Drop for TaskbarWindow {
    fn drop(&mut self) {
        self.detach();
    }
}

fn taskbar_parent(taskbar: HWND) -> HWND {
    unsafe {
        let bridge = FindWindowExW(
            taskbar,
            null_mut(),
            w!("Windows.UI.Composition.DesktopWindowContentBridge"),
            null(),
        );
        if !bridge.is_null() {
            return taskbar;
        }
        let rebar = FindWindowExW(taskbar, null_mut(), w!("ReBarWindow32"), null());
        if rebar.is_null() { taskbar } else { rebar }
    }
}

fn same_rect(a: RECT, b: RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

fn rect_in(window: HWND, parent: HWND) -> Option<RECT> {
    unsafe {
        let mut rect: RECT = std::mem::zeroed();
        if window.is_null() || GetWindowRect(window, &mut rect) == 0 {
            return None;
        }
        MapWindowPoints(null_mut(), parent, (&mut rect as *mut RECT).cast(), 2);
        Some(rect)
    }
}

fn position(window: HWND, rect: RECT, flags: u32) -> bool {
    unsafe {
        SetWindowPos(
            window,
            null_mut(),
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            flags,
        ) != 0
    }
}

unsafe extern "system" fn window_proc(window: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        match message {
            WM_CREATE => {
                BufferedPaintInit();
                0
            }
            WM_NCDESTROY => {
                BufferedPaintUnInit();
                DefWindowProcW(window, message, wparam, lparam)
            }
            WM_PAINT => {
                paint(window);
                0
            }
            WM_ERASEBKGND => 1,
            WM_MOUSEACTIVATE => MA_NOACTIVATE as LRESULT,
            _ => DefWindowProcW(window, message, wparam, lparam),
        }
    }
}

fn theme_colors() -> (u32, u32) {
    let light = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
        .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
        .unwrap_or(0)
        != 0;
    if light {
        (0x00f3f3f3, 0x00181818)
    } else {
        (0x00202020, 0x00ffffff)
    }
}

fn paint(window: HWND) {
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let target = BeginPaint(window, &mut paint);
        if GetWindowLongPtrW(window, GWL_EXSTYLE) as u32 & WS_EX_LAYERED != 0 {
            EndPaint(window, &paint);
            return;
        }
        let mut bounds: RECT = std::mem::zeroed();
        GetClientRect(window, &mut bounds);
        let mut dc = null_mut();
        let buffer = BeginBufferedPaint(target, &bounds, BPBF_TOPDOWNDIB, null(), &mut dc);
        if buffer == 0 {
            EndPaint(window, &paint);
            return;
        }
        let (background, foreground) = theme_colors();
        draw_content(window, dc, bounds, background, foreground);
        // GDI leaves alpha unset; glass taskbars otherwise blend dark text into their light background.
        BufferedPaintSetAlpha(buffer, null(), 255);
        EndBufferedPaint(buffer, 1);
        EndPaint(window, &paint);
    }
}

fn paint_layered(window: HWND) -> Result<()> {
    unsafe {
        let mut bounds: RECT = std::mem::zeroed();
        if GetClientRect(window, &mut bounds) == 0 {
            return Err(std::io::Error::last_os_error()).context("Could not read the speed display bounds");
        }
        let screen = GetDC(null_mut());
        if screen.is_null() {
            bail!("Could not get a screen device context for taskbar drawing");
        }
        let mut dc = null_mut();
        let buffer = BeginBufferedPaint(screen, &bounds, BPBF_TOPDOWNDIB, null(), &mut dc);
        if buffer == 0 {
            ReleaseDC(null_mut(), screen);
            bail!("Could not create the taskbar drawing buffer");
        }
        let result = (|| {
            // Grayscale glyph coverage avoids ClearType fringes on the transparent taskbar surface.
            draw_content(window, dc, bounds, 0, 0x00ffffff);
            GdiFlush();
            let mut bits = null_mut();
            let mut stride = 0;
            if GetBufferedPaintBits(buffer, &mut bits, &mut stride) < 0 || bits.is_null() || stride < bounds.right {
                bail!("Could not read the taskbar drawing buffer pixels");
            }
            let (_, foreground) = theme_colors();
            for y in 0..bounds.bottom {
                for x in 0..bounds.right {
                    let pixel = &mut *bits.add((y * stride + x) as usize);
                    *pixel = glyph_pixel(pixel.rgbRed, foreground);
                }
            }
            let size = SIZE {
                cx: bounds.right,
                cy: bounds.bottom,
            };
            let source = POINT { x: 0, y: 0 };
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            if UpdateLayeredWindow(window, screen, null(), &size, dc, &source, 0, &blend, ULW_ALPHA) == 0 {
                return Err(std::io::Error::last_os_error())
                    .context("UpdateLayeredWindow failed for the speed display");
            }
            Ok(())
        })();
        EndBufferedPaint(buffer, 0);
        ReleaseDC(null_mut(), screen);
        result
    }
}

fn glyph_pixel(coverage: u8, foreground: u32) -> RGBQUAD {
    let alpha = u32::from(coverage);
    RGBQUAD {
        rgbRed: ((foreground & 0xff) * alpha / 255) as u8,
        rgbGreen: (((foreground >> 8) & 0xff) * alpha / 255) as u8,
        rgbBlue: (((foreground >> 16) & 0xff) * alpha / 255) as u8,
        rgbReserved: coverage,
    }
}

fn draw_content(window: HWND, dc: HDC, bounds: RECT, background: u32, foreground: u32) {
    unsafe {
        let brush = CreateSolidBrush(background);
        FillRect(dc, &bounds, brush);
        DeleteObject(brush);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, foreground);
        let dpi = GetDpiForWindow(window).max(96) as i32;
        let font = CreateFontW(
            -(12 * dpi / 96),
            0,
            0,
            0,
            FW_NORMAL as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET as u32,
            OUT_DEFAULT_PRECIS as u32,
            CLIP_DEFAULT_PRECIS as u32,
            ANTIALIASED_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(dc, font);
        let mut text = [0u16; 128];
        let length = GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32) as usize;
        let middle = bounds.bottom / 2;
        let padding = 6 * dpi / 96;
        let arrow_right = padding + 10 * dpi / 96;
        let gap = 2 * dpi / 96;
        for (index, line) in text[..length].split(|ch| *ch == 10).take(2).enumerate() {
            if line.len() < 6 {
                continue;
            }
            // The shared formatter's units are all three characters (B/s, K/s, M/s, G/s, T/s).
            let unit_start = line.len() - 3;
            let mut rate = line[2..unit_start].to_vec();
            rate.push(u16::from(b' '));
            rate.extend_from_slice(&line[unit_start..]);
            for (part, left, right) in [
                (&line[..1], padding, arrow_right),
                (rate.as_slice(), arrow_right + gap, bounds.right - padding),
            ] {
                let mut rect = RECT {
                    left,
                    right,
                    top: index as i32 * middle,
                    bottom: (index as i32 + 1) * middle,
                };
                DrawTextW(
                    dc,
                    part.as_ptr(),
                    part.len() as i32,
                    &mut rect,
                    DT_LEFT | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
                );
            }
        }
        SelectObject(dc, old_font);
        DeleteObject(font);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn glyph_alpha_has_no_opaque_background_or_color_fringe() {
        for coverage in [0, 128, 255] {
            let light = glyph_pixel(coverage, 0x00181818);
            let dark = glyph_pixel(coverage, 0x00ffffff);
            assert_eq!(light.rgbReserved, coverage);
            assert_eq!(light.rgbRed, (24 * u32::from(coverage) / 255) as u8);
            assert_eq!((light.rgbRed, light.rgbRed), (light.rgbGreen, light.rgbBlue));
            assert_eq!(
                (dark.rgbRed, dark.rgbGreen, dark.rgbBlue, dark.rgbReserved),
                (coverage, coverage, coverage, coverage)
            );
        }
    }

    #[test]
    fn executable_manifest_allows_layered_child_windows() {
        unsafe {
            let module = GetModuleHandleW(null());
            let parent = CreateWindowExW(
                0,
                w!("STATIC"),
                w!(""),
                0,
                0,
                0,
                100,
                40,
                null_mut(),
                null_mut(),
                module,
                null(),
            );
            assert!(!parent.is_null());
            let child = CreateWindowExW(
                WS_EX_LAYERED,
                w!("STATIC"),
                w!("↑ 1.0M/s\n↓ 2.0M/s"),
                WS_CHILD,
                0,
                0,
                100,
                40,
                parent,
                null_mut(),
                module,
                null(),
            );
            BufferedPaintInit();
            let rendered = paint_layered(child);
            BufferedPaintUnInit();
            DestroyWindow(parent);
            assert!(rendered.is_ok(), "Layered child rendering failed: {rendered:?}");
        }
    }

    #[test]
    fn modern_taskbar_does_not_use_the_legacy_rebar_container() {
        unsafe {
            let module = GetModuleHandleW(null());
            for name in [
                w!("ReBarWindow32"),
                w!("Windows.UI.Composition.DesktopWindowContentBridge"),
            ] {
                RegisterClassW(&WNDCLASSW {
                    lpfnWndProc: Some(DefWindowProcW),
                    hInstance: module,
                    lpszClassName: name,
                    ..std::mem::zeroed()
                });
            }
            let taskbar = CreateWindowExW(
                0,
                w!("STATIC"),
                w!(""),
                0,
                0,
                0,
                1000,
                48,
                null_mut(),
                null_mut(),
                module,
                null(),
            );
            assert!(!taskbar.is_null());
            let child = |name| {
                CreateWindowExW(
                    0,
                    name,
                    w!(""),
                    WS_CHILD,
                    0,
                    0,
                    0,
                    0,
                    taskbar,
                    null_mut(),
                    module,
                    null(),
                )
            };
            let rebar = child(w!("ReBarWindow32"));
            assert!(!rebar.is_null());
            assert_eq!(taskbar_parent(taskbar), rebar);

            let bridge = child(w!("Windows.UI.Composition.DesktopWindowContentBridge"));
            assert!(!bridge.is_null());
            assert_eq!(taskbar_parent(taskbar), taskbar);

            DestroyWindow(bridge);
            assert_eq!(taskbar_parent(taskbar), rebar);
            DestroyWindow(rebar);
            assert_eq!(taskbar_parent(taskbar), taskbar);
            DestroyWindow(taskbar);
        }
    }

    #[test]
    fn task_list_reservation_is_idempotent_and_restores_only_its_own_resize() {
        for horizontal in [true, false] {
            unsafe {
                let parent = CreateWindowExW(
                    0,
                    w!("STATIC"),
                    w!(""),
                    0,
                    0,
                    0,
                    600,
                    600,
                    null_mut(),
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                assert!(!parent.is_null());
                let (width, height) = if horizontal { (500, 40) } else { (60, 500) };
                let list = CreateWindowExW(
                    0,
                    w!("STATIC"),
                    w!(""),
                    WS_CHILD,
                    0,
                    0,
                    width,
                    height,
                    parent,
                    null_mut(),
                    GetModuleHandleW(null()),
                    null(),
                );
                assert!(!list.is_null());
                let original = rect_in(list, parent).expect("task list rectangle");
                let bounds = RECT {
                    left: 0,
                    top: 0,
                    right: width,
                    bottom: height,
                };
                let mut display = TaskbarWindow {
                    window: null_mut(),
                    parent,
                    reservation: None,
                    text: String::new(),
                };
                let area = display.reserve(list, bounds, 92, 32).expect("reserved area");
                let reserved = rect_in(list, parent).expect("resized task list");
                assert_eq!(
                    if horizontal { reserved.right } else { reserved.bottom },
                    if horizontal { width - 92 } else { height - 32 }
                );
                assert!(same_rect(
                    area,
                    display.reserve(list, bounds, 92, 32).expect("same area")
                ));
                assert!(same_rect(reserved, rect_in(list, parent).expect("unchanged task list")));
                display.reservation.take();
                assert!(same_rect(original, rect_in(list, parent).expect("restored task list")));

                display.reserve(list, bounds, 92, 32).expect("reserve again");
                let external = RECT {
                    right: width / 2,
                    bottom: height / 2,
                    ..original
                };
                assert!(position(list, external, SWP_NOZORDER | SWP_NOACTIVATE));
                drop(display);
                assert!(same_rect(
                    external,
                    rect_in(list, parent).expect("preserved Explorer layout")
                ));
                DestroyWindow(parent);
            }
        }
    }
}
