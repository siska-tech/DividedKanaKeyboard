//! キーマトリクススキャン（§2.1 ピンアサイン準拠 / COL2ROW）。
//!
//! 回路図(SCH_Naginata-left)で確定: ダイオード D1-D35 は **アノード=列側 / カソード=行側**、
//! 電流は **列 → スイッチ → ダイオード → 行** に流れる（COL2ROW）。
//! したがって **列(col0-13 = GP2-15)を出力Highでストローブし、行(row0-5 = GP16-21)を
//! Pull-down入力で読む**。押下時に行が High になる。
//!
//! （行を駆動する向きはダイオードが阻止するため不可。列駆動=14ステップ/スキャン）

use embassy_rp::gpio::{Input, Output};
use embassy_time::{Duration, Timer};

pub const COLS: usize = 14;
pub const ROWS: usize = 6;

pub struct Matrix<'d> {
    cols: [Output<'d>; COLS], // drive（スキャン時 High）
    rows: [Input<'d>; ROWS],  // sense（Pull-down）
    /// 簡易デバウンス: 各キーの安定カウンタ（FR-1.3）。
    debounce: [[u8; ROWS]; COLS],
    state: [[bool; ROWS]; COLS],
}

const DEBOUNCE_TICKS: u8 = 3;

impl<'d> Matrix<'d> {
    /// cols: GP2-15 を Output(Low) で、rows: GP16-21 を Input(Pull-down) で渡す。
    pub fn new(cols: [Output<'d>; COLS], rows: [Input<'d>; ROWS]) -> Self {
        Self {
            cols,
            rows,
            debounce: [[0; ROWS]; COLS],
            state: [[false; ROWS]; COLS],
        }
    }

    /// 全列を1回スキャンし、デバウンス後の確定状態の変化を `on_change` へ通知する。
    /// on_change(row, col, pressed)
    pub async fn scan(&mut self, on_change: &mut dyn FnMut(u8, u8, bool)) {
        for c in 0..COLS {
            self.cols[c].set_high();
            // セトリング待ち（GPIO/ダイオードの立ち上がり）。
            Timer::after(Duration::from_micros(5)).await;

            for r in 0..ROWS {
                let raw = self.rows[r].is_high();
                let stable = &mut self.debounce[c][r];
                if raw == self.state[c][r] {
                    *stable = 0;
                } else {
                    *stable += 1;
                    if *stable >= DEBOUNCE_TICKS {
                        self.state[c][r] = raw;
                        *stable = 0;
                        on_change(r as u8, c as u8, raw);
                    }
                }
            }
            self.cols[c].set_low();
        }
    }

    /// 指定位置(row,col)が押されているかを即時に読む（デバウンス無し）。
    /// 起動時プロビジョニング（EE_HANDS, #05）用。
    pub async fn is_held(&mut self, row: usize, col: usize) -> bool {
        self.cols[col].set_high();
        Timer::after(Duration::from_micros(10)).await;
        let pressed = self.rows[row].is_high();
        self.cols[col].set_low();
        pressed
    }
}
