//! 薙刀式 同時打鍵かな入力エンジン（コア・ロジック）。
//!
//! ハードウェア非依存・`no_std`。ファーム(embassy)からもホストテストからも使える。
//! 要件: 入力方式の中核を「言語非依存（移植容易）」に保つ（要件 §8.1）。
//!
//! データフロー:
//!   物理キー(row,col,hand) --[keymap]--> sc(スキャンコードID)
//!   sc 列 --[engine: 同時打鍵判定 + シフトレイヤ]--> Action
//!   Action::Kana --[romaji]--> ローマ字 --[hid]--> USB HID キーコード
//!   Action::Keys --[hid]------------------------> USB HID キーコード（直接）
//!
//! テスト時のみ std を有効化（test ハーネスが std を要求するため）。
#![cfg_attr(not(test), no_std)]

pub mod config;
pub mod engine;
pub mod hid;
pub mod keymap;
pub mod romaji;

pub use engine::Engine;
pub use hid::KeyPress;

/// 打鍵解決の結果（詳細設計 A.7）。
///
/// - `Kana`: かな。romaji→hid を経てローマ字入力で確定（FR-4.1）。
/// - `Keys`: 直接 HID キー列（IME ON/OFF, Enter, 編集ショートカット等, FR-4.2）。
/// - `None`: 未定義（無視）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Kana(&'static str),
    Keys(&'static [KeyPress]),
    None,
}
