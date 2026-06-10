//! 設定 IR(#12)のフラッシュ永続化 — `handedness.rs` と同型。
//!
//! 末尾から **2番目** の 4KB セクタに `naginata_core::config` の IR バイナリを保存する
//! （末尾セクタは EE_HANDS 手番が使用済み。十分離れたプログラム外領域）。
//! 起動時に読み、未保存/破損(magic/CRC 不一致)なら `Config::default()` へフォールバックする。
//!
//! 安全性: embassy-rp の blocking_erase/write は内部で in_ram（core1停止＋critical_section）を
//! 行うため XIP 無効化が安全。書込みは PC からの COMMIT 時のみ（稀）。

use embassy_rp::flash::{ERASE_SIZE, WRITE_SIZE};
use naginata_core::config::{self, Config};

use crate::handedness::{HandFlash, FLASH_SIZE};

/// 設定セクタ = 末尾から2番目の4KBセクタ先頭（手番セクタ FLASH_SIZE-ERASE_SIZE と非衝突）。
const CONFIG_OFFSET: u32 = (FLASH_SIZE - 2 * ERASE_SIZE) as u32;

/// 書込みバッファ長。IR v2 の現実的最大（全カテゴリ差分）を収め、WRITE_SIZE(256) の倍数かつ
/// 4KBセクタ未満。3840 = 15ページ。これを超える設定は serialize が None → COMMIT が ST_ERR。
const FLASH_BUF: usize = 3840;

/// 保存された設定を読む。未保存(全0xFF)・magic/CRC 不一致なら None（→ 既定を使う）。
pub fn read_config(flash: &mut HandFlash) -> Option<Config> {
    let mut buf = [0u8; FLASH_BUF];
    flash.blocking_read(CONFIG_OFFSET, &mut buf).ok()?;
    config::parse(&buf)
}

/// 設定を書込む（PC からの COMMIT）。セクタ消去 → IR を1ブロック書込み。
/// IR を 0xFF パディングで WRITE_SIZE 倍数に揃えて書く。
pub fn write_config(flash: &mut HandFlash, cfg: &Config) -> Result<(), ()> {
    let mut buf = [0xFFu8; FLASH_BUF];
    let n = config::serialize(cfg, &mut buf).ok_or(())?;
    // WRITE_SIZE 倍数へ切り上げ（残りは 0xFF のまま）。
    let write_len = n.div_ceil(WRITE_SIZE) * WRITE_SIZE;
    flash
        .blocking_erase(CONFIG_OFFSET, CONFIG_OFFSET + ERASE_SIZE as u32)
        .map_err(|_| ())?;
    flash
        .blocking_write(CONFIG_OFFSET, &buf[..write_len])
        .map_err(|_| ())
}

/// 設定を消去（RESET → 既定復帰）。セクタを消去するだけ（次回 read で None → 既定）。
pub fn erase_config(flash: &mut HandFlash) -> Result<(), ()> {
    flash
        .blocking_erase(CONFIG_OFFSET, CONFIG_OFFSET + ERASE_SIZE as u32)
        .map_err(|_| ())
}
