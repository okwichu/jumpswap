#![windows_subsystem = "windows"]

use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::LibraryLoader::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::*;

static SWAP_ENABLED: AtomicBool = AtomicBool::new(false);

const SENTINEL: usize = 0x4A534B59; // "JSKY" — marks our injected keys
const WM_TRAYICON: u32 = WM_USER + 1;
const IDM_SWAP: u32 = 1001;
const IDM_QUIT: u32 = 1002;

fn main() -> Result<()> {
    unsafe {
        let instance = GetModuleHandleW(None)?;

        // Register window class
        let class_name = w!("JumpSwapClass");
        let wc = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wnd_proc),
            hInstance: instance.into(),
            lpszClassName: class_name,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        // Create message-only window
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

        // Create system tray icon
        add_tray_icon(hwnd)?;

        // Install low-level keyboard hook
        let hook = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), None, 0)?;

        // Message loop
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        // Cleanup
        let _ = UnhookWindowsHookEx(hook);
        remove_tray_icon(hwnd)?;
    }

    Ok(())
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

    let tip = "JumpSwap (Off)";
    let tip_wide: Vec<u16> = tip.encode_utf16().chain(std::iter::once(0)).collect();
    let len = tip_wide.len().min(nid.szTip.len());
    nid.szTip[..len].copy_from_slice(&tip_wide[..len]);

    Shell_NotifyIconW(NIM_ADD, &nid).ok()
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

    let tip = if enabled {
        "JumpSwap (On)"
    } else {
        "JumpSwap (Off)"
    };
    let tip_wide: Vec<u16> = tip.encode_utf16().chain(std::iter::once(0)).collect();
    let len = tip_wide.len().min(nid.szTip.len());
    nid.szTip[..len].copy_from_slice(&tip_wide[..len]);

    Shell_NotifyIconW(NIM_MODIFY, &nid).ok()
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

/// Create a simple 16x16 icon programmatically.
/// Grey circle when off, green circle when on.
unsafe fn create_swap_icon(enabled: bool) -> HICON {
    let size: i32 = 16;
    let hdc_screen = GetDC(None);
    let hdc = CreateCompatibleDC(Some(hdc_screen));
    let bmp = CreateCompatibleBitmap(hdc_screen, size, size);
    let old_bmp = SelectObject(hdc, bmp.into());

    // Black background
    let bg_brush = CreateSolidBrush(COLORREF(0x00000000));
    let rect = RECT {
        left: 0,
        top: 0,
        right: size,
        bottom: size,
    };
    FillRect(hdc, &rect, bg_brush);
    let _ = DeleteObject(bg_brush.into());

    // Draw filled circle: green when on, grey when off
    let color = if enabled {
        COLORREF(0x0000CC00) // Green (BGR)
    } else {
        COLORREF(0x00808080) // Grey
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

    // Create mask bitmap
    let hdc_mask = CreateCompatibleDC(Some(hdc_screen));
    let bmp_mask = CreateBitmap(size, size, 1, 1, None);
    let old_mask = SelectObject(hdc_mask, bmp_mask.into());

    // White = transparent, black circle = opaque
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
    let enabled = SWAP_ENABLED.load(Ordering::SeqCst);

    let mut swap_item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID | MIIM_STATE | MIIM_STRING,
        wID: IDM_SWAP,
        fState: if enabled { MFS_CHECKED } else { MFS_UNCHECKED },
        dwTypeData: PWSTR(w!("Swap").as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 0, true, &mut swap_item);

    let mut quit_item = MENUITEMINFOW {
        cbSize: size_of::<MENUITEMINFOW>() as u32,
        fMask: MIIM_ID | MIIM_STRING,
        wID: IDM_QUIT,
        dwTypeData: PWSTR(w!("Quit").as_ptr() as *mut _),
        ..Default::default()
    };
    let _ = InsertMenuItemW(menu, 1, true, &mut quit_item);

    // Required for tray menu to dismiss properly
    let _ = SetForegroundWindow(hwnd);

    let mut pt = POINT::default();
    let _ = GetCursorPos(&mut pt);
    let _ = TrackPopupMenu(menu, TPM_BOTTOMALIGN | TPM_LEFTALIGN, pt.x, pt.y, Some(0), hwnd, None);
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
        WM_COMMAND => {
            let id = (wparam.0 as u32) & 0xFFFF;
            match id {
                IDM_SWAP => {
                    let new_state = !SWAP_ENABLED.load(Ordering::SeqCst);
                    SWAP_ENABLED.store(new_state, Ordering::SeqCst);
                    let _ = update_tray_icon(hwnd, new_state);
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

        // Don't process keys we injected ourselves
        if kb.dwExtraInfo == SENTINEL {
            return CallNextHookEx(None, code, wparam, lparam);
        }

        let vk = VIRTUAL_KEY(kb.vkCode as u16);
        let swap_to = match vk {
            VK_RETURN => Some(VK_SPACE),
            VK_SPACE => Some(VK_RETURN),
            _ => None,
        };

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
            return LRESULT(1); // Suppress original key
        }
    }

    CallNextHookEx(None, code, wparam, lparam)
}
