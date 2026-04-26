pub const SENTINEL: usize = 0x4A534B59;

pub const GAME_PROCESSES: &[&str] = &[
    "fortniteclient-win64-shipping.exe",
    "destiny2.exe",
    "marathon.exe",
    "lastflagclient-win64-shipping.exe",
];

pub const THE_FINALS_PROCESS: &str = "discovery.exe";

pub const VK_RETURN_CODE: u16 = 0x0D;
pub const VK_SPACE_CODE: u16 = 0x20;

pub fn should_enable_swap(
    manual_swap: bool,
    auto_detect: bool,
    game_running: bool,
    auto_suppressed: bool,
) -> bool {
    manual_swap || (auto_detect && game_running && !auto_suppressed)
}

/// Testable state machine for swap logic, mirroring the AtomicBool statics in main.
#[derive(Debug, Default)]
pub struct SwapState {
    pub manual_swap: bool,
    pub auto_detect: bool,
    pub game_running: bool,
    pub auto_suppressed: bool,
    pub swap_enabled: bool,
}

impl SwapState {
    pub fn new() -> Self {
        Self {
            auto_detect: true,
            ..Default::default()
        }
    }

    fn recalculate(&mut self) -> bool {
        self.swap_enabled =
            should_enable_swap(self.manual_swap, self.auto_detect, self.game_running, self.auto_suppressed);
        self.swap_enabled
    }

    /// User clicked the Swap toggle in the tray menu.
    pub fn toggle_swap(&mut self) -> bool {
        if self.swap_enabled {
            self.manual_swap = false;
            self.auto_suppressed = true;
        } else {
            self.manual_swap = true;
            self.auto_suppressed = false;
        }
        self.recalculate()
    }

    /// Game detector reported a state change.
    pub fn on_game_state_changed(&mut self, running: bool) -> bool {
        self.game_running = running;
        self.auto_suppressed = false;
        self.recalculate()
    }

    /// User toggled the Auto-detect checkbox.
    pub fn toggle_auto_detect(&mut self) -> bool {
        self.auto_detect = !self.auto_detect;
        self.recalculate()
    }
}

pub fn normalize_process_name(process_name: &str) -> String {
    process_name.trim().to_ascii_lowercase()
}

pub fn is_watched_game_process(process_name: &str) -> bool {
    let normalized = normalize_process_name(process_name);
    GAME_PROCESSES.iter().any(|game| normalized == *game)
}

pub fn is_the_finals_process(process_name: &str) -> bool {
    normalize_process_name(process_name) == THE_FINALS_PROCESS
}

pub fn any_watched_game_running<'a>(
    process_names: impl IntoIterator<Item = &'a str>,
) -> bool {
    process_names.into_iter().any(is_watched_game_process)
}

pub fn any_the_finals_running<'a>(
    process_names: impl IntoIterator<Item = &'a str>,
) -> bool {
    process_names.into_iter().any(is_the_finals_process)
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
        // (manual, auto, game, suppressed, expected)
        let cases = [
            // No suppression — original truth table
            (false, false, false, false, false),
            (false, false, true, false, false),
            (false, true, false, false, false),
            (false, true, true, false, true),
            (true, false, false, false, true),
            (true, false, true, false, true),
            (true, true, false, false, true),
            (true, true, true, false, true),
            // Suppressed — auto-detect is blocked, manual still works
            (false, true, true, true, false),
            (true, true, true, true, true),
        ];

        for (manual, auto, game, suppressed, expected) in cases {
            assert_eq!(
                should_enable_swap(manual, auto, game, suppressed),
                expected,
                "manual={manual}, auto={auto}, game={game}, suppressed={suppressed}"
            );
        }
    }

    // -- SwapState scenario tests --

    #[test]
    fn auto_detect_activates_swap_when_game_launches() {
        let mut s = SwapState::new();
        assert!(!s.swap_enabled);
        s.on_game_state_changed(true);
        assert!(s.swap_enabled);
    }

    #[test]
    fn auto_detect_deactivates_swap_when_game_exits() {
        let mut s = SwapState::new();
        s.on_game_state_changed(true);
        assert!(s.swap_enabled);
        s.on_game_state_changed(false);
        assert!(!s.swap_enabled);
    }

    #[test]
    fn user_can_disable_swap_during_auto_detected_game() {
        let mut s = SwapState::new();
        s.on_game_state_changed(true);
        assert!(s.swap_enabled);

        // User clicks Swap to turn it off
        s.toggle_swap();
        assert!(!s.swap_enabled);
    }

    #[test]
    fn suppression_clears_on_next_game_launch() {
        let mut s = SwapState::new();
        s.on_game_state_changed(true);
        s.toggle_swap(); // suppress
        assert!(!s.swap_enabled);

        // Game exits and a new game launches
        s.on_game_state_changed(false);
        s.on_game_state_changed(true);
        assert!(s.swap_enabled);
    }

    #[test]
    fn manual_swap_works_independently_of_auto_detect() {
        let mut s = SwapState::new();
        s.toggle_swap(); // manual on
        assert!(s.swap_enabled);
        assert!(s.manual_swap);

        s.toggle_swap(); // manual off (sets suppression, but no game running so irrelevant)
        assert!(!s.swap_enabled);
    }

    #[test]
    fn manual_on_then_game_launches_then_user_disables() {
        let mut s = SwapState::new();
        s.toggle_swap(); // manual on
        assert!(s.swap_enabled);

        s.on_game_state_changed(true); // game also running
        assert!(s.swap_enabled);

        s.toggle_swap(); // user wants off — suppresses auto AND clears manual
        assert!(!s.swap_enabled);
        assert!(!s.manual_swap);
        assert!(s.auto_suppressed);
    }

    #[test]
    fn toggle_auto_detect_off_disables_auto_swap() {
        let mut s = SwapState::new();
        s.on_game_state_changed(true);
        assert!(s.swap_enabled);

        s.toggle_auto_detect(); // auto off
        assert!(!s.swap_enabled);
    }

    #[test]
    fn user_re_enables_swap_after_suppressing() {
        let mut s = SwapState::new();
        s.on_game_state_changed(true);
        s.toggle_swap(); // suppress → off
        assert!(!s.swap_enabled);

        s.toggle_swap(); // manual on, clears suppression
        assert!(s.swap_enabled);
        assert!(s.manual_swap);
        assert!(!s.auto_suppressed);
    }

    // -- Process / key remap tests --

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
    fn the_finals_is_recognized_case_insensitively() {
        assert!(is_the_finals_process("Discovery.exe"));
        assert!(is_the_finals_process("DISCOVERY.EXE"));
        assert!(!is_the_finals_process("discoverylauncher.exe"));
    }

    #[test]
    fn the_finals_is_not_on_the_swap_list() {
        assert!(!is_watched_game_process("discovery.exe"));
    }

    #[test]
    fn the_finals_scan_finds_match_among_other_processes() {
        let processes = ["explorer.exe", "steam.exe", "Discovery.exe"];
        assert!(any_the_finals_running(processes.iter().copied()));
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
