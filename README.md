# JumpSwap

Minimal Windows system tray app that swaps the Enter and Spacebar keys — designed for games that hardwire Enter to open chat when you need Enter as your jump key.

## Usage

1. Run `jumpswap.exe`
2. A tray icon appears (grey circle = swap off)
3. Right-click (or left-click) the tray icon → click **Swap** to toggle
4. When enabled, the icon turns green and Enter ↔ Spacebar are swapped globally
5. Click **Quit** to exit

## Building

```
cargo build --release
```

The binary will be at `target/release/jumpswap.exe`.
