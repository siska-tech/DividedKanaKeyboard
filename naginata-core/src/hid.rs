//! ASCII（ローマ字出力）→ USB HID キーボード Usage ID 変換、および直接キー型。
//!
//! 要件 FR-4.1: ローマ字シーケンスをスキャンコード列として送出する。
//! ここでは "1文字 → KeyPress(usage + modifiers)" を返す。レポート組み立てはファーム側。

/// USB HID modifier ビット（レポート先頭バイト）。
pub mod modkeys {
    pub const CTRL: u8 = 0x01;
    pub const SHIFT: u8 = 0x02;
    pub const ALT: u8 = 0x04;
    pub const GUI: u8 = 0x08;
}

/// USB HID Keyboard Usage ID ＋ modifier バイト。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyPress {
    pub usage: u8,
    pub modifiers: u8,
}

impl KeyPress {
    pub const fn new(usage: u8, modifiers: u8) -> Self {
        Self { usage, modifiers }
    }
    pub const fn plain(usage: u8) -> Self {
        Self { usage, modifiers: 0 }
    }
}

/// ASCII 1 文字を HID キーへ。対応外は None。
/// ローマ字出力で使う範囲（a-z, 0-9, '-', 空白）をカバー。
pub fn ascii_to_hid(c: char) -> Option<KeyPress> {
    let (usage, shift) = match c {
        'a'..='z' => (0x04 + (c as u8 - b'a'), false),
        'A'..='Z' => (0x04 + (c as u8 - b'A'), true),
        '1'..='9' => (0x1e + (c as u8 - b'1'), false),
        '0' => (0x27, false),
        '-' => (0x2d, false),
        ' ' => (0x2c, false),
        _ => return None,
    };
    Some(KeyPress::new(usage, if shift { modkeys::SHIFT } else { 0 }))
}

/// ローマ字文字列を HID キー列へ変換し、コールバックへ流す。
pub fn romaji_to_hid(romaji: &str, out: &mut dyn FnMut(KeyPress)) {
    for c in romaji.chars() {
        if let Some(k) = ascii_to_hid(c) {
            out(k);
        }
    }
}

/// usage が HID modifier キー（LCtrl..RGUI, 0xE0..=0xE7）なら、そのレポート先頭
/// バイトのビット（1<<n）を返す。パススルー/ホールドでの修飾キー合成に使う。
pub fn modifier_bit(usage: u8) -> Option<u8> {
    if (0xe0..=0xe7).contains(&usage) {
        Some(1 << (usage - 0xe0))
    } else {
        None
    }
}

/// Set-1 スキャンコード → HID Keyboard Usage ID（パススルーモード, #05-future-work §3 Tier A）。
///
/// 物理キーは Set-1 sc（= QWERTY 配列そのもの）で管理されているため、この静的変換だけで
/// マッピングテーブル 0 件の素の QWERTY になる。メインブロック＋ファンクション行をカバー。
/// 対応外（テンキー等）は None（パススルー中は無視）。
pub fn sc_to_usage(sc: u8) -> Option<u8> {
    let u = match sc {
        0x01 => 0x29,                       // Esc
        0x02..=0x0a => 0x1e + (sc - 0x02),  // 1..9
        0x0b => 0x27,                       // 0
        0x0c => 0x2d,                       // -
        0x0d => 0x2e,                       // =
        0x0e => 0x2a,                       // Backspace
        0x0f => 0x2b,                       // Tab
        // qwertyuiop
        0x10 => 0x14, 0x11 => 0x1a, 0x12 => 0x08, 0x13 => 0x15, 0x14 => 0x17,
        0x15 => 0x1c, 0x16 => 0x18, 0x17 => 0x0c, 0x18 => 0x12, 0x19 => 0x13,
        0x1a => 0x2f,                       // [
        0x1b => 0x30,                       // ]
        0x1c => 0x28,                       // Enter
        0x1d => 0xe0,                       // LCtrl
        // asdfghjkl
        0x1e => 0x04, 0x1f => 0x16, 0x20 => 0x07, 0x21 => 0x09, 0x22 => 0x0a,
        0x23 => 0x0b, 0x24 => 0x0d, 0x25 => 0x0e, 0x26 => 0x0f,
        0x27 => 0x33,                       // ;
        0x28 => 0x34,                       // '
        0x29 => 0x35,                       // `
        0x2a => 0xe1,                       // LShift
        0x2b => 0x31,                       // バックスラッシュ
        // zxcvbnm
        0x2c => 0x1d, 0x2d => 0x1b, 0x2e => 0x06, 0x2f => 0x19, 0x30 => 0x05,
        0x31 => 0x11, 0x32 => 0x10,
        0x33 => 0x36,                       // ,
        0x34 => 0x37,                       // .
        0x35 => 0x38,                       // /
        0x36 => 0xe5,                       // RShift
        0x38 => 0xe2,                       // LAlt
        0x39 => 0x2c,                       // Space
        0x3a => 0x39,                       // CapsLock
        0x3b..=0x44 => 0x3a + (sc - 0x3b),  // F1..F10
        0x57 => 0x44,                       // F11
        0x58 => 0x45,                       // F12
        _ => return None,
    };
    Some(u)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters() {
        assert_eq!(ascii_to_hid('a'), Some(KeyPress::new(0x04, 0)));
        assert_eq!(ascii_to_hid('z'), Some(KeyPress::new(0x1d, 0)));
        assert_eq!(ascii_to_hid('-'), Some(KeyPress::new(0x2d, 0)));
    }

    #[test]
    fn sequence() {
        let mut v = Vec::new();
        romaji_to_hid("kya", &mut |k| v.push(k.usage));
        assert_eq!(v, vec![0x0e, 0x1c, 0x04]); // k, y, a
    }

    #[test]
    fn passthrough_sc_to_usage() {
        assert_eq!(sc_to_usage(0x10), Some(0x14)); // q
        assert_eq!(sc_to_usage(0x32), Some(0x10)); // m
        assert_eq!(sc_to_usage(0x39), Some(0x2c)); // space
        assert_eq!(sc_to_usage(0x02), Some(0x1e)); // 1
        assert_eq!(sc_to_usage(0x0b), Some(0x27)); // 0
        assert_eq!(sc_to_usage(0x3b), Some(0x3a)); // F1
        assert_eq!(sc_to_usage(0x58), Some(0x45)); // F12
        assert_eq!(sc_to_usage(0x2a), Some(0xe1)); // LShift
        assert_eq!(sc_to_usage(0x37), None);       // KP* は対応外
    }

    #[test]
    fn modifier_bits() {
        assert_eq!(modifier_bit(0xe0), Some(0x01)); // LCtrl
        assert_eq!(modifier_bit(0xe1), Some(0x02)); // LShift
        assert_eq!(modifier_bit(0xe5), Some(0x20)); // RShift
        assert_eq!(modifier_bit(0x04), None);
    }
}
