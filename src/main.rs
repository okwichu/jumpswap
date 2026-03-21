#![windows_subsystem = "windows"]

use jumpswap::{
    SENTINEL, any_watched_game_running, is_injected_event, remap_virtual_key, should_enable_swap,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

static SWAP_ENABLED: AtomicBool = AtomicBool::new(false);
static AUTO_DETECT: AtomicBool = AtomicBool::new(true);
static GAME_RUNNING: AtomicBool = AtomicBool::new(false);
static MANUAL_SWAP: AtomicBool = AtomicBool::new(false);

const WM_TRAYICON: u32 = WM_USER + 1;
const WM_GAME_STATE: u32 = WM_USER + 2; // posted by detector thread
const IDM_SWAP: u32 = 1001;
const IDM_AUTO: u32 = 1002;
const IDM_QUIT: u32 = 1003;

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
    loop {
        if !AUTO_DETECT.load(Ordering::SeqCst) {
            // When auto-detect is off, just sleep and check again
            if was_running {
                was_running = false;
                GAME_RUNNING.store(false, Ordering::SeqCst);
                unsafe {
                    let hwnd = HWND(hwnd_raw as *mut _);
                    let _ = PostMessageW(Some(hwnd), WM_GAME_STATE, WPARAM(0), LPARAM(0));
                }
            }
            thread::sleep(Duration::from_secs(3));
            continue;
        }

        let running = is_any_game_running();
        if running != was_running {
            was_running = running;
            GAME_RUNNING.store(running, Ordering::SeqCst);
            unsafe {
                let hwnd = HWND(hwnd_raw as *mut _);
                let _ = PostMessageW(
                    Some(hwnd),
                    WM_GAME_STATE,
                    WPARAM(running as usize),
                    LPARAM(0),
                );
            }
        }

        thread::sleep(Duration::from_secs(3));
    }
}

/// Check if any watched game process is currently running.
fn is_any_game_running() -> bool {
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
        let snapshot = match snapshot {
            Ok(h) => h,
            Err(_) => return false,
        };

        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        if Process32FirstW(snapshot, &mut entry).is_ok() {
            loop {
                let exe_name = String::from_utf16_lossy(
                    &entry.szExeFile[..entry
                        .szExeFile
                        .iter()
                        .position(|&c| c == 0)
                        .unwrap_or(entry.szExeFile.len())],
                );

                if any_watched_game_running(std::iter::once(exe_name.as_str())) {
                    let _ = CloseHandle(snapshot);
                    return true;
                }

                if Process32NextW(snapshot, &mut entry).is_err() {
                    break;
                }
            }
        }

        let _ = CloseHandle(snapshot);
        false
    }
}

/// Recalculate effective swap state from manual toggle + auto-detect.
fn update_swap_state() -> bool {
    let auto = AUTO_DETECT.load(Ordering::SeqCst);
    let game = GAME_RUNNING.load(Ordering::SeqCst);
    let manual = MANUAL_SWAP.load(Ordering::SeqCst);

    let effective = should_enable_swap(manual, auto, game);
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

unsafe fn show_context_menu(hwnd: HWND) {
    let menu = CreatePopupMenu().expect("Failed to create menu");
    let manual = MANUAL_SWAP.load(Ordering::SeqCst);
    let auto = AUTO_DETECT.load(Ordering::SeqCst);

    // Swap item shows manual state (checkmark = manually forced on)
    let mut swap_item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID | MIIM_STATE | MIIM_STRING,
        wID: IDM_SWAP,
        fState: if manual { MFS_CHECKED } else { MFS_UNCHECKED },
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

    // Separator
    let mut sep = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_FTYPE,
        fType: MFT_SEPARATOR,
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 2, true, &mut sep);

    let mut quit_item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID | MIIM_STRING,
        wID: IDM_QUIT,
        dwTypeData: PWSTR(w!("Quit").as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 3, true, &mut quit_item);

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
            // Posted by detector thread when game state changes
            let effective = update_swap_state();
            let _ = update_tray_icon(hwnd, effective);
            LRESULT(0)
        }
        WM_COMMAND => {
            let id = (wparam.0 as u32) & 0xFFFF;
            match id {
                IDM_SWAP => {
                    let new_manual = !MANUAL_SWAP.load(Ordering::SeqCst);
                    MANUAL_SWAP.store(new_manual, Ordering::SeqCst);
                    let effective = update_swap_state();
                    let _ = update_tray_icon(hwnd, effective);
                }
                IDM_AUTO => {
                    let new_auto = !AUTO_DETECT.load(Ordering::SeqCst);
                    AUTO_DETECT.store(new_auto, Ordering::SeqCst);
                    let effective = update_swap_state();
                    let _ = update_tray_icon(hwnd, effective);
                }
                IDM_QUIT => {
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
