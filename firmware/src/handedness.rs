//! 手番(左右)の永続化 — EE_HANDS 方式（#05 / §3.1, 詳細設計 D.2）。
//!
//! 同一バイナリのまま左右を区別するため、フラッシュ末尾4KBセクタに
//! `"NGHD" + 'L'/'R'` を保存し、起動時に読む。書込みは初回プロビジョニングのみ。
//!
//! 安全性: embassy-rp の blocking_erase/write は内部で `in_ram`
//! （core1停止＋critical_section＋DMA待ち）を行うため、XIP無効化が安全に処理される。

use embassy_rp::flash::{Blocking, Flash, ERASE_SIZE};
use embassy_rp::peripherals::FLASH;
use embassy_rp::Peri;
use naginata_core::keymap::Hand;

/// Pico/Pico W の QSPI フラッシュ容量（2MB）。
pub const FLASH_SIZE: usize = 2 * 1024 * 1024;
/// 保存位置 = 末尾4KBセクタ先頭（プログラム領域と十分離れている）。
const STORAGE_OFFSET: u32 = (FLASH_SIZE - ERASE_SIZE) as u32;
const MAGIC: [u8; 4] = *b"NGHD";

pub type HandFlash<'d> = Flash<'d, FLASH, Blocking, FLASH_SIZE>;

/// FLASH ペリフェラルから blocking フラッシュドライバを作る。
pub fn new_flash<'d>(flash: Peri<'d, FLASH>) -> HandFlash<'d> {
    Flash::new_blocking(flash)
}

/// 保存された手番を読む。未プロビジョニング(magic不一致)なら None。
pub fn read_hand(flash: &mut HandFlash) -> Option<Hand> {
    let mut buf = [0u8; 5];
    flash.blocking_read(STORAGE_OFFSET, &mut buf).ok()?;
    if buf[0..4] != MAGIC {
        return None;
    }
    match buf[4] {
        b'L' => Some(Hand::Left),
        b'R' => Some(Hand::Right),
        _ => None,
    }
}

/// 手番を書込む（初回プロビジョニング）。1ページ(256B)消去→書込み。
pub fn write_hand(flash: &mut HandFlash, hand: Hand) -> Result<(), ()> {
    let mut page = [0xFFu8; 256];
    page[0..4].copy_from_slice(&MAGIC);
    page[4] = match hand {
        Hand::Left => b'L',
        Hand::Right => b'R',
    };
    flash
        .blocking_erase(STORAGE_OFFSET, STORAGE_OFFSET + ERASE_SIZE as u32)
        .map_err(|_| ())?;
    flash.blocking_write(STORAGE_OFFSET, &page).map_err(|_| ())?;
    Ok(())
}
