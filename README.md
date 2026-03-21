# JumpSwap

Minimal Windows system tray app that swaps the Enter and Spacebar keys — designed for games that hardwire Enter to open chat when you need Enter as your jump key.

## Usage

1. Run `jumpswap.exe`
2. A tray icon appears (grey circle = swap off)
3. Right-click (or left-click) the tray icon for the menu:
   - **Swap** — manually toggle Enter ↔ Spacebar (off by default)
   - **Auto-detect games** — automatically enable swap when a supported game is running (on by default)
   - **Quit** — exit the app
4. When the swap is active (manually or via auto-detect), the icon turns green

## Supported Games

JumpSwap auto-detects these games by process name:

- Fortnite (`FortniteClient-Win64-Shipping.exe`)
- Destiny 2 (`destiny2.exe`)
- Marathon (`marathon.exe`)

The swap activates within ~3 seconds of game launch and deactivates when the game exits.

## Building

```
cargo build --release
```

The binary will be at `target/release/jumpswap.exe`.
