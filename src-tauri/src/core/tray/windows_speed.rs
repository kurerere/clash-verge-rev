use crate::utils::speed::format_bytes_per_second;
use anyhow::{Context, Result, bail};
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
    Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM},
    Graphics::Gdi::{
        BeginPaint, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, CreateFontW, CreateSolidBrush, DEFAULT_CHARSET,
        DEFAULT_PITCH, DT_CENTER, DT_NOPREFIX, DT_SINGLELINE, DT_VCENTER, DeleteObject, DrawTextW, EndPaint,
        FF_DONTCARE, FW_NORMAL, FillRect, InvalidateRect, MapWindowPoints, OUT_DEFAULT_PRECIS, PAINTSTRUCT,
        SelectObject, SetBkMode, SetTextColor, TRANSPARENT,
    },
    System::LibraryLoader::GetModuleHandleW,
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, FindWindowExW, FindWindowW,
            GetClientRect, GetWindowRect, GetWindowTextW, IsWindow, MA_NOACTIVATE, MSG, PM_REMOVE, PeekMessageW,
            RegisterClassW, SW_HIDE, SWP_NOACTIVATE, SWP_NOZORDER, SWP_SHOWWINDOW, SetWindowPos, SetWindowTextW,
            ShowWindow, TranslateMessage, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_PAINT, WNDCLASSW, WS_CHILD,
            WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
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
                window.refresh(*shared.speed.lock());
                unsafe {
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
            RegisterClassW(&class);
        }
        let mut window = Self {
            window: null_mut(),
            parent: null_mut(),
            reservation: None,
            text: String::new(),
        };
        if !window.refresh((0, 0)) {
            bail!("Could not attach the speed display to the Windows taskbar");
        }
        Ok(window)
    }

    fn refresh(&mut self, speed: (u64, u64)) -> bool {
        unsafe {
            let taskbar = FindWindowW(w!("Shell_TrayWnd"), null());
            if taskbar.is_null() {
                self.detach();
                return false;
            }
            let rebar = FindWindowExW(taskbar, null_mut(), w!("ReBarWindow32"), null());
            let parent = if rebar.is_null() { taskbar } else { rebar };
            if parent != self.parent || IsWindow(self.window) == 0 {
                self.detach();
                self.parent = parent;
                self.window = CreateWindowExW(
                    WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
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
                    return false;
                }
                self.text.clear();
            }

            let dpi = GetDpiForWindow(taskbar).max(96) as i32;
            let mut bounds: RECT = std::mem::zeroed();
            if GetClientRect(parent, &mut bounds) == 0 {
                return false;
            }
            let width = 92 * dpi / 96;
            let height = (32 * dpi / 96).min(bounds.bottom);
            let area = if rebar.is_null() {
                let notify = FindWindowExW(taskbar, null_mut(), w!("TrayNotifyWnd"), null());
                rect_in(notify, parent).and_then(|notify| {
                    (notify.left >= width).then_some(RECT {
                        left: notify.left - width,
                        right: notify.left,
                        top: (bounds.bottom - height) / 2,
                        bottom: (bounds.bottom + height) / 2,
                    })
                })
            } else {
                let mut list = FindWindowExW(rebar, null_mut(), w!("MSTaskSwWClass"), null());
                if list.is_null() {
                    list = FindWindowExW(rebar, null_mut(), w!("MSTaskListWClass"), null());
                }
                self.reserve(list, bounds, width, height)
            };
            let Some(area) = area else {
                ShowWindow(self.window, SW_HIDE);
                return false;
            };
            position(self.window, area, SWP_SHOWWINDOW | SWP_NOACTIVATE);
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
            InvalidateRect(self.window, null(), 0);
            true
        }
    }

    fn reserve(&mut self, list: HWND, bounds: RECT, width: i32, height: i32) -> Option<RECT> {
        let current = rect_in(list, self.parent)?;
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
            let original = rect_in(list, self.parent)?;
            let mut reserved = original;
            if horizontal {
                if original.right - original.left < width * 2 {
                    return None;
                }
                reserved.right -= width;
            } else {
                if original.bottom - original.top < height * 2 {
                    return None;
                }
                reserved.bottom -= height;
            }
            if !position(list, reserved, SWP_NOZORDER | SWP_NOACTIVATE) {
                return None;
            }
            self.reservation = Some(Reservation {
                window: list,
                parent: self.parent,
                original,
                reserved,
            });
        }
        let reservation = self.reservation.as_ref()?;
        Some(if horizontal {
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

fn paint(window: HWND) {
    unsafe {
        let mut paint: PAINTSTRUCT = std::mem::zeroed();
        let dc = BeginPaint(window, &mut paint);
        let mut bounds: RECT = std::mem::zeroed();
        GetClientRect(window, &mut bounds);
        let light = winreg::RegKey::predef(winreg::enums::HKEY_CURRENT_USER)
            .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize")
            .and_then(|key| key.get_value::<u32, _>("SystemUsesLightTheme"))
            .unwrap_or(0)
            != 0;
        let brush = CreateSolidBrush(if light { 0x00f3f3f3 } else { 0x00202020 });
        FillRect(dc, &bounds, brush);
        DeleteObject(brush);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, if light { 0x00181818 } else { 0x00ffffff });
        let font = CreateFontW(
            -(11 * GetDpiForWindow(window).max(96) as i32 / 96),
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
            CLEARTYPE_QUALITY as u32,
            (DEFAULT_PITCH | FF_DONTCARE) as u32,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(dc, font);
        let mut text = [0u16; 128];
        let length = GetWindowTextW(window, text.as_mut_ptr(), text.len() as i32) as usize;
        let middle = bounds.bottom / 2;
        for (index, line) in text[..length].split(|ch| *ch == 10).take(2).enumerate() {
            let mut rect = RECT {
                left: 0,
                right: bounds.right,
                top: index as i32 * middle,
                bottom: (index as i32 + 1) * middle,
            };
            DrawTextW(
                dc,
                line.as_ptr(),
                line.len() as i32,
                &mut rect,
                DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
            );
        }
        SelectObject(dc, old_font);
        DeleteObject(font);
        EndPaint(window, &paint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
