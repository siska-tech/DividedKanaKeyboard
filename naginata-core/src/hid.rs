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
}
