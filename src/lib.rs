pub const SENTINEL: usize = 0x4A534B59;

pub const GAME_PROCESSES: &[&str] = &[
    "fortniteclient-win64-shipping.exe",
    "destiny2.exe",
    "marathon.exe",
];

pub const VK_RETURN_CODE: u16 = 0x0D;
pub const VK_SPACE_CODE: u16 = 0x20;

pub fn should_enable_swap(manual_swap: bool, auto_detect: bool, game_running: bool) -> bool {
    manual_swap || (auto_detect && game_running)
}

pub fn normalize_process_name(process_name: &str) -> String {
    process_name.trim().to_ascii_lowercase()
}

pub fn is_watched_game_process(process_name: &str) -> bool {
    let normalized = normalize_process_name(process_name);
    GAME_PROCESSES.iter().any(|game| normalized == *game)
}

pub fn any_watched_game_running<'a>(
    process_names: impl IntoIterator<Item = &'a str>,
) -> bool {
    process_names.into_iter().any(is_watched_game_process)
}

pub fn remap_virtual_key(vk_code: u16) -> Option<u16> {
    match vk_code {
        VK_RETURN_CODE => Some(VK_SPACE_CODE),
        VK_SPACE_CODE => Some(VK_RETURN_CODE),
        _ => None,
    }
}

pub fn is_injected_event(extra_info: usize) -> bool {
    extra_info == SENTINEL
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_state_follows_manual_or_auto_game_rule() {
        let cases = [
            (false, false, false, false),
            (false, false, true, false),
            (false, true, false, false),
            (false, true, true, true),
            (true, false, false, true),
            (true, false, true, true),
            (true, true, false, true),
            (true, true, true, true),
        ];

        for (manual, auto, game, expected) in cases {
            assert_eq!(should_enable_swap(manual, auto, game), expected);
        }
    }

    #[test]
    fn watched_game_matching_is_case_insensitive() {
        assert!(is_watched_game_process("DESTINY2.EXE"));
        assert!(is_watched_game_process("FortniteClient-Win64-Shipping.exe"));
        assert!(is_watched_game_process(" marathon.exe "));
    }

    #[test]
    fn watched_game_matching_rejects_unknown_processes() {
        assert!(!is_watched_game_process("notepad.exe"));
        assert!(!is_watched_game_process("marathonlauncher.exe"));
    }

    #[test]
    fn watched_game_scan_stops_when_any_match_is_present() {
        let processes = ["explorer.exe", "steam.exe", "destiny2.exe"];
        assert!(any_watched_game_running(processes.iter().copied()));
    }

    #[test]
    fn watched_game_scan_returns_false_without_matches() {
        let processes = ["explorer.exe", "steam.exe", "notepad.exe"];
        assert!(!any_watched_game_running(processes.iter().copied()));
    }

    #[test]
    fn remap_virtual_key_only_swaps_enter_and_space() {
        assert_eq!(remap_virtual_key(VK_RETURN_CODE), Some(VK_SPACE_CODE));
        assert_eq!(remap_virtual_key(VK_SPACE_CODE), Some(VK_RETURN_CODE));
        assert_eq!(remap_virtual_key(0x41), None);
    }

    #[test]
    fn sentinel_only_matches_our_injected_events() {
        assert!(is_injected_event(SENTINEL));
        assert!(!is_injected_event(0));
        assert!(!is_injected_event(SENTINEL + 1));
    }
}
