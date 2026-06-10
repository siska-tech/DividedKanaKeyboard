//! USB HID キーボード出力（FR-4 / FR-5）。
//!
//! Boot Keyboard レポート(8バイト: modifier, reserved, key[6]) を送る。
//! ローマ字1文字 = 「押下レポート→解放レポート」の1ストロークとして送出する。
//!
//! 注意: embassy-usb の API は版差がある。初回ビルド時に examples/usb_hid_keyboard.rs に
//! 合わせて HidWriter の構築（State, Config, descriptor）を確定すること。ここは骨子。

use naginata_core::hid::KeyPress;

/// USB HID Boot Keyboard レポートディスクリプタ（標準）。
pub const KEYBOARD_REPORT_DESCRIPTOR: &[u8] = &[
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x05, 0x07, //   Usage Page (Keyboard)
    0x19, 0xE0, //   Usage Minimum (LeftControl)
    0x29, 0xE7, //   Usage Maximum (Right GUI)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x08, //   Report Count (8)
    0x81, 0x02, //   Input (Data,Var,Abs)  ; modifier byte
    0x95, 0x01, //   Report Count (1)
    0x75, 0x08, //   Report Size (8)
    0x81, 0x01, //   Input (Const)         ; reserved byte
    0x95, 0x06, //   Report Count (6)
    0x75, 0x08, //   Report Size (8)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0xFF, //   Logical Maximum (255)
    0x05, 0x07, //   Usage Page (Keyboard)
    0x19, 0x00, //   Usage Minimum (0)
    0x29, 0xFF, //   Usage Maximum (255)
    0x81, 0x00, //   Input (Data,Array)    ; key array
    0xC0, // End Collection
];

const LEFT_SHIFT: u8 = 0xE1;

/// 1キー押下の 8バイトレポートを作る。
pub fn report_for(key: KeyPress) -> [u8; 8] {
    let mut r = [0u8; 8];
    r[0] = key.modifiers; // Ctrl/Shift/Alt/GUI（hid::modkeys）
    r[2] = key.usage;
    r
}

/// 全キー解放レポート。
pub fn release_report() -> [u8; 8] {
    [0u8; 8]
}

/// 参考: Shift を単独で送る必要がある場合の usage。
pub const _SHIFT_USAGE: u8 = LEFT_SHIFT;
