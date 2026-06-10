//! PC↔端末 設定チャネル(#12) — vendor-defined HID（VIA/Vial 類似, 追加ドライバ不要）。
//!
//! 既存の HID キーボード(IF0)に加え、64B in/out の **vendor HID(IF1)** を列挙する。
//! WebHID から usagePage=0xFF00 でフィルタして直接 read/write できる。
//!
//! プロトコル v2（フェーズ2: 全カテゴリ。共に64B固定）:
//!   OUT(host→dev): [0]=cmd [1]=seq [2..]=args
//!   IN (dev→host): [0]=cmd [1]=seq [2]=status [3..]=payload
//!
//! 既定値は静的テーブル（phf）を直接走査して返す＝デバイスが真値。差分はステージング Config。
//! 反映方針: WRITE 系はステージング更新のみ。COMMIT でフラッシュ保存し **再起動後** に反映。

use core::sync::atomic::Ordering;

use naginata_core::config::{Config, OverrideVal, Params, MAX_KANA_LEN, MAX_KEYS, TAG_KANA, TAG_KEYS};
use naginata_core::{keymap, Action};

use crate::config_store;
use crate::handedness::HandFlash;

/// レポート長（IN/OUT 共通）。フェーズ2で 64B（キー操作の長い列に対応）。
pub const REPORT_LEN: usize = 64;

/// vendor-defined HID レポートディスクリプタ（usagePage 0xFF00 / usage 0x01, 64B in+out）。
pub const VENDOR_REPORT_DESCRIPTOR: &[u8] = &[
    0x06, 0x00, 0xFF, // Usage Page (Vendor Defined 0xFF00)
    0x09, 0x01, //       Usage (0x01)
    0xA1, 0x01, //       Collection (Application)
    0x09, 0x02, //         Usage (0x02)  ; data in
    0x15, 0x00, //         Logical Minimum (0)
    0x26, 0xFF, 0x00, //   Logical Maximum (255)
    0x95, 0x40, //         Report Count (64)
    0x75, 0x08, //         Report Size (8)
    0x81, 0x02, //         Input (Data,Var,Abs)
    0x09, 0x03, //         Usage (0x03)  ; data out
    0x91, 0x02, //         Output (Data,Var,Abs)
    0xC0, //             End Collection
];

// --- コマンド ---------------------------------------------------------------
pub const CMD_INFO: u8 = 0x01;
pub const CMD_READ_DEFAULTS: u8 = 0x02;
pub const CMD_READ_OVERRIDES: u8 = 0x03;
pub const CMD_WRITE: u8 = 0x04;
pub const CMD_COMMIT: u8 = 0x05;
pub const CMD_RESET: u8 = 0x06;
pub const CMD_REBOOT: u8 = 0x07;
pub const CMD_READ_PARAMS: u8 = 0x08;
pub const CMD_WRITE_PARAMS: u8 = 0x09;
pub const CMD_READ_MATRIX: u8 = 0x0a;
pub const CMD_READ_LASTKEY: u8 = 0x0b;

// --- セクション（config.rs の SEC_* と一致させる）--------------------------
pub const SEC_LAYERS: u8 = 1;
pub const SEC_COMBO2: u8 = 2;
pub const SEC_COMBO3: u8 = 3;
pub const SEC_MODES: u8 = 4;
pub const SEC_EXTRA: u8 = 5;

// --- ステータス -------------------------------------------------------------
const ST_OK: u8 = 0x00;
const ST_ERR: u8 = 0x01;
const ST_UNKNOWN: u8 = 0x02;

const PROTO_VER: u8 = 2;
const FW_MAJOR: u8 = 0;
const FW_MINOR: u8 = 2;

/// コマンド処理の結果。`reboot` が true なら呼び出し側で sys_reset する。
pub struct Outcome {
    pub resp: [u8; REPORT_LEN],
    pub reboot: bool,
}

fn put_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off] = (v & 0xff) as u8;
    buf[off + 1] = (v >> 8) as u8;
}
fn get_u16(buf: &[u8], off: usize) -> u16 {
    (buf[off] as u16) | ((buf[off + 1] as u16) << 8)
}

/// Action を ワイヤ値 [tag][len][payload] として書く。書いた末尾 off を返す。
fn write_action(buf: &mut [u8], off: usize, a: Action) -> usize {
    match a {
        Action::Kana(s) => {
            let b = s.as_bytes();
            let l = b.len().min(MAX_KANA_LEN);
            buf[off] = TAG_KANA;
            buf[off + 1] = l as u8;
            buf[off + 2..off + 2 + l].copy_from_slice(&b[..l]);
            off + 2 + l
        }
        Action::Keys(k) => {
            let n = k.len().min(MAX_KEYS);
            buf[off] = TAG_KEYS;
            buf[off + 1] = (n * 2) as u8;
            let mut p = off + 2;
            for kp in &k[..n] {
                buf[p] = kp.usage;
                buf[p + 1] = kp.modifiers;
                p += 2;
            }
            p
        }
        Action::None => {
            buf[off] = TAG_KANA;
            buf[off + 1] = 0;
            off + 2
        }
    }
}
fn action_len(a: Action) -> usize {
    match a {
        Action::Kana(s) => 2 + s.len().min(MAX_KANA_LEN),
        Action::Keys(k) => 2 + k.len().min(MAX_KEYS) * 2,
        Action::None => 2,
    }
}

/// 1 コマンドを処理し、応答レポートを組み立てる。
pub fn handle_command(cmd_buf: &[u8; REPORT_LEN], staged: &mut Config, flash: &mut HandFlash) -> Outcome {
    let cmd = cmd_buf[0];
    let seq = cmd_buf[1];
    let mut resp = [0u8; REPORT_LEN];
    resp[0] = cmd;
    resp[1] = seq;
    resp[2] = ST_OK;
    let mut reboot = false;

    match cmd {
        CMD_INFO => {
            resp[3] = PROTO_VER;
            resp[4] = FW_MAJOR;
            resp[5] = FW_MINOR;
            resp[6] = MAX_KEYS as u8;
            resp[7] = MAX_KANA_LEN as u8;
            put_u16(&mut resp, 8, keymap::LAYERS.len() as u16);
            put_u16(&mut resp, 10, keymap::COMBO2.len() as u16);
            put_u16(&mut resp, 12, keymap::COMBO3.len() as u16);
            put_u16(&mut resp, 14, keymap::MODE_LAYERS.len() as u16);
        }
        CMD_READ_DEFAULTS => {
            let section = cmd_buf[2];
            let start = get_u16(cmd_buf, 3);
            read_defaults(section, start, &mut resp);
        }
        CMD_READ_OVERRIDES => {
            let section = cmd_buf[2];
            let start = get_u16(cmd_buf, 3);
            read_overrides(section, start, staged, &mut resp);
        }
        CMD_WRITE => {
            if write_entry(cmd_buf, staged).is_err() {
                resp[2] = ST_ERR;
            }
        }
        CMD_COMMIT => {
            if config_store::write_config(flash, staged).is_err() {
                resp[2] = ST_ERR;
            }
        }
        CMD_RESET => {
            if config_store::erase_config(flash).is_err() {
                resp[2] = ST_ERR;
            } else {
                *staged = Config::default();
            }
        }
        CMD_READ_PARAMS => {
            let p = &staged.params;
            put_u16(&mut resp, 3, p.window_ms);
            put_u16(&mut resp, 5, p.tap_ms);
            put_u16(&mut resp, 7, p.repeat_delay_ms);
            put_u16(&mut resp, 9, p.repeat_interval_ms);
            resp[11] = p.led_brightness;
        }
        CMD_WRITE_PARAMS => {
            staged.params = Params {
                window_ms: get_u16(cmd_buf, 2),
                tap_ms: get_u16(cmd_buf, 4),
                repeat_delay_ms: get_u16(cmd_buf, 6),
                repeat_interval_ms: get_u16(cmd_buf, 8),
                led_brightness: cmd_buf[10],
            };
        }
        CMD_READ_MATRIX => {
            let start = get_u16(cmd_buf, 2);
            read_matrix(start, &mut resp);
        }
        CMD_READ_LASTKEY => {
            // ARMv6-M は RMW(swap) 非対応 → load + 条件付き store でクリア（fresh は1回だけ true）。
            let v = crate::LAST_KEY.load(Ordering::Relaxed);
            let fresh = v & 0x8000 != 0;
            if fresh {
                crate::LAST_KEY.store(0, Ordering::Relaxed);
            }
            let pos = v & 0x7fff;
            resp[3] = (pos >> 12) as u8; // hand
            resp[4] = ((pos >> 4) & 0x0f) as u8; // row
            resp[5] = (pos & 0x0f) as u8; // col
            resp[6] = if fresh { 1 } else { 0 };
        }
        CMD_REBOOT => reboot = true,
        _ => resp[2] = ST_UNKNOWN,
    }

    Outcome { resp, reboot }
}

/// 既定（静的 phf テーブル）の section を start から詰める。
/// resp: [3]=section [4..6]=next_index [6]=count [7..]=entries。
/// entry = key bytes + [tag][len][payload]。size 適応ページング（収まるだけ詰めて next を返す）。
fn read_defaults(section: u8, start: u16, resp: &mut [u8; REPORT_LEN]) {
    resp[3] = section;
    let mut count: u16 = 0;
    let mut off = 7usize;
    match section {
        SEC_LAYERS => {
            for (i, (k, a)) in keymap::LAYERS.entries().enumerate() {
                if (i as u16) < start {
                    continue;
                }
                let need = 2 + action_len(*a);
                if off + need > REPORT_LEN {
                    break;
                }
                resp[off] = (*k >> 8) as u8; // mask
                resp[off + 1] = (*k & 0xff) as u8; // sc
                off = write_action(resp, off + 2, *a);
                count += 1;
            }
        }
        SEC_COMBO2 => {
            for (i, (k, a)) in keymap::COMBO2.entries().enumerate() {
                if (i as u16) < start {
                    continue;
                }
                let need = 2 + action_len(*a);
                if off + need > REPORT_LEN {
                    break;
                }
                resp[off] = (*k & 0xff) as u8; // a
                resp[off + 1] = (*k >> 8) as u8; // b
                off = write_action(resp, off + 2, *a);
                count += 1;
            }
        }
        SEC_COMBO3 => {
            for (i, (k, a)) in keymap::COMBO3.entries().enumerate() {
                if (i as u16) < start {
                    continue;
                }
                let need = 3 + action_len(*a);
                if off + need > REPORT_LEN {
                    break;
                }
                resp[off] = (*k & 0xff) as u8;
                resp[off + 1] = ((*k >> 8) & 0xff) as u8;
                resp[off + 2] = ((*k >> 16) & 0xff) as u8;
                off = write_action(resp, off + 3, *a);
                count += 1;
            }
        }
        SEC_MODES => {
            for (i, (k, a)) in keymap::MODE_LAYERS.entries().enumerate() {
                if (i as u16) < start {
                    continue;
                }
                let need = 2 + action_len(*a);
                if off + need > REPORT_LEN {
                    break;
                }
                resp[off] = (*k >> 8) as u8; // mode
                resp[off + 1] = (*k & 0xff) as u8; // sc
                off = write_action(resp, off + 2, *a);
                count += 1;
            }
        }
        _ => {}
    }
    put_u16(resp, 4, start + count);
    resp[6] = count as u8;
}

/// マトリクス (hand,row,col)→sc を start から詰める（learn/レイアウト表示用, #12 フェーズ3）。
/// 左(hand=0)→右(hand=1) の順。entry=[hand][row][col][sc]。resp[4..6]=next, resp[6]=count。
fn read_matrix(start: u16, resp: &mut [u8; REPORT_LEN]) {
    let mut count = 0u16;
    let mut off = 7usize;
    let mut idx = 0u16;
    'outer: for (hand, map) in [(0u8, &keymap::MATRIX_LEFT), (1u8, &keymap::MATRIX_RIGHT)] {
        for (k, sc) in map.entries() {
            if idx < start {
                idx += 1;
                continue;
            }
            if off + 4 > REPORT_LEN {
                break 'outer;
            }
            resp[off] = hand;
            resp[off + 1] = (*k >> 4) & 0x0f; // row
            resp[off + 2] = *k & 0x0f; // col
            resp[off + 3] = *sc;
            off += 4;
            count += 1;
            idx += 1;
        }
    }
    put_u16(resp, 4, start + count);
    resp[6] = count as u8;
}

/// ステージング Config の差分 section を start から詰める（同形式）。
fn read_overrides(section: u8, start: u16, staged: &Config, resp: &mut [u8; REPORT_LEN]) {
    resp[3] = section;
    let mut count: u16 = 0;
    let mut off = 7usize;
    match section {
        SEC_LAYERS => {
            for (i, (mask, sc, v)) in staged.layers.iter().enumerate() {
                if (i as u16) < start {
                    continue;
                }
                if off + 2 + v.enc_len() > REPORT_LEN {
                    break;
                }
                resp[off] = *mask;
                resp[off + 1] = *sc;
                off = v.encode(resp, off + 2);
                count += 1;
            }
        }
        SEC_COMBO2 => {
            for (i, (k, v)) in staged.combo2.iter().enumerate() {
                if (i as u16) < start {
                    continue;
                }
                if off + 2 + v.enc_len() > REPORT_LEN {
                    break;
                }
                resp[off] = k[0];
                resp[off + 1] = k[1];
                off = v.encode(resp, off + 2);
                count += 1;
            }
        }
        SEC_COMBO3 => {
            for (i, (k, v)) in staged.combo3.iter().enumerate() {
                if (i as u16) < start {
                    continue;
                }
                if off + 3 + v.enc_len() > REPORT_LEN {
                    break;
                }
                resp[off] = k[0];
                resp[off + 1] = k[1];
                resp[off + 2] = k[2];
                off = v.encode(resp, off + 3);
                count += 1;
            }
        }
        SEC_MODES => {
            for (i, (mode, sc, v)) in staged.modes.iter().enumerate() {
                if (i as u16) < start {
                    continue;
                }
                if off + 2 + v.enc_len() > REPORT_LEN {
                    break;
                }
                resp[off] = *mode;
                resp[off + 1] = *sc;
                off = v.encode(resp, off + 2);
                count += 1;
            }
        }
        SEC_EXTRA => {
            for (i, (hand, row, col, v)) in staged.extra.iter().enumerate() {
                if (i as u16) < start {
                    continue;
                }
                if off + 3 + v.enc_len() > REPORT_LEN {
                    break;
                }
                resp[off] = *hand;
                resp[off + 1] = *row;
                resp[off + 2] = *col;
                off = v.encode(resp, off + 3);
                count += 1;
            }
        }
        _ => {}
    }
    put_u16(resp, 4, start + count);
    resp[6] = count as u8;
}

/// WRITE: ステージング差分を設定。value の len==0 は「削除」（既定へ戻す）。
/// args: [2]=section, key bytes, value=[tag][len][payload]。
fn write_entry(cmd_buf: &[u8; REPORT_LEN], staged: &mut Config) -> Result<(), ()> {
    let section = cmd_buf[2];
    // キーバイト長とキー解釈。
    let (klen, val_off) = match section {
        SEC_LAYERS | SEC_COMBO2 | SEC_MODES => (2usize, 5usize),
        SEC_COMBO3 | SEC_EXTRA => (3usize, 6usize),
        _ => return Err(()),
    };
    let _ = klen;
    // 削除判定: value の len バイト（val_off+1）が 0。
    let is_remove = cmd_buf[val_off + 1] == 0;
    match section {
        SEC_LAYERS => {
            let (mask, sc) = (cmd_buf[3], cmd_buf[4]);
            if is_remove {
                staged.remove_layer(mask, sc);
            } else {
                let (v, _) = OverrideVal::decode(cmd_buf, val_off).ok_or(())?;
                staged.set_layer(mask, sc, v)?;
            }
        }
        SEC_COMBO2 => {
            let (a, b) = (cmd_buf[3], cmd_buf[4]);
            if is_remove {
                staged.remove_combo2(a, b);
            } else {
                let (v, _) = OverrideVal::decode(cmd_buf, val_off).ok_or(())?;
                staged.set_combo2(a, b, v)?;
            }
        }
        SEC_COMBO3 => {
            let (a, b, c) = (cmd_buf[3], cmd_buf[4], cmd_buf[5]);
            if is_remove {
                staged.remove_combo3(a, b, c);
            } else {
                let (v, _) = OverrideVal::decode(cmd_buf, val_off).ok_or(())?;
                staged.set_combo3(a, b, c, v)?;
            }
        }
        SEC_MODES => {
            let (mode, sc) = (cmd_buf[3], cmd_buf[4]);
            if is_remove {
                staged.remove_mode(mode, sc);
            } else {
                let (v, _) = OverrideVal::decode(cmd_buf, val_off).ok_or(())?;
                staged.set_mode(mode, sc, v)?;
            }
        }
        SEC_EXTRA => {
            let (hand, row, col) = (cmd_buf[3], cmd_buf[4], cmd_buf[5]);
            if is_remove {
                staged.remove_extra(hand, row, col);
            } else {
                let (v, _) = OverrideVal::decode(cmd_buf, val_off).ok_or(())?;
                staged.set_extra(hand, row, col, v)?;
            }
        }
        _ => return Err(()),
    }
    Ok(())
}
