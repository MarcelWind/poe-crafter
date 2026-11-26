use std::mem::{size_of, zeroed};
use std::ptr::null_mut;
use std::thread::sleep;
use std::time::{Duration, Instant};

use clipboard_win::{get_clipboard_string, set_clipboard_string};
use thiserror::Error;
use winapi::shared::minwindef::{BOOL, DWORD, LPARAM, TRUE};
use winapi::shared::windef::{HWND, POINT, RECT};
use winapi::um::handleapi::CloseHandle;
use winapi::um::processthreadsapi::{GetCurrentThreadId, OpenProcess};
use winapi::um::winbase::QueryFullProcessImageNameW;
use winapi::um::winnt::PROCESS_QUERY_LIMITED_INFORMATION;
use winapi::um::winuser::{
    AttachThreadInput, BringWindowToTop, ClientToScreen, EnumWindows, GetAsyncKeyState,
    GetClientRect, GetCursorPos, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, SendInput, SetCursorPos, SetForegroundWindow, SetProcessDPIAware,
    ShowWindow, INPUT, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_KEYUP,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEINPUT, SW_RESTORE, VK_CONTROL, VK_F12, VK_F8, VK_MENU, VK_SHIFT,
};

use crate::calibration::NormPoint;

const WINDOW_TITLE_BUFFER: usize = 512;
const VK_C_KEY: u32 = 0x43;
const CLIPBOARD_TIMEOUT: Duration = Duration::from_millis(2_500);
const CLIPBOARD_POLL: Duration = Duration::from_millis(60);

#[derive(Debug, Error)]
pub enum InputError {
    #[error("game window not found")]
    WindowNotFound,
    #[error("game is not foreground")]
    NotForeground,
    #[error("window client area is empty")]
    EmptyClientArea,
    #[error("point is outside the game client area")]
    PointOutsideWindow,
    #[error("calibration target `{0}` is missing")]
    MissingCalibration(String),
    #[error("clipboard error: {0}")]
    Clipboard(String),
    #[error("input error: {0}")]
    Input(String),
    #[error("timed out waiting for {0}")]
    Timeout(String),
    #[error("cancelled")]
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowInfo {
    pub title: String,
    pub client_width: i32,
    pub client_height: i32,
    pub origin_x: i32,
    pub origin_y: i32,
    pub foreground: bool,
}

struct WindowSearch {
    needle: String,
    found: Option<HWND>,
}

unsafe extern "system" fn enum_window(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let search = &mut *(lparam as *mut WindowSearch);
    if IsWindowVisible(hwnd) == 0 || !process_matches(hwnd) {
        return TRUE;
    }
    let mut buffer = [0u16; WINDOW_TITLE_BUFFER];
    let length = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    if length <= 0 {
        return TRUE;
    }
    let title = String::from_utf16_lossy(&buffer[..length as usize]).to_lowercase();
    if title.contains("path of exile 2") {
        return TRUE;
    }
    if title.contains(&search.needle) {
        search.found = Some(hwnd);
        return 0;
    }
    TRUE
}

unsafe fn process_matches(hwnd: HWND) -> bool {
    let mut pid: DWORD = 0;
    GetWindowThreadProcessId(hwnd, &mut pid);
    if pid == 0 {
        return false;
    }
    let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
    if handle.is_null() {
        return false;
    }
    let mut buffer = [0u16; 1024];
    let mut length = buffer.len() as DWORD;
    let ok = QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut length);
    CloseHandle(handle);
    if ok == 0 || length == 0 {
        return false;
    }
    let path = String::from_utf16_lossy(&buffer[..length as usize]).to_lowercase();
    path.contains("pathofexile") && !path.contains("pathofexile2") && !path.contains("launcher")
}

pub struct Win32Game {
    hwnd: HWND,
    title: String,
}

impl Drop for Win32Game {
    fn drop(&mut self) {
        // Best effort: never leave a synthetic Shift key held if a session exits
        // while a currency stack is attached to the cursor.
        let _ = send_key(VK_SHIFT as u32, true);
    }
}

impl Win32Game {
    pub fn find(title_substring: &str) -> Result<Self, InputError> {
        unsafe {
            SetProcessDPIAware();
        }
        let mut search = WindowSearch {
            needle: title_substring.to_lowercase(),
            found: None,
        };
        unsafe {
            EnumWindows(
                Some(enum_window),
                &mut search as *mut WindowSearch as LPARAM,
            );
        }
        let hwnd = search.found.ok_or(InputError::WindowNotFound)?;
        let mut buffer = [0u16; WINDOW_TITLE_BUFFER];
        let length = unsafe { GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        let title = if length > 0 {
            String::from_utf16_lossy(&buffer[..length as usize])
        } else {
            title_substring.to_string()
        };
        Ok(Self { hwnd, title })
    }

    pub fn window_info(&self) -> Result<WindowInfo, InputError> {
        let (width, height, origin_x, origin_y) = self.client_rect()?;
        Ok(WindowInfo {
            title: self.title.clone(),
            client_width: width,
            client_height: height,
            origin_x,
            origin_y,
            foreground: self.is_foreground(),
        })
    }

    pub fn is_foreground(&self) -> bool {
        unsafe { GetForegroundWindow() == self.hwnd }
    }

    pub fn require_foreground(&self) -> Result<(), InputError> {
        if self.is_foreground() {
            Ok(())
        } else {
            Err(InputError::NotForeground)
        }
    }

    pub fn emergency_stop(&self) -> bool {
        key_down(VK_F12)
    }

    /// Bring the game window to the foreground. The CLI process is normally the
    /// foreground process when the user runs a command, so it may transfer
    /// activation even when Windows would block a background process.
    pub fn focus(&self) -> Result<(), InputError> {
        unsafe {
            if IsIconic(self.hwnd) != 0 {
                ShowWindow(self.hwnd, SW_RESTORE);
            }
            if self.is_foreground() {
                return Ok(());
            }
            let foreground = GetForegroundWindow();
            let current_thread = GetCurrentThreadId();
            let foreground_thread = if foreground.is_null() {
                0
            } else {
                GetWindowThreadProcessId(foreground, null_mut())
            };
            let attached = foreground_thread != 0
                && foreground_thread != current_thread
                && AttachThreadInput(current_thread, foreground_thread, TRUE) != 0;
            BringWindowToTop(self.hwnd);
            SetForegroundWindow(self.hwnd);
            if attached {
                AttachThreadInput(current_thread, foreground_thread, 0);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if self.is_foreground() {
                return Ok(());
            }
            sleep(Duration::from_millis(30));
        }
        Err(InputError::NotForeground)
    }

    pub fn capture_point(&mut self, timeout: Duration) -> Result<NormPoint, InputError> {
        let start = Instant::now();
        while start.elapsed() < timeout {
            if self.emergency_stop() {
                return Err(InputError::Cancelled);
            }
            if self.is_foreground() && key_down(VK_F8) {
                let point = cursor_pos()?;
                let (width, height, origin_x, origin_y) = self.client_rect()?;
                return NormPoint::from_screen(point.x, point.y, origin_x, origin_y, width, height)
                    .ok_or(InputError::PointOutsideWindow);
            }
            sleep(Duration::from_millis(30));
        }
        Err(InputError::Timeout(
            "calibration point (focus game, hover target, press F8)".to_string(),
        ))
    }

    pub fn read_item(&mut self, point: NormPoint) -> Result<String, InputError> {
        self.require_foreground()?;
        let original = cursor_pos()?;
        let result = self.read_item_at(point);
        let _ = set_cursor(original.x, original.y);
        result
    }

    fn read_item_at(&mut self, point: NormPoint) -> Result<String, InputError> {
        self.move_to(point)?;
        self.require_foreground()?;
        self.clear_clipboard()?;
        send_key_combo(&[VK_CONTROL as u32, VK_MENU as u32, VK_C_KEY])?;
        let text = self
            .wait_for_clipboard(|text| text.contains("Item Class:") && text.contains("Rarity:"))?;
        Ok(text)
    }

    pub fn read_currency(&mut self, point: NormPoint) -> Result<u32, InputError> {
        self.require_foreground()?;
        let original = cursor_pos()?;
        let result = self.read_currency_at(point);
        let _ = set_cursor(original.x, original.y);
        result
    }

    fn read_currency_at(&mut self, point: NormPoint) -> Result<u32, InputError> {
        self.move_to(point)?;
        self.require_foreground()?;
        self.clear_clipboard()?;
        send_key_combo(&[VK_CONTROL as u32, VK_C_KEY])?;
        let deadline = Instant::now() + CLIPBOARD_TIMEOUT;
        while Instant::now() < deadline {
            if self.emergency_stop() {
                return Err(InputError::Cancelled);
            }
            if !self.is_foreground() {
                return Err(InputError::NotForeground);
            }
            match get_clipboard_string() {
                Ok(text) if !text.trim().is_empty() => {
                    if let Some(count) = parse_stack_size(&text) {
                        return Ok(count);
                    }
                    return Err(InputError::Input(
                        "clipboard text is not a currency stack".to_string(),
                    ));
                }
                _ => sleep(CLIPBOARD_POLL),
            }
        }
        // An empty currency slot produces no clipboard text. Treat it as zero so
        // the session aborts before attempting a click.
        Ok(0)
    }

    pub fn begin_hold(&mut self, currency_point: NormPoint) -> Result<(), InputError> {
        self.require_foreground()?;
        self.move_to(currency_point)?;
        sleep(Duration::from_millis(100));
        self.mouse_click(false)?;
        sleep(Duration::from_millis(200));
        send_key(VK_SHIFT as u32, false)?;
        sleep(Duration::from_millis(100));
        if self.emergency_stop() {
            let _ = send_key(VK_SHIFT as u32, true);
            return Err(InputError::Cancelled);
        }
        Ok(())
    }

    pub fn use_held(&mut self, item_point: NormPoint) -> Result<(), InputError> {
        self.require_foreground()?;
        self.move_to(item_point)?;
        sleep(Duration::from_millis(100));
        self.mouse_click(true)?;
        sleep(Duration::from_millis(350));
        Ok(())
    }

    pub fn end_hold(&mut self) -> Result<(), InputError> {
        send_key(VK_SHIFT as u32, true)?;
        sleep(Duration::from_millis(200));
        Ok(())
    }

    pub fn apply_currency(
        &mut self,
        currency_point: NormPoint,
        item_point: NormPoint,
    ) -> Result<(), InputError> {
        self.require_foreground()?;
        let original = cursor_pos()?;
        let result = self.apply_currency_steps(currency_point, item_point);
        let _ = set_cursor(original.x, original.y);
        result
    }

    fn apply_currency_steps(
        &mut self,
        currency_point: NormPoint,
        item_point: NormPoint,
    ) -> Result<(), InputError> {
        self.move_to(currency_point)?;
        sleep(Duration::from_millis(100));
        self.mouse_click(false)?;
        sleep(Duration::from_millis(200));
        if self.emergency_stop() {
            return Err(InputError::Cancelled);
        }

        self.move_to(item_point)?;
        sleep(Duration::from_millis(100));
        self.mouse_click(true)?;
        sleep(Duration::from_millis(350));
        if self.emergency_stop() {
            return Err(InputError::Cancelled);
        }

        // The game clears the held currency after applying it. Do not click the
        // currency slot again: doing so picks the remaining stack back up and
        // leaves it attached to the cursor.
        Ok(())
    }

    pub fn wait_for_clipboard(
        &mut self,
        accept: impl Fn(&str) -> bool,
    ) -> Result<String, InputError> {
        let deadline = Instant::now() + CLIPBOARD_TIMEOUT;
        while Instant::now() < deadline {
            if self.emergency_stop() {
                return Err(InputError::Cancelled);
            }
            if !self.is_foreground() {
                return Err(InputError::NotForeground);
            }
            match get_clipboard_string() {
                Ok(text) if accept(&text) => return Ok(text),
                _ => sleep(CLIPBOARD_POLL),
            }
        }
        Err(InputError::Timeout("clipboard text".to_string()))
    }

    fn clear_clipboard(&mut self) -> Result<(), InputError> {
        set_clipboard_string("").map_err(|error| InputError::Clipboard(error.to_string()))
    }

    pub fn clipboard_text(&mut self) -> Result<String, InputError> {
        get_clipboard_string().map_err(|error| InputError::Clipboard(error.to_string()))
    }

    fn move_to(&self, point: NormPoint) -> Result<(), InputError> {
        let (width, height, origin_x, origin_y) = self.client_rect()?;
        let (x, y) = point.to_screen(origin_x, origin_y, width, height);
        if x < origin_x || y < origin_y || x >= origin_x + width || y >= origin_y + height {
            return Err(InputError::PointOutsideWindow);
        }
        set_cursor(x, y)?;
        sleep(Duration::from_millis(60));
        let current = cursor_pos()?;
        if current.x != x || current.y != y {
            return Err(InputError::Input(
                "cursor did not move to the requested point".to_string(),
            ));
        }
        Ok(())
    }

    fn mouse_click(&self, left: bool) -> Result<(), InputError> {
        self.require_foreground()?;
        let (down, up) = if left {
            (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP)
        } else {
            (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP)
        };
        send_mouse(down)?;
        sleep(Duration::from_millis(20));
        send_mouse(up)
    }

    fn client_rect(&self) -> Result<(i32, i32, i32, i32), InputError> {
        let mut rect: RECT = unsafe { zeroed() };
        if unsafe { GetClientRect(self.hwnd, &mut rect) } == 0 {
            return Err(InputError::Input("GetClientRect failed".to_string()));
        }
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        if width <= 0 || height <= 0 {
            return Err(InputError::EmptyClientArea);
        }
        let mut origin = POINT { x: 0, y: 0 };
        if unsafe { ClientToScreen(self.hwnd, &mut origin) } == 0 {
            return Err(InputError::Input("ClientToScreen failed".to_string()));
        }
        Ok((width, height, origin.x, origin.y))
    }
}

fn cursor_pos() -> Result<POINT, InputError> {
    let mut point = POINT { x: 0, y: 0 };
    if unsafe { GetCursorPos(&mut point) } == 0 {
        return Err(InputError::Input("GetCursorPos failed".to_string()));
    }
    Ok(point)
}

fn set_cursor(x: i32, y: i32) -> Result<(), InputError> {
    if unsafe { SetCursorPos(x, y) } == 0 {
        return Err(InputError::Input("SetCursorPos failed".to_string()));
    }
    Ok(())
}

fn key_down(vk: i32) -> bool {
    (unsafe { GetAsyncKeyState(vk) } as u16 & 0x8000) != 0
}

fn send_mouse(flags: DWORD) -> Result<(), InputError> {
    let mut input: INPUT = unsafe { zeroed() };
    input.type_ = INPUT_MOUSE;
    unsafe {
        *input.u.mi_mut() = MOUSEINPUT {
            dx: 0,
            dy: 0,
            mouseData: 0,
            dwFlags: flags,
            time: 0,
            dwExtraInfo: 0,
        };
    }
    send_input(&input)
}

fn send_key(vk: u32, up: bool) -> Result<(), InputError> {
    let mut input: INPUT = unsafe { zeroed() };
    input.type_ = INPUT_KEYBOARD;
    unsafe {
        *input.u.ki_mut() = KEYBDINPUT {
            wVk: vk as u16,
            wScan: 0,
            dwFlags: if up { KEYEVENTF_KEYUP } else { 0 },
            time: 0,
            dwExtraInfo: 0,
        };
    }
    send_input(&input)
}

fn send_key_combo(keys: &[u32]) -> Result<(), InputError> {
    for key in keys {
        send_key(*key, false)?;
        sleep(Duration::from_millis(8));
    }
    for key in keys.iter().rev() {
        send_key(*key, true)?;
        sleep(Duration::from_millis(8));
    }
    Ok(())
}

fn send_input(input: &INPUT) -> Result<(), InputError> {
    let sent = unsafe {
        SendInput(
            1,
            input as *const INPUT as *mut INPUT,
            size_of::<INPUT>() as i32,
        )
    };
    if sent != 1 {
        return Err(InputError::Input("SendInput failed".to_string()));
    }
    Ok(())
}

fn parse_stack_size(text: &str) -> Option<u32> {
    let marker = text.find("Stack Size:")?;
    let rest = &text[marker + "Stack Size:".len()..];
    let digits = rest
        .trim_start()
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>();
    digits.parse().ok()
}
