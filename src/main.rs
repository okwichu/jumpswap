#![windows_subsystem = "windows"]

use jumpswap::{
    SENTINEL, any_the_finals_running, any_watched_game_running, is_injected_event,
    remap_virtual_key, should_enable_swap,
};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::System::Registry::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

static SWAP_ENABLED: AtomicBool = AtomicBool::new(false);
static AUTO_DETECT: AtomicBool = AtomicBool::new(true);
static GAME_RUNNING: AtomicBool = AtomicBool::new(false);
static MANUAL_SWAP: AtomicBool = AtomicBool::new(false);
static AUTO_SUPPRESSED: AtomicBool = AtomicBool::new(false);
static FINALS_RES_SWITCH: AtomicBool = AtomicBool::new(true);
static SAVED_DISPLAY: Mutex<Option<(Vec<u16>, DEVMODEW)>> = Mutex::new(None);

const WM_TRAYICON: u32 = WM_USER + 1;
const WM_GAME_STATE: u32 = WM_USER + 2; // posted by detector thread
const WM_FINALS_STATE: u32 = WM_USER + 3; // posted by detector thread (THE FINALS)
const IDM_SWAP: u32 = 1001;
const IDM_AUTO: u32 = 1002;
const IDM_QUIT: u32 = 1003;
const IDM_STARTUP: u32 = 1004;
const IDM_FINALS_RES: u32 = 1005;

const FINALS_RES_WIDTH: u32 = 1920;
const FINALS_RES_HEIGHT: u32 = 1080;
const FINALS_RES_FREQ: u32 = 120;

fn main() -> Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;

        let class_name = w!("JumpSwapClass");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            class_name,
            w!("JumpSwap"),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )?;

        add_tray_icon(hwnd)?;

        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0)?;

        // Start game detection thread
        let hwnd_raw = hwnd.0 as usize;
        thread::spawn(move || game_detector_thread(hwnd_raw));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = UnhookWindowsHookEx(hook);
        remove_tray_icon(hwnd)?;
    }

    Ok(())
}

/// Background thread that polls for game processes every 3 seconds.
fn game_detector_thread(hwnd_raw: usize) {
    let mut was_running = false;
    let mut was_finals = false;
    loop {
        let auto_on = AUTO_DETECT.load(Ordering::SeqCst);

        // THE FINALS detection runs whether or not swap auto-detect is on,
        // since the resolution-switch feature is independent.
        let (game_running, finals_running) = scan_processes();

        let effective_game = if auto_on { game_running } else { false };
        if effective_game != was_running {
            was_running = effective_game;
            GAME_RUNNING.store(effective_game, Ordering::SeqCst);
            unsafe {
                let hwnd = HWND(hwnd_raw as *mut _);
                let _ = PostMessageW(
                    Some(hwnd),
                    WM_GAME_STATE,
                    WPARAM(effective_game as usize),
                    LPARAM(0),
                );
            }
        }

        if finals_running != was_finals {
            was_finals = finals_running;
            unsafe {
                let hwnd = HWND(hwnd_raw as *mut _);
                let _ = PostMessageW(
                    Some(hwnd),
                    WM_FINALS_STATE,
                    WPARAM(finals_running as usize),
                    LPARAM(0),
                );
            }
        }

        thread::sleep(Duration::from_secs(3));
    }
}

/// Single-pass process scan returning (any-watched-game, the-finals).
fn scan_processes() -> (bool, bool) {
    unsafe {
        let snapshot = match CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) {
            Ok(h) => h,
            Err(_) => return (false, false),
        };

        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        let mut any_game = false;
        let mut finals = false;

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let exe_name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len())],
                );

                if !any_game && any_watched_game_running(std::iter::once(exe_name.as_str())) {
                    any_game = true;
                }
                if !finals && any_the_finals_running(std::iter::once(exe_name.as_str())) {
                    finals = true;
                }
                if any_game && finals {
                    break;
                }

                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
        (any_game, finals)
    }
}

/// Find the primary attached display device. Returns its null-terminated
/// `\\.\DISPLAYn` device name as a UTF-16 buffer.
fn find_external_display() -> Option<Vec<u16>> {
    unsafe {
        for i in 0u32.. {
            let mut dd = DISPLAY_DEVICEW {
                cb: size_of::<DISPLAY_DEVICEW>() as u32,
                ..Default::default()
            };
            if !EnumDisplayDevicesW(PCWSTR::null(), i, &mut dd, 0).as_bool() {
                return None;
            }
            let attached = (dd.StateFlags.0 & DISPLAY_DEVICE_ATTACHED_TO_DESKTOP.0) != 0;
            let primary = (dd.StateFlags.0 & DISPLAY_DEVICE_PRIMARY_DEVICE.0) != 0;
            if attached && primary {
                let len = dd
                    .DeviceName
                    .iter()
                    .position(|&c| c == 0)
                    .unwrap_or(dd.DeviceName.len());
                let mut name = dd.DeviceName[..len].to_vec();
                name.push(0);
                return Some(name);
            }
        }
        None
    }
}

/// Apply 1080p@120Hz to the given display, returning the prior DEVMODE on success.
fn apply_resolution(device: &[u16], width: u32, height: u32, freq: u32) -> Option<DEVMODEW> {
    unsafe {
        let mut current = DEVMODEW {
            dmSize: size_of::<DEVMODEW>() as u16,
            ..Default::default()
        };
        if !EnumDisplaySettingsW(PCWSTR(device.as_ptr()), ENUM_CURRENT_SETTINGS, &mut current)
            .as_bool()
        {
            return None;
        }
        let saved = current;

        let mut new_mode = current;
        new_mode.dmPelsWidth = width;
        new_mode.dmPelsHeight = height;
        new_mode.dmDisplayFrequency = freq;
        new_mode.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_DISPLAYFREQUENCY;

        let result = ChangeDisplaySettingsExW(
            PCWSTR(device.as_ptr()),
            Some(&new_mode),
            None,
            CDS_TYPE(0),
            None,
        );
        if result == DISP_CHANGE_SUCCESSFUL {
            Some(saved)
        } else {
            None
        }
    }
}

fn restore_display(device: &[u16], mode: &DEVMODEW) {
    unsafe {
        let _ = ChangeDisplaySettingsExW(
            PCWSTR(device.as_ptr()),
            Some(mode),
            None,
            CDS_TYPE(0),
            None,
        );
    }
}

fn on_finals_started() {
    if !FINALS_RES_SWITCH.load(Ordering::SeqCst) {
        return;
    }
    let mut guard = match SAVED_DISPLAY.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if guard.is_some() {
        // Already applied — don't double-save and clobber the original mode.
        return;
    }
    let Some(device) = find_external_display() else {
        return;
    };
    if let Some(saved) = apply_resolution(&device, FINALS_RES_WIDTH, FINALS_RES_HEIGHT, FINALS_RES_FREQ) {
        *guard = Some((device, saved));
    }
}

fn on_finals_stopped() {
    let mut guard = match SAVED_DISPLAY.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if let Some((device, mode)) = guard.take() {
        restore_display(&device, &mode);
    }
}

/// Recalculate effective swap state from manual toggle + auto-detect.
fn update_swap_state() -> bool {
    let auto = AUTO_DETECT.load(Ordering::SeqCst);
    let game = GAME_RUNNING.load(Ordering::SeqCst);
    let manual = MANUAL_SWAP.load(Ordering::SeqCst);
    let suppressed = AUTO_SUPPRESSED.load(Ordering::SeqCst);

    let effective = should_enable_swap(manual, auto, game, suppressed);
    SWAP_ENABLED.store(effective, Ordering::SeqCst);
    effective
}

unsafe fn add_tray_icon(hwnd: HWND) -> Result<()> {
    let icon = create_swap_icon(false);

    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP,
        uCallbackMessage: WM_TRAYICON,
        hIcon: icon,
        ..Default::default()
    };

    set_tip(&mut nid, false);
    let result = Shell_NotifyIconW(NIM_ADD, &nid).ok();
    let _ = DestroyIcon(icon);
    result
}

unsafe fn update_tray_icon(hwnd: HWND, enabled: bool) -> Result<()> {
    let icon = create_swap_icon(enabled);

    let mut nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        uFlags: NIF_ICON | NIF_TIP,
        hIcon: icon,
        ..Default::default()
    };

    set_tip(&mut nid, enabled);
    let result = Shell_NotifyIconW(NIM_MODIFY, &nid).ok();
    let _ = DestroyIcon(icon);
    result
}

fn set_tip(nid: &mut NOTIFYICONDATAW, enabled: bool) {
    let tip = if enabled {
        "JumpSwap (On)"
    } else {
        "JumpSwap (Off)"
    };
    let tip_wide: Vec<u16> = tip.encode_utf16().chain(std::iter::once(0)).collect();
    let len = tip_wide.len().min(nid.szTip.len());
    nid.szTip[..len].copy_from_slice(&tip_wide[..len]);
}

unsafe fn remove_tray_icon(hwnd: HWND) -> Result<()> {
    let nid = NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: 1,
        ..Default::default()
    };
    Shell_NotifyIconW(NIM_DELETE, &nid).ok()
}

unsafe fn create_swap_icon(enabled: bool) -> HICON {
    let size: i32 = 16;
    let hdc_screen = GetDC(None);
    let hdc = CreateCompatibleDC(Some(hdc_screen));
    let bmp = CreateCompatibleBitmap(hdc_screen, size, size);
    let old_bmp = SelectObject(hdc, bmp.into());

    let bg_brush = CreateSolidBrush(COLORREF(0x00000000));
    let rect = RECT {
        left: 0,
        top: 0,
        right: size,
        bottom: size,
    };
    FillRect(hdc, &rect, bg_brush);
    let _ = DeleteObject(bg_brush.into());

    let color = if enabled {
        COLORREF(0x0000CC00)
    } else {
        COLORREF(0x00808080)
    };
    let brush = CreateSolidBrush(color);
    let pen = CreatePen(PS_SOLID, 1, color);
    let old_brush = SelectObject(hdc, brush.into());
    let old_pen = SelectObject(hdc, pen.into());
    let _ = Ellipse(hdc, 1, 1, size - 1, size - 1);
    SelectObject(hdc, old_brush);
    SelectObject(hdc, old_pen);
    let _ = DeleteObject(brush.into());
    let _ = DeleteObject(pen.into());

    let hdc_mask = CreateCompatibleDC(Some(hdc_screen));
    let bmp_mask = CreateBitmap(size, size, 1, 1, None);
    let old_mask = SelectObject(hdc_mask, bmp_mask.into());

    let white_brush = CreateSolidBrush(COLORREF(0x00FFFFFF));
    let mask_rect = RECT {
        left: 0,
        top: 0,
        right: size,
        bottom: size,
    };
    FillRect(hdc_mask, &mask_rect, white_brush);
    let _ = DeleteObject(white_brush.into());

    let black_brush = CreateSolidBrush(COLORREF(0x00000000));
    let black_pen = CreatePen(PS_SOLID, 1, COLORREF(0x00000000));
    let ob = SelectObject(hdc_mask, black_brush.into());
    let op = SelectObject(hdc_mask, black_pen.into());
    let _ = Ellipse(hdc_mask, 1, 1, size - 1, size - 1);
    SelectObject(hdc_mask, ob);
    SelectObject(hdc_mask, op);
    let _ = DeleteObject(black_brush.into());
    let _ = DeleteObject(black_pen.into());

    SelectObject(hdc, old_bmp);
    SelectObject(hdc_mask, old_mask);

    let icon_info = ICONINFO {
        fIcon: TRUE,
        xHotspot: 0,
        yHotspot: 0,
        hbmMask: bmp_mask,
        hbmColor: bmp,
    };
    let icon = CreateIconIndirect(&icon_info).unwrap_or_default();

    let _ = DeleteObject(bmp.into());
    let _ = DeleteObject(bmp_mask.into());
    let _ = DeleteDC(hdc);
    let _ = DeleteDC(hdc_mask);
    ReleaseDC(None, hdc_screen);

    icon
}

const STARTUP_REG_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
const STARTUP_VALUE_NAME: PCWSTR = w!("JumpSwap");

fn is_startup_enabled() -> bool {
    unsafe {
        let mut key = HKEY::default();
        let result = RegOpenKeyExW(HKEY_CURRENT_USER, STARTUP_REG_KEY, Some(0), KEY_READ, &mut key);
        if result.is_err() {
            return false;
        }
        let result = RegQueryValueExW(key, STARTUP_VALUE_NAME, None, None, None, None);
        let _ = RegCloseKey(key);
        result.is_ok()
    }
}

fn set_startup_enabled(enable: bool) {
    unsafe {
        let mut key = HKEY::default();
        let result = RegOpenKeyExW(HKEY_CURRENT_USER, STARTUP_REG_KEY, Some(0), KEY_WRITE, &mut key);
        if result.is_err() {
            return;
        }
        if enable {
            // Get the path to the current executable
            let mut buf = [0u16; 512];
            let len = GetModuleFileNameW(None, &mut buf);
            if len > 0 {
                let exe_path_bytes =
                    std::slice::from_raw_parts(buf.as_ptr() as *const u8, ((len + 1) as usize) * 2);
                let _ = RegSetValueExW(key, STARTUP_VALUE_NAME, Some(0), REG_SZ, Some(exe_path_bytes));
            }
        } else {
            let _ = RegDeleteValueW(key, STARTUP_VALUE_NAME);
        }
        let _ = RegCloseKey(key);
    }
}

unsafe fn show_context_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().expect("Failed to create menu");
    let effective = SWAP_ENABLED.load(Ordering::SeqCst);
    let auto = AUTO_DETECT.load(Ordering::SeqCst);

    // Swap item reflects the effective swap state
    let mut swap_item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID | MIIM_STATE | MIIM_STRING,
        wID: IDM_SWAP,
        fState: if effective { MFS_CHECKED } else { MFS_UNCHECKED },
        dwTypeData: PWSTR(w!("Swap").as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 0, true, &mut swap_item);

    let mut auto_item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID | MIIM_STATE | MIIM_STRING,
        wID: IDM_AUTO,
        fState: if auto { MFS_CHECKED } else { MFS_UNCHECKED },
        dwTypeData: PWSTR(w!("Auto-detect games").as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 1, true, &mut auto_item);

    let res_switch = FINALS_RES_SWITCH.load(Ordering::SeqCst);
    let mut res_item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID | MIIM_STATE | MIIM_STRING,
        wID: IDM_FINALS_RES,
        fState: if res_switch { MFS_CHECKED } else { MFS_UNCHECKED },
        dwTypeData: PWSTR(w!("1080p @ 120Hz for THE FINALS").as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 2, true, &mut res_item);

    let startup = is_startup_enabled();
    let mut startup_item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID | MIIM_STATE | MIIM_STRING,
        wID: IDM_STARTUP,
        fState: if startup { MFS_CHECKED } else { MFS_UNCHECKED },
        dwTypeData: PWSTR(w!("Run on startup").as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 3, true, &mut startup_item);

    // Separator
    let mut sep = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_FTYPE,
        fType: MFT_SEPARATOR,
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 4, true, &mut sep);

    let mut quit_item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID | MIIM_STRING,
        wID: IDM_QUIT,
        dwTypeData: PWSTR(w!("Quit").as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 5, true, &mut quit_item);

    let _ = SetForegroundWindow(hwnd);

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = TrackPopupMenu(
        menu,
        TPM_BOTTOMALIGN | TPM_LEFTALIGN,
        pt.x,
        pt.y,
        Some(0),
        hwnd,
        None,
    );
    PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0)).ok();

    let _ = DestroyMenu(menu);
}

unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match msg {
        WM_TRAYICON => {
            let event = (lparam.0 as u32) & 0xFFFF;
            if event == WM_RBUTTONUP || event == WM_LBUTTONUP {
                show_context_menu(hwnd);
            }
            LRESULT(0)
        }
        WM_GAME_STATE => {
            // Posted by detector thread when game state changes.
            // Clear suppression so auto-detect works fresh for new game sessions.
            AUTO_SUPPRESSED.store(false, Ordering::SeqCst);
            let effective = update_swap_state();
            let _ = update_tray_icon(hwnd, effective);
            LRESULT(0)
        }
        WM_FINALS_STATE => {
            // Posted by detector thread when THE FINALS starts/stops.
            if wparam.0 != 0 {
                on_finals_started();
            } else {
                on_finals_stopped();
            }
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 as u32) & 0xFFFF;
            match id {
                IDM_SWAP => {
                    let was_effective = SWAP_ENABLED.load(Ordering::SeqCst);
                    if was_effective {
                        // User wants swap OFF
                        MANUAL_SWAP.store(false, Ordering::SeqCst);
                        // Suppress auto-detect so it doesn't immediately re-enable
                        AUTO_SUPPRESSED.store(true, Ordering::SeqCst);
                    } else {
                        // User wants swap ON
                        MANUAL_SWAP.store(true, Ordering::SeqCst);
                        AUTO_SUPPRESSED.store(false, Ordering::SeqCst);
                    }
                    let effective = update_swap_state();
                    let _ = update_tray_icon(hwnd, effective);
                }
                IDM_AUTO => {
                    let new_auto = !AUTO_DETECT.load(Ordering::SeqCst);
                    AUTO_DETECT.store(new_auto, Ordering::SeqCst);
                    let effective = update_swap_state();
                    let _ = update_tray_icon(hwnd, effective);
                }
                IDM_STARTUP => {
                    let enabled = is_startup_enabled();
                    set_startup_enabled(!enabled);
                }
                IDM_FINALS_RES => {
                    let new_val = !FINALS_RES_SWITCH.load(Ordering::SeqCst);
                    FINALS_RES_SWITCH.store(new_val, Ordering::SeqCst);
                    // If turning off mid-game, restore the original mode now.
                    if !new_val {
                        on_finals_stopped();
                    }
                }
                IDM_QUIT => {
                    // Restore display before exit if we changed it.
                    on_finals_stopped();
                    let _ = remove_tray_icon(hwnd);
                    PostQuitMessage(0);
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe extern "system" fn keyboard_hook(
    code: i32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if code as u32 == HC_ACTION && SWAP_ENABLED.load(Ordering::Relaxed) {
        let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);

        if is_injected_event(kb.dwExtraInfo) {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        let swap_to = remap_virtual_key(kb.vkCode as u16).map(VIRTUAL_KEY);

        if let Some(target_vk) = swap_to {
            let flags = if wparam.0 as u32 == WM_KEYUP || wparam.0 as u32 == WM_SYSKEYUP {
                KEYBD_EVENT_FLAGS(KEYEVENTF_KEYUP.0)
            } else {
                KEYBD_EVENT_FLAGS(0)
            };

            let input = INPUT {
                r#type: INPUT_KEYBOARD,
                Anonymous: INPUT_0 {
                    ki: KEYBDINPUT {
                        wVk: target_vk,
                        wScan: 0,
                        dwFlags: flags,
                        time: 0,
                        dwExtraInfo: SENTINEL,
                    },
                },
            };

            SendInput(&[input], size_of::<INPUT>() as i32);
            return LRESULT(1);
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}
