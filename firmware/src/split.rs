//! 左右間シリアル連結（UART0: GP0=TX, GP1=RX）— FR-2。
//!
//! 2バイト自己同期フレーム。先頭バイトのみ bit7=1（SYNC）。
//!   b0: [7]=SYNC(1) [6]=type(0=Key,1=Status) [5]=pressed(Key時) [2:0]=row(Key時)
//!   b1: bit7=0。 Key: [3:0]=col / Status: [5:0]=ShiftMask
//! 受信は bit7=1 を待ってフレーム頭を捕捉、復号失敗は破棄して再同期（§C.3）。

/// スレーブ→マスタ へ送る1キーイベント。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub row: u8,
    pub col: u8,
    pub pressed: bool,
}

/// 復号結果。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Frame {
    /// キーイベント（スレーブ→マスタ）
    Key(KeyEvent),
    /// シフト状態（マスタ→スレーブ, LED表示用）
    Status(u8),
}

const SYNC: u8 = 0x80;
const TYPE_STATUS: u8 = 0x40;
const PRESSED: u8 = 0x20;

impl KeyEvent {
    pub fn encode(&self) -> [u8; 2] {
        let b0 = SYNC | if self.pressed { PRESSED } else { 0 } | (self.row & 0x07);
        [b0, self.col & 0x0f]
    }
}

/// シフト状態フレーム（ShiftMask は6bit）。
pub fn encode_status(mask: u8) -> [u8; 2] {
    [SYNC | TYPE_STATUS, mask & 0x3f]
}

/// 2バイトを復号。先頭バイトに SYNC が無ければ None（再同期へ）。
pub fn decode(b0: u8, b1: u8) -> Option<Frame> {
    if b0 & SYNC == 0 {
        return None;
    }
    if b0 & TYPE_STATUS != 0 {
        Some(Frame::Status(b1 & 0x3f))
    } else {
        Some(Frame::Key(KeyEvent {
            pressed: b0 & PRESSED != 0,
            row: b0 & 0x07,
            col: b1 & 0x0f,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_roundtrip() {
        let e = KeyEvent {
            row: 5,
            col: 13,
            pressed: true,
        };
        let [a, b] = e.encode();
        assert_eq!(decode(a, b), Some(Frame::Key(e)));
    }
    #[test]
    fn status_roundtrip() {
        let [a, b] = encode_status(0x2d);
        assert_eq!(decode(a, b), Some(Frame::Status(0x2d)));
    }
}
