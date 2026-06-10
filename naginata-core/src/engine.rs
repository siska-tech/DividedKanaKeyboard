//! 同時押し判定エンジン v3（薙刀式シフト体系・順序非依存）。
//!
//! 入力: sc 単位の press/release/tick + ミリ秒タイムスタンプ。
//! 出力: `Action`（Kana/Keys）を `out: &mut dyn FnMut(Action)` へ push。
//!
//! モデル（詳細設計 A の発展）:
//!   - `pending`: 役割未確定の1キー（次キーで2キー分解 or 単打フラッシュ）。
//!   - `held_shifts`(ShiftMask): 確定した持続シフト → 連続シフトが成立。
//!   - **2キー分解 `resolve_pair`**: 押し順に依存せず (mask,base) の有効な割当を選ぶ。
//!     これにより「か(f)+右濁(j)→が」のような**シフトキー同士の濁音**も両順序で成立。
//!   - シフトキーが pending の間は tick でフラッシュしない（連続シフト待ち）。
//!
//! 出力 Action::Kana / Keys の振り分けはファーム側。

use crate::hid::KeyPress;
use crate::{keymap, Action};
use heapless::Vec;

const MAX: usize = 8;

/// キーリピート既定値（#11 / FR-7.1）。初回ディレイの後この間隔で再送。
pub const REPEAT_DELAY_MS: u32 = 300;
pub const REPEAT_INTERVAL_MS: u32 = 40;

/// リピート対象の HID Usage（修飾なしの単一キーのみ）。
/// BS / Del / ←→↑↓。編集モードで長押し連続移動・連続削除に使う。
/// （将来 #12 でランタイム設定化する想定。）
const REPEAT_USAGES: &[u8] = &[
    0x2a, // BackSpace
    0x4c, // Delete
    0x4f, // Right
    0x50, // Left
    0x51, // Down
    0x52, // Up
];

/// 編集モードで長押し中の制御キー（オートリピート用）。
struct Repeat {
    sc: u8,        // 押下継続中のキー（解放で停止）
    action: Action, // 再送するアクション
    next_at: u32,  // 次に再送する時刻（ms）
}

/// `action` が単一・修飾なしのリピート対象キーなら true。
fn is_repeatable(action: Action) -> bool {
    if let Action::Keys([KeyPress { usage, modifiers: 0 }]) = action {
        REPEAT_USAGES.contains(usage)
    } else {
        false
    }
}

pub struct Engine {
    window_ms: u32,
    held_shifts: u8,               // 確定した持続シフト集合
    shift_src: Vec<(u8, u8), MAX>, // (sc, bit) 確定シフトの出所
    pending: Option<(u8, u32)>,     // 役割未確定の1キー
    pend3: Option<(u8, u8, u32)>,   // 3キー待ち: 確定保留した2キー(a,b)+時刻
    mode: Option<u8>,               // 編集モード id（トリガ2キー保持中）
    mode_keys: (u8, u8),            // 編集モードのトリガキー（終了判定用）
    repeat: Option<Repeat>,         // 編集モードの制御キー長押し（#11）
    repeat_delay_ms: u32,           // 初回ディレイ
    repeat_interval_ms: u32,        // 以降のリピート間隔
    // 全カテゴリのランタイム・オーバーレイ（#12 フェーズ2）。
    // firmware が flash 設定から `static` アリーナを作り渡す。既定は空＝codegen 既定のみ。
    overlay: &'static keymap::Overlay,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self::with_window(keymap::WINDOW_MS)
    }

    pub fn with_window(window_ms: u32) -> Self {
        Self {
            window_ms,
            held_shifts: 0,
            shift_src: Vec::new(),
            pending: None,
            pend3: None,
            mode: None,
            mode_keys: (0, 0),
            repeat: None,
            repeat_delay_ms: REPEAT_DELAY_MS,
            repeat_interval_ms: REPEAT_INTERVAL_MS,
            overlay: &keymap::EMPTY_OVERLAY,
        }
    }

    pub fn set_window(&mut self, window_ms: u32) {
        self.window_ms = window_ms;
    }

    /// ランタイム・オーバーレイを設定する（#12 フェーズ2 ランタイム配列変更）。
    /// firmware が flash 設定を `static` アリーナへ展開して渡す。`EMPTY_OVERLAY` で既定へ戻る。
    pub fn set_overlay(&mut self, overlay: &'static keymap::Overlay) {
        self.overlay = overlay;
    }

    /// レイヤ解決（`keymap::layer_lookup` 互換）にオーバーレイを重ねる（#12）。
    ///
    /// 優先順: mask≠0 → overlay(mask,sc) > 静的(mask,sc) > overlay(0,sc) > 静的(0,sc)。
    /// mask==0 → overlay(0,sc) > 静的(0,sc)。
    /// オーバーレイが空のとき全 (mask,sc) で `keymap::layer_lookup` と一致（既存意味論を保つ）。
    fn resolve(&self, mask: u8, sc: u8) -> Action {
        if mask != 0 {
            if let Some(a) = self.overlay.find_layer(mask, sc) {
                return a;
            }
            if keymap::layer_has(mask, sc) {
                return keymap::layer_lookup(mask, sc);
            }
            // 厳密シフト一致なし → 単打段へフォールバック
        }
        if let Some(a) = self.overlay.find_layer(0, sc) {
            return a;
        }
        keymap::layer_lookup(0, sc)
    }

    /// (mask,sc) の厳密エントリが存在するか（overlay ∪ 静的）。シフト分解の割当判定に使う。
    fn has_layer(&self, mask: u8, sc: u8) -> bool {
        self.overlay.find_layer(mask, sc).is_some() || keymap::layer_has(mask, sc)
    }

    /// 2キーコンボ（overlay 先引き→静的）。
    fn combo2(&self, a: u8, b: u8) -> Action {
        if let Some(x) = self.overlay.find_combo2(a, b) {
            return x;
        }
        keymap::combo2_lookup(a, b)
    }

    /// 3キーコンボ（overlay 先引き→静的）。
    fn combo3(&self, a: u8, b: u8, c: u8) -> Action {
        if let Some(x) = self.overlay.find_combo3(a, b, c) {
            return x;
        }
        keymap::combo3_lookup(a, b, c)
    }

    /// 編集モードアクション（overlay 先引き→静的）。
    fn mode_action(&self, mode: u8, sc: u8) -> Action {
        if let Some(x) = self.overlay.find_mode(mode, sc) {
            return x;
        }
        keymap::mode_lookup(mode, sc)
    }

    /// {a,b} がいずれかの3キーコンボの部分集合か（静的 ∪ overlay 追加分）。
    fn is_combo3_prefix(&self, a: u8, b: u8) -> bool {
        keymap::is_combo3_prefix(a, b) || self.overlay.has_combo3_prefix(a, b)
    }

    /// キーリピートのタイミングを設定（FR-7.1）。
    /// `delay_ms`: 押下から初回リピートまで。`interval_ms`: 以降の間隔。
    pub fn set_repeat(&mut self, delay_ms: u32, interval_ms: u32) {
        self.repeat_delay_ms = delay_ms;
        self.repeat_interval_ms = interval_ms;
    }

    /// 現在アクティブな持続シフト集合（ShiftMask）。
    pub fn held_shifts(&self) -> u8 {
        self.held_shifts
    }

    /// LED表示用のシフト集合: 確定シフト＋**保留中のシフトキー**。
    /// シフトキーは単独押下だと held_shifts に未反映（次キーで確定）なので、
    /// 「押している最中」を見せるため pending のシフトビットも含める。
    pub fn display_shifts(&self) -> u8 {
        let mut m = self.held_shifts;
        if let Some((p, _)) = self.pending {
            if let Some(bit) = keymap::shift_mask_of(p) {
                m |= bit;
            }
        }
        m
    }

    // ---- イベント ---------------------------------------------------------

    pub fn press(&mut self, sc: u8, now: u32, out: &mut dyn FnMut(Action)) {
        // 編集モード中: 非トリガキー → モードアクション（カーソル/コピペ等）
        if let Some(mode) = self.mode {
            if sc != self.mode_keys.0 && sc != self.mode_keys.1 {
                let action = self.mode_action(mode, sc);
                emit(out, action);
                // BS/←/→ 等は長押しで初回ディレイ後にリピート開始（#11）。
                self.repeat = if is_repeatable(action) {
                    Some(Repeat { sc, action, next_at: now.wrapping_add(self.repeat_delay_ms) })
                } else {
                    None
                };
            }
            return;
        }

        // 3キー待ち（外来音等）: 保留中の2キー(a,b)に3キー目 sc
        if let Some((a, b, _)) = self.pend3.take() {
            let k3 = self.combo3(a, b, sc);
            if k3 != Action::None {
                emit(out, k3); // 3キー成立
                return;
            }
            // 不成立 → 2キーを確定（遅延無し）してから sc を通常処理
            self.resolve_pair(a, b, now, out, false);
            // fall through で sc を処理
        }

        // 連続シフト中
        if self.held_shifts != 0 {
            match keymap::shift_mask_of(sc) {
                // シフトキーだが現シフト下でベース定義あり（例: 連続濁音中の が）
                Some(_) if self.has_layer(self.held_shifts, sc) => {
                    emit(out, self.resolve(self.held_shifts, sc));
                }
                // 別のシフト → 二重シフトへ累積
                Some(bit) => self.commit(sc, bit),
                // 通常ベース（未定義は単打へフォールバック）
                None => emit(out, self.resolve(self.held_shifts, sc)),
            }
            return;
        }

        // held_shifts == 0: pending と2キー分解
        match self.pending.take() {
            None => self.pending = Some((sc, now)),
            Some((p, _)) => self.resolve_pair(p, sc, now, out, true),
        }
    }

    pub fn release(&mut self, sc: u8, now: u32, out: &mut dyn FnMut(Action)) {
        // リピート中のキー解放 → 即停止（#11）
        if matches!(&self.repeat, Some(r) if r.sc == sc) {
            self.repeat = None;
        }
        // 編集モード: トリガキー解放でモード終了（保険でリピートも停止）
        if self.mode.is_some() && (sc == self.mode_keys.0 || sc == self.mode_keys.1) {
            self.mode = None;
            self.repeat = None;
            return;
        }
        // 編集モード中の非トリガキー解放は、かな状態（pending/shift）へ波及させない。
        if self.mode.is_some() {
            return;
        }
        // 3キー待ちのペアのキー解放 → 3キー不成立、2キーを確定
        if let Some((a, b, _)) = self.pend3 {
            if sc == a || sc == b {
                self.pend3 = None;
                self.resolve_pair(a, b, now, out, false);
                // fall through で sc の通常解放処理へ
            }
        }
        // 持続シフトの解放
        if let Some(i) = self.shift_src.iter().position(|&(s, _)| s == sc) {
            let (_, bit) = self.shift_src.swap_remove(i);
            self.held_shifts &= !bit;
        }
        // pending本体の解放 → 単打フラッシュ（解放なのでリピートは開始しない）
        if matches!(self.pending, Some((p, _)) if p == sc) {
            let _ = self.flush_pending(out);
        }
    }

    pub fn tick(&mut self, now: u32, out: &mut dyn FnMut(Action)) {
        // キーリピート（編集モードの制御キー長押し, #11）。
        // 期限到達で1回再送し、次回を now+interval に再設定（取りこぼし時もバースト無し）。
        let interval = self.repeat_interval_ms;
        if let Some(r) = &mut self.repeat {
            if (now.wrapping_sub(r.next_at) as i32) >= 0 {
                let action = r.action;
                r.next_at = now.wrapping_add(interval);
                emit(out, action);
            }
        }
        // 3キー待ちが窓超過 → 3キー目は来ない、2キーを確定
        if let Some((a, b, t)) = self.pend3 {
            if now.wrapping_sub(t) > self.window_ms {
                self.pend3 = None;
                self.resolve_pair(a, b, now, out, false);
            }
            return;
        }
        // 非シフトの pending のみ窓超過で単打確定（シフトキーは連続シフト待ちで保持）。
        if let Some((p, t)) = self.pending {
            if keymap::shift_mask_of(p).is_none() && now.wrapping_sub(t) > self.window_ms {
                // 窓超過での確定 = キーは押下継続中。単打面の制御キー（BS/←/→等）なら
                // 初回ディレイ後にリピート開始（#11。release 経由の確定は押下解放なので不可）。
                if let Some((sc, action)) = self.flush_pending(out) {
                    if is_repeatable(action) {
                        self.repeat = Some(Repeat {
                            sc,
                            action,
                            next_at: now.wrapping_add(self.repeat_delay_ms),
                        });
                    }
                }
            }
        }
    }

    // ---- 内部 -------------------------------------------------------------

    /// pending(p) と新規(k) の2キーを、押し順に依存せず解決する。
    /// p と k の2キーを解決する。`allow_defer` 時、{p,k} が3キーコンボの一部なら
    /// 即確定せず pend3 へ保留して3キー目を待つ（外来音の衝突回避, 大岡式）。
    fn resolve_pair(&mut self, p: u8, k: u8, now: u32, out: &mut dyn FnMut(Action), allow_defer: bool) {
        // 0) 編集モードトリガ（最優先）: 2キーで持続モードへ入る（出力なし）
        if let Some(mode) = keymap::mode_trigger(p, k) {
            self.mode = Some(mode);
            self.mode_keys = (p, k);
            return;
        }

        // 0.5) 3キーコンボの前置なら確定を遅延（3キー目を待つ）
        if allow_defer && self.is_combo3_prefix(p, k) {
            self.pend3 = Some((p, k, now));
            return;
        }

        let mp = keymap::shift_mask_of(p);
        let mk = keymap::shift_mask_of(k);

        // 1) p=シフト, k=ベース
        if let Some(bp) = mp {
            if self.has_layer(bp, k) {
                emit(out, self.resolve(bp, k));
                self.commit(p, bp);
                return;
            }
        }
        // 2) k=シフト, p=ベース（後置/逆順）
        if let Some(bk) = mk {
            if self.has_layer(bk, p) {
                emit(out, self.resolve(bk, p));
                self.commit(k, bk);
                return;
            }
        }
        // 3) 両方シフト → 二重シフト累積（ベース待ち）
        if let (Some(bp), Some(bk)) = (mp, mk) {
            self.commit(p, bp);
            self.commit(k, bk);
            return;
        }
        // 4) p=シフトのみ → p継続, k=ベース（フォールバック）
        if let Some(bp) = mp {
            self.commit(p, bp);
            emit(out, self.resolve(bp, k));
            return;
        }
        // 5) k=シフトのみ → p単打, k継続
        if let Some(bk) = mk {
            emit(out, self.resolve(0, p));
            self.commit(k, bk);
            return;
        }
        // 6) 両方ベース → まず2キーコンボ（清音拗音 等, 順不同）を照合
        let combo = self.combo2(p, k);
        if combo != Action::None {
            emit(out, combo);
            return;
        }
        // コンボ無し → p単打, k を pending（逐次入力）
        emit(out, self.resolve(0, p));
        self.pending = Some((k, now));
    }

    fn commit(&mut self, sc: u8, bit: u8) {
        if self.held_shifts & bit == 0 {
            self.held_shifts |= bit;
            let _ = self.shift_src.push((sc, bit));
        }
    }

    /// pending を単打として確定・emit する。emit した (sc, Action) を返す（無ければ None）。
    /// 返り値で「押下継続中の単打が制御キーか」を呼び出し側が判定しリピート開始に使う。
    fn flush_pending(&mut self, out: &mut dyn FnMut(Action)) -> Option<(u8, Action)> {
        if let Some((p, _)) = self.pending.take() {
            let a = self.resolve(self.held_shifts, p);
            emit(out, a);
            Some((p, a))
        } else {
            None
        }
    }
}

fn emit(out: &mut dyn FnMut(Action), a: Action) {
    if a != Action::None {
        out(a);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hid::KeyPress;

    /// イベント列を流し、emit された Action を集める。kind: 'p'|'r'|'t'
    fn run(events: &[(char, u8, u32)]) -> heapless::Vec<Action, 16> {
        let mut e = Engine::with_window(40);
        let mut out: heapless::Vec<Action, 16> = heapless::Vec::new();
        for &(kind, sc, now) in events {
            let mut sink = |a: Action| {
                let _ = out.push(a);
            };
            match kind {
                'p' => e.press(sc, now, &mut sink),
                'r' => e.release(sc, now, &mut sink),
                't' => e.tick(now, &mut sink),
                _ => unreachable!(),
            }
        }
        out
    }

    /// run と同じだが、キーリピートを (delay, interval) に設定して実行する。
    fn run_rep(delay: u32, interval: u32, events: &[(char, u8, u32)]) -> heapless::Vec<Action, 16> {
        let mut e = Engine::with_window(40);
        e.set_repeat(delay, interval);
        let mut out: heapless::Vec<Action, 16> = heapless::Vec::new();
        for &(kind, sc, now) in events {
            let mut sink = |a: Action| {
                let _ = out.push(a);
            };
            match kind {
                'p' => e.press(sc, now, &mut sink),
                'r' => e.release(sc, now, &mut sink),
                't' => e.tick(now, &mut sink),
                _ => unreachable!(),
            }
        }
        out
    }

    /// オーバーレイ(#12)を設定して実行する。
    fn run_ovr(
        ovr: &'static keymap::Overlay,
        events: &[(char, u8, u32)],
    ) -> heapless::Vec<Action, 16> {
        let mut e = Engine::with_window(40);
        e.set_overlay(ovr);
        let mut out: heapless::Vec<Action, 16> = heapless::Vec::new();
        for &(kind, sc, now) in events {
            let mut sink = |a: Action| {
                let _ = out.push(a);
            };
            match kind {
                'p' => e.press(sc, now, &mut sink),
                'r' => e.release(sc, now, &mut sink),
                't' => e.tick(now, &mut sink),
                _ => unreachable!(),
            }
        }
        out
    }

    // 単打: w=き / シフト単打: f=か / q長押し単打=ヴ
    #[test]
    fn single_tap_base() {
        assert_eq!(run(&[('p', 0x11, 0), ('r', 0x11, 10)]), [Action::Kana("き")]);
    }

    // --- #12 オーバーレイ（フェーズ2: 全カテゴリ）-------------------------
    /// w(0x11) の単打を き→め に差し替える（mask=0 layers）。
    #[test]
    fn override_changes_single_tap() {
        static OV: keymap::Overlay = keymap::Overlay {
            layers: &[(keymap::layer_key(0, 0x11), Action::Kana("め"))],
            combo2: &[],
            combo3: &[],
            modes: &[],
        };
        let out = run_ovr(&OV, &[('p', 0x11, 0), ('r', 0x11, 10)]);
        assert_eq!(out, [Action::Kana("め")]);
    }
    /// 単打を差し替えても他キーは無影響。
    #[test]
    fn override_does_not_affect_other_keys() {
        static OV: keymap::Overlay = keymap::Overlay {
            layers: &[(keymap::layer_key(0, 0x11), Action::Kana("め"))],
            combo2: &[],
            combo3: &[],
            modes: &[],
        };
        let out = run_ovr(&OV, &[('p', 0x12, 0), ('r', 0x12, 10)]);
        assert_eq!(out, [Action::Kana("て")]);
    }
    /// 空オーバーレイは既定と完全一致（回帰なし）。
    #[test]
    fn empty_override_matches_default() {
        let out = run_ovr(&keymap::EMPTY_OVERLAY, &[('p', 0x11, 0), ('r', 0x11, 10)]);
        assert_eq!(out, [Action::Kana("き")]);
    }
    /// シフト面(センターシフト)のかなを差し替える: CENTER+u(0x16) さ→ＸＸ。
    #[test]
    fn override_changes_shifted_face() {
        static OV: keymap::Overlay = keymap::Overlay {
            layers: &[(keymap::layer_key(0x01, 0x16), Action::Kana("ＸＸ"))], // CENTER=bit0
            combo2: &[],
            combo3: &[],
            modes: &[],
        };
        let out = run_ovr(
            &OV,
            &[('p', 0x39, 0), ('p', 0x16, 5), ('r', 0x16, 30), ('r', 0x39, 35)],
        );
        assert_eq!(out, [Action::Kana("ＸＸ")]);
        // 単打(mask=0)の u は不変（既定 BS = Action::Keys[usage 0x2a]）。
        let single = run_ovr(&OV, &[('p', 0x16, 0), ('r', 0x16, 10)]);
        assert_eq!(single, [Action::Keys(&[crate::hid::KeyPress { usage: 0x2a, modifiers: 0 }])]);
    }
    /// combo2 を置換: き(0x11)+や(0x23) きゃ→ＸＸ。
    #[test]
    fn override_replaces_combo2() {
        static OV: keymap::Overlay = keymap::Overlay {
            layers: &[],
            combo2: &[(keymap::combo2_key(0x11, 0x23), Action::Kana("ＸＸ"))],
            combo3: &[],
            modes: &[],
        };
        let out = run_ovr(&OV, &[('p', 0x11, 0), ('p', 0x23, 5), ('r', 0x11, 30), ('r', 0x23, 35)]);
        assert_eq!(out, [Action::Kana("ＸＸ")]);
    }
    /// combo2 を新規追加: 既定に無い 2キー(0x1e+0x1f)→ＮＥＷ。
    #[test]
    fn override_adds_new_combo2() {
        static OV: keymap::Overlay = keymap::Overlay {
            layers: &[],
            combo2: &[(keymap::combo2_key(0x1e, 0x1f), Action::Kana("ＮＥＷ"))],
            combo3: &[],
            modes: &[],
        };
        let out = run_ovr(&OV, &[('p', 0x1e, 0), ('p', 0x1f, 5), ('r', 0x1e, 30), ('r', 0x1f, 35)]);
        assert_eq!(out, [Action::Kana("ＮＥＷ")]);
    }
    /// 編集モードのキー操作を差し替える: モード1の j(0x24) Up→Down。
    #[test]
    fn override_changes_mode_action() {
        static DOWN: [crate::hid::KeyPress; 1] = [crate::hid::KeyPress { usage: 0x51, modifiers: 0 }];
        static OV: keymap::Overlay = keymap::Overlay {
            layers: &[],
            combo2: &[],
            combo3: &[],
            modes: &[(keymap::layer_key(1, 0x24), Action::Keys(&DOWN))],
        };
        // モード1トリガ D(0x20)+F(0x21) を押しながら j。
        let out = run_ovr(
            &OV,
            &[('p', 0x20, 0), ('p', 0x21, 5), ('p', 0x24, 20), ('r', 0x24, 25)],
        );
        assert_eq!(out, [Action::Keys(&DOWN)]);
    }
    #[test]
    fn shift_key_tap_is_its_base() {
        assert_eq!(run(&[('p', 0x21, 0), ('r', 0x21, 10)]), [Action::Kana("か")]);
    }
    #[test]
    fn long_solo_tap_of_shift_key_still_emits_kana() {
        let out = run(&[('p', 0x10, 0), ('t', 0, 50), ('t', 0, 90), ('r', 0x10, 100)]);
        assert_eq!(out, [Action::Kana("ヴ")]);
    }

    // センターシフト 前置/後置/連続/間あき
    #[test]
    fn center_shift_prefix() {
        let out = run(&[('p', 0x39, 0), ('p', 0x16, 5), ('r', 0x16, 30), ('r', 0x39, 35)]);
        assert_eq!(out, [Action::Kana("さ")]);
    }
    #[test]
    fn center_shift_postfix() {
        let out = run(&[('p', 0x16, 0), ('p', 0x39, 10), ('r', 0x16, 30), ('r', 0x39, 35)]);
        assert_eq!(out, [Action::Kana("さ")]);
    }
    #[test]
    fn continuous_shift_applies_to_multiple_bases() {
        let out = run(&[
            ('p', 0x39, 0), ('p', 0x16, 5), ('r', 0x16, 10),
            ('p', 0x17, 15), ('r', 0x17, 20), ('r', 0x39, 25),
        ]);
        assert_eq!(out, [Action::Kana("さ"), Action::Kana("よ")]);
    }
    #[test]
    fn continuous_shift_applies_after_gap() {
        let out = run(&[('p', 0x39, 0), ('t', 0, 50), ('p', 0x16, 55), ('r', 0x16, 60)]);
        assert_eq!(out, [Action::Kana("さ")]);
    }
    #[test]
    fn pending_base_flushes_as_single_on_timeout() {
        assert_eq!(run(&[('p', 0x11, 0), ('t', 0, 50)]), [Action::Kana("き")]);
    }
    #[test]
    fn sequential_singles() {
        let out = run(&[('p', 0x11, 0), ('p', 0x12, 5), ('r', 0x11, 10), ('r', 0x12, 15)]);
        assert_eq!(out, [Action::Kana("き"), Action::Kana("て")]);
    }

    // 濁音: 非シフトベース（ぎ = 右濁j + w）両順序
    #[test]
    fn dakuten_nonshift_base_both_orders() {
        let a = run(&[('p', 0x24, 0), ('p', 0x11, 5), ('r', 0x11, 20), ('r', 0x24, 25)]);
        assert_eq!(a, [Action::Kana("ぎ")]);
        let b = run(&[('p', 0x11, 0), ('p', 0x24, 5), ('r', 0x11, 20), ('r', 0x24, 25)]);
        assert_eq!(b, [Action::Kana("ぎ")]);
    }

    // 濁音: シフトキー同士（が = か(f,左濁) + 右濁j）両順序 ← v3の核心
    #[test]
    fn dakuten_shift_base_both_orders() {
        let a = run(&[('p', 0x24, 0), ('p', 0x21, 5), ('r', 0x21, 20), ('r', 0x24, 25)]);
        assert_eq!(a, [Action::Kana("が")]);
        let b = run(&[('p', 0x21, 0), ('p', 0x24, 5), ('r', 0x21, 20), ('r', 0x24, 25)]);
        assert_eq!(b, [Action::Kana("が")]);
    }

    // ご = こ(v,左半) + 右濁j（シフト同士）
    #[test]
    fn dakuten_go() {
        let out = run(&[('p', 0x2f, 0), ('p', 0x24, 5), ('r', 0x2f, 20), ('r', 0x24, 25)]);
        assert_eq!(out, [Action::Kana("ご")]);
    }

    // 半濁音: ぱ = は(c) + 右半m
    #[test]
    fn handakuten_pa() {
        let out = run(&[('p', 0x32, 0), ('p', 0x2e, 5), ('r', 0x2e, 20), ('r', 0x32, 25)]);
        assert_eq!(out, [Action::Kana("ぱ")]);
    }

    // 小書き: ゃ = 小q + や位置h
    #[test]
    fn small_ya() {
        let out = run(&[('p', 0x10, 0), ('p', 0x23, 5), ('r', 0x23, 20), ('r', 0x10, 25)]);
        assert_eq!(out, [Action::Kana("ゃ")]);
    }

    // 連続濁音: 右濁j 保持で複数（ぎ・が）。が はシフトキーベース
    #[test]
    fn continuous_dakuten_including_shift_base() {
        let out = run(&[
            ('p', 0x24, 0),         // j 保持
            ('p', 0x11, 5), ('r', 0x11, 10),  // w → ぎ
            ('p', 0x21, 15), ('r', 0x21, 20), // f → が（シフトキーがベース）
            ('r', 0x24, 25),
        ]);
        assert_eq!(out, [Action::Kana("ぎ"), Action::Kana("が")]);
    }

    // IME: j+h = LANG1（Action::Keys）
    #[test]
    fn ime_on_emits_keys() {
        let out = run(&[('p', 0x24, 0), ('p', 0x23, 6), ('r', 0x23, 30), ('r', 0x24, 36)]);
        const LANG1: &[KeyPress] = &[KeyPress::new(0x90, 0)];
        assert_eq!(out, [Action::Keys(LANG1)]);
    }

    // 句点: センター(space) + v = 、{Enter}（Comma=0x36, Enter=0x28）
    #[test]
    fn center_v_emits_touten() {
        let out = run(&[('p', 0x39, 0), ('p', 0x2f, 5), ('r', 0x2f, 20), ('r', 0x39, 25)]);
        const TOUTEN: &[KeyPress] = &[KeyPress::new(0x36, 0), KeyPress::new(0x28, 0)];
        assert_eq!(out, [Action::Keys(TOUTEN)]);
    }

    // 外来音(3キー): でぃ = 右濁(j,0x24)+て(e,0x12)+い(k,0x25)
    #[test]
    fn gairai_di_3key() {
        let out = run(&[
            ('p', 0x24, 0),  // j（で の前置 → 確定遅延）
            ('p', 0x12, 5),  // e → pend3
            ('p', 0x25, 10), // k → でぃ 成立
            ('r', 0x25, 30), ('r', 0x12, 35), ('r', 0x24, 40),
        ]);
        assert_eq!(out, [Action::Kana("でぃ")]);
    }

    // 3キー目が来なければ2キー（で）に確定（窓超過）
    #[test]
    fn gairai_prefix_falls_back_to_dakuten() {
        let out = run(&[('p', 0x24, 0), ('p', 0x12, 5), ('t', 0, 50)]);
        assert_eq!(out, [Action::Kana("で")]);
    }

    // とぅ = 右半(m,0x32)+と(d,0x20)+う(l,0x26)
    #[test]
    fn gairai_tou_3key() {
        let out = run(&[
            ('p', 0x32, 0), ('p', 0x20, 5), ('p', 0x26, 10),
            ('r', 0x26, 30), ('r', 0x20, 35), ('r', 0x32, 40),
        ]);
        assert_eq!(out, [Action::Kana("とぅ")]);
    }

    // 3キー前置でない濁音（が=j+f）は遅延せず即確定（回帰）
    #[test]
    fn ga_not_deferred() {
        let out = run(&[('p', 0x24, 0), ('p', 0x21, 5), ('r', 0x21, 20), ('r', 0x24, 25)]);
        assert_eq!(out, [Action::Kana("が")]);
    }

    // 濁音拗音(3キー): ぎゃ = 右濁(j)+き(w,0x11)+や(h,0x23)
    #[test]
    fn dakuon_youon_gya() {
        let out = run(&[
            ('p', 0x24, 0), ('p', 0x11, 5), ('p', 0x23, 10),
            ('r', 0x23, 30), ('r', 0x11, 35), ('r', 0x24, 40),
        ]);
        assert_eq!(out, [Action::Kana("ぎゃ")]);
    }

    // きゃ/ぎゃ の曖昧性: き+や は遅延し、右濁が来なければ きゃ（清音拗音）に確定
    #[test]
    fn kya_defers_then_falls_back() {
        let out = run(&[('p', 0x11, 0), ('p', 0x23, 5), ('t', 0, 50)]);
        assert_eq!(out, [Action::Kana("きゃ")]);
    }

    // フ系(3キー): ふぁ = 左半(v,0x2f)+ふ(;,0x27)+あ(j,0x24)
    #[test]
    fn gairai_fa() {
        let out = run(&[
            ('p', 0x2f, 0), ('p', 0x27, 5), ('p', 0x24, 10),
            ('r', 0x24, 30), ('r', 0x27, 35), ('r', 0x2f, 40),
        ]);
        assert_eq!(out, [Action::Kana("ふぁ")]);
    }

    // 編集モード1: D(0x20)+F(0x21) 押しながら j → Up（↑）、解放で終了
    #[test]
    fn edit_mode1_cursor() {
        const UP: &[KeyPress] = &[KeyPress::new(0x52, 0)];
        let out = run(&[
            ('p', 0x20, 0),  // D
            ('p', 0x21, 5),  // F → モード1入り（出力なし）
            ('p', 0x24, 20), // j → ↑
            ('r', 0x24, 25),
            ('r', 0x21, 30), // F解放 → モード終了
            ('r', 0x20, 35),
        ]);
        assert_eq!(out, [Action::Keys(UP)]);
    }

    // 編集モード2: C(0x2e)+V(0x2f) 押しながら h → C-c（コピー）
    #[test]
    fn edit_mode2_copy() {
        const COPY: &[KeyPress] = &[KeyPress::new(0x06, 0x01)]; // Ctrl+C
        let out = run(&[
            ('p', 0x2e, 0),
            ('p', 0x2f, 5),  // モード2入り
            ('p', 0x23, 20), // h → C-c
            ('r', 0x23, 25),
            ('r', 0x2f, 30),
            ('r', 0x2e, 35),
        ]);
        assert_eq!(out, [Action::Keys(COPY)]);
    }

    // モード終了後は通常入力に戻る
    #[test]
    fn edit_mode_exits() {
        let out = run(&[
            ('p', 0x20, 0), ('p', 0x21, 5),   // モード1
            ('r', 0x20, 10), ('r', 0x21, 15), // 終了
            ('p', 0x11, 50), ('r', 0x11, 55), // w → き（通常）
        ]);
        assert_eq!(out, [Action::Kana("き")]);
    }

    // 清音拗音(2キー): き(0x11)+や(0x23) → きゃ（両押し順, 重なりのみ）
    #[test]
    fn seion_youon_kya() {
        let a = run(&[('p', 0x11, 0), ('p', 0x23, 5), ('r', 0x11, 20), ('r', 0x23, 25)]);
        assert_eq!(a, [Action::Kana("きゃ")]);
        let b = run(&[('p', 0x23, 0), ('p', 0x11, 5), ('r', 0x23, 20), ('r', 0x11, 25)]);
        assert_eq!(b, [Action::Kana("きゃ")]);
    }

    // 拗音にならない逐次入力（重ならない）→ コンボ誤爆しない。
    // き(0x11)→離す→h(0x23) は き＋く（h単独=base く）。
    #[test]
    fn sequential_not_combo() {
        let out = run(&[('p', 0x11, 0), ('r', 0x11, 10), ('p', 0x23, 50), ('r', 0x23, 60)]);
        assert_eq!(out, [Action::Kana("き"), Action::Kana("く")]);
    }

    // Enter: こ(v=0x2f) + な(m=0x32) 同時押し → Enter（両押し順）
    #[test]
    fn ko_na_emits_enter() {
        const ENTER: &[KeyPress] = &[KeyPress::new(0x28, 0)];
        let a = run(&[('p', 0x2f, 0), ('p', 0x32, 5), ('r', 0x32, 20), ('r', 0x2f, 25)]);
        assert_eq!(a, [Action::Keys(ENTER)]);
        let b = run(&[('p', 0x32, 0), ('p', 0x2f, 5), ('r', 0x2f, 20), ('r', 0x32, 25)]);
        assert_eq!(b, [Action::Keys(ENTER)]);
    }

    // ---- #11 キーリピート -------------------------------------------------

    // 単打面の BS(u=0x16) 長押し → 窓確定後に連続削除、解放で停止。
    #[test]
    fn repeat_base_bs() {
        const BS: &[KeyPress] = &[KeyPress::new(0x2a, 0)];
        let out = run_rep(100, 20, &[
            ('p', 0x16, 0),   // u → pending
            ('t', 0, 50),     // 窓(40)超過 → BS確定 + repeat(next=150)
            ('t', 0, 100),    // 期限前
            ('t', 0, 160),    // BS（next=180）
            ('t', 0, 185),    // BS（next=205）
            ('r', 0x16, 190), // 解放 → 停止
            ('t', 0, 260),    // 何も無し
        ]);
        assert_eq!(out, [Action::Keys(BS), Action::Keys(BS), Action::Keys(BS)]);
    }

    // 単打面の ←(t=0x14) 長押し → 連続左移動。
    #[test]
    fn repeat_base_left() {
        const LEFT: &[KeyPress] = &[KeyPress::new(0x50, 0)];
        let out = run_rep(100, 20, &[
            ('p', 0x14, 0), ('t', 0, 50), ('t', 0, 160), ('r', 0x14, 170), ('t', 0, 220),
        ]);
        assert_eq!(out, [Action::Keys(LEFT), Action::Keys(LEFT)]);
    }

    // 単打面 BS をすぐ離す → 1回だけ（押下解放はリピートしない, 回帰）。
    #[test]
    fn base_bs_tap_no_repeat() {
        const BS: &[KeyPress] = &[KeyPress::new(0x2a, 0)];
        let out = run_rep(100, 20, &[
            ('p', 0x16, 0), ('r', 0x16, 10), ('t', 0, 100), ('t', 0, 200),
        ]);
        assert_eq!(out, [Action::Keys(BS)]);
    }

    // 編集モード2で m=Left を長押し → 初回ディレイ後に間隔リピート、解放で停止。
    #[test]
    fn repeat_left_after_delay_and_stops_on_release() {
        const LEFT: &[KeyPress] = &[KeyPress::new(0x50, 0)];
        let out = run_rep(100, 20, &[
            ('p', 0x2e, 0), ('p', 0x2f, 5), // C+V → モード2
            ('p', 0x32, 10),                // m → Left（初回, next=110）
            ('t', 0, 50),                   // 期限前 → 何も無し
            ('t', 0, 115),                  // Left（next=135）
            ('t', 0, 120),                  // 期限前
            ('t', 0, 140),                  // Left（next=160）
            ('r', 0x32, 145),               // 解放 → 停止
            ('t', 0, 200),                  // 何も無し
        ]);
        assert_eq!(out, [Action::Keys(LEFT), Action::Keys(LEFT), Action::Keys(LEFT)]);
    }

    // 編集モード1で o=Del を長押し → 連続削除（BS系・削除のリピート確認）。
    #[test]
    fn repeat_del_in_mode1() {
        const DEL: &[KeyPress] = &[KeyPress::new(0x4c, 0)];
        let out = run_rep(100, 20, &[
            ('p', 0x20, 0), ('p', 0x21, 5), // D+F → モード1
            ('p', 0x18, 10),                // o → Del（初回, next=110）
            ('t', 0, 115),                  // Del
            ('t', 0, 140),                  // Del
            ('r', 0x18, 145),
            ('t', 0, 200),
        ]);
        assert_eq!(out, [Action::Keys(DEL), Action::Keys(DEL), Action::Keys(DEL)]);
    }

    // 非リピート対象（C-c コピー）は長押ししても1回だけ（誤爆防止）。
    #[test]
    fn modifier_action_does_not_repeat() {
        const COPY: &[KeyPress] = &[KeyPress::new(0x06, 0x01)];
        let out = run_rep(100, 20, &[
            ('p', 0x2e, 0), ('p', 0x2f, 5), // モード2
            ('p', 0x23, 10),                // h → C-c
            ('t', 0, 150), ('t', 0, 300),
            ('r', 0x23, 310),
        ]);
        assert_eq!(out, [Action::Keys(COPY)]);
    }

    // 通常のかな入力は tick が来てもリピートしない（単打フラッシュは1回, 回帰）。
    #[test]
    fn kana_does_not_repeat() {
        let out = run_rep(100, 20, &[
            ('p', 0x11, 0),                 // w
            ('t', 0, 50),                   // 窓超過 → き（1回）
            ('t', 0, 100), ('t', 0, 200),
        ]);
        assert_eq!(out, [Action::Kana("き")]);
    }

    // モード終了（トリガ解放）でリピートも止まる。
    #[test]
    fn repeat_stops_when_mode_exits() {
        const LEFT: &[KeyPress] = &[KeyPress::new(0x50, 0)];
        let out = run_rep(100, 20, &[
            ('p', 0x2e, 0), ('p', 0x2f, 5), // モード2
            ('p', 0x32, 10),                // m → Left
            ('r', 0x2f, 15),                // V解放 → モード終了
            ('t', 0, 150), ('t', 0, 300),
        ]);
        assert_eq!(out, [Action::Keys(LEFT)]);
    }
}
