//! build.rs が生成した配列テーブルへの薄いアクセサ（レイヤモデル）。
//!
//! 生成元: layout/naginata.yaml → $OUT_DIR/keymap_gen.rs
//! モデル: 出力 = LAYERS[(mask<<8)|sc]（詳細設計 A.5/B.2）。

use crate::Action;

include!(concat!(env!("OUT_DIR"), "/keymap_gen.rs"));

/// マトリクス位置を sc に変換する手番（左右）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hand {
    Left,
    Right,
}

impl Hand {
    /// 反対の手番。マスタが相方(スレーブ)分を変換する際に使う（§3.1）。
    pub fn opposite(self) -> Hand {
        match self {
            Hand::Left => Hand::Right,
            Hand::Right => Hand::Left,
        }
    }
}

/// (row,col) → sc。未定義の位置は None。
pub fn matrix_to_sc(hand: Hand, row: u8, col: u8) -> Option<u8> {
    let pos = (row << 4) | (col & 0x0f);
    match hand {
        Hand::Left => MATRIX_LEFT.get(&pos).copied(),
        Hand::Right => MATRIX_RIGHT.get(&pos).copied(),
    }
}

/// sc がシフト能力キーなら、その ShiftMask ビット（1<<bit）を返す。
pub fn shift_mask_of(sc: u8) -> Option<u8> {
    SHIFT_KEYS.get(&sc).copied()
}

/// LAYERS / MODE_LAYERS のキー: (mask<<8)|sc（mode<<8|sc）を u16 にパック。
pub const fn layer_key(mask: u8, sc: u8) -> u16 {
    ((mask as u16) << 8) | sc as u16
}

/// レイヤ解決（詳細設計 A.5）: 厳密一致レイヤ → 単打面フォールバック → None。
pub fn layer_lookup(mask: u8, sc: u8) -> Action {
    if let Some(a) = LAYERS.get(&layer_key(mask, sc)) {
        return *a;
    }
    if mask != 0 {
        if let Some(a) = LAYERS.get(&layer_key(0, sc)) {
            return *a;
        }
    }
    Action::None
}

/// (mask, sc) の**厳密な**エントリが存在するか（フォールバックしない）。
/// 同時押し分解で「どちらをシフト/ベースに割り当てるか」を判定するのに使う。
pub fn layer_has(mask: u8, sc: u8) -> bool {
    LAYERS.contains_key(&layer_key(mask, sc))
}

/// 2キーを sorted で u16 にパック（build.rs combo2_pack と一致）。
pub const fn combo2_key(a: u8, b: u8) -> u16 {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    (lo as u16) | ((hi as u16) << 8)
}

/// 非シフト2キー同時押しコンボ（清音拗音 等）。順不同。無ければ None。
pub fn combo2_lookup(a: u8, b: u8) -> Action {
    COMBO2.get(&combo2_key(a, b)).copied().unwrap_or(Action::None)
}

/// 3キーを sorted で u32 にパック（build.rs と一致）。
pub const fn combo3_key(a: u8, b: u8, c: u8) -> u32 {
    // 3要素を昇順ソート（const fn のため swap を手書き）。
    let mut lo = a;
    let mut mid = b;
    let mut hi = c;
    if lo > mid {
        let t = lo;
        lo = mid;
        mid = t;
    }
    if mid > hi {
        let t = mid;
        mid = hi;
        hi = t;
    }
    if lo > mid {
        let t = lo;
        lo = mid;
        mid = t;
    }
    (lo as u32) | (mid as u32) << 8 | (hi as u32) << 16
}

/// 3キー同時押しコンボ（外来音 等）。順不同。無ければ None。
pub fn combo3_lookup(a: u8, b: u8, c: u8) -> Action {
    COMBO3.get(&combo3_key(a, b, c)).copied().unwrap_or(Action::None)
}

/// 2キー {a,b} が、いずれかの3キーコンボの部分集合か（＝確定を遅延すべきか）。
pub fn is_combo3_prefix(a: u8, b: u8) -> bool {
    PREFIX2.contains(&combo2_key(a, b))
}

/// 2キーが編集モードのトリガなら mode id を返す（順不同）。
pub fn mode_trigger(a: u8, b: u8) -> Option<u8> {
    MODE_TRIGGERS.get(&combo2_key(a, b)).copied()
}

/// 編集モード中のキー → アクション。無ければ None。
pub fn mode_lookup(mode: u8, sc: u8) -> Action {
    MODE_LAYERS
        .get(&(((mode as u16) << 8) | sc as u16))
        .copied()
        .unwrap_or(Action::None)
}

/// ランタイム・オーバーレイ（#12 フェーズ2）。各静的テーブルの「先に引く」層。
///
/// 空（既定 [`EMPTY_OVERLAY`]）なら静的テーブル（codegen 既定）と完全一致＝回帰なし。
/// firmware が flash 設定を `static` アリーナへ展開し `&'static` で `Engine::set_overlay` に渡す。
/// ホストテストは `Action::Kana("..")` 等のリテラルでスライスを作る。
/// キーのパックは静的テーブルと同じ（[`layer_key`]/[`combo2_key`]/[`combo3_key`]）。
pub struct Overlay {
    /// layers 上書き: key=(mask<<8)|sc。
    pub layers: &'static [(u16, Action)],
    /// combo2 上書き: key=combo2_key(a,b)。
    pub combo2: &'static [(u16, Action)],
    /// combo3 上書き: key=combo3_key(a,b,c)。
    pub combo3: &'static [(u32, Action)],
    /// 編集モード上書き: key=(mode<<8)|sc。
    pub modes: &'static [(u16, Action)],
}

/// 空オーバーレイ（既定）。`Engine::new` の初期値。
pub static EMPTY_OVERLAY: Overlay = Overlay {
    layers: &[],
    combo2: &[],
    combo3: &[],
    modes: &[],
};

impl Overlay {
    /// layers 上書きを引く（無ければ None）。線形走査（件数小・人間打鍵速度で十分）。
    pub fn find_layer(&self, mask: u8, sc: u8) -> Option<Action> {
        let k = layer_key(mask, sc);
        self.layers.iter().find(|(kk, _)| *kk == k).map(|(_, a)| *a)
    }
    /// combo2 上書きを引く。
    pub fn find_combo2(&self, a: u8, b: u8) -> Option<Action> {
        let k = combo2_key(a, b);
        self.combo2.iter().find(|(kk, _)| *kk == k).map(|(_, a)| *a)
    }
    /// combo3 上書きを引く。
    pub fn find_combo3(&self, a: u8, b: u8, c: u8) -> Option<Action> {
        let k = combo3_key(a, b, c);
        self.combo3.iter().find(|(kk, _)| *kk == k).map(|(_, a)| *a)
    }
    /// 編集モード上書きを引く。
    pub fn find_mode(&self, mode: u8, sc: u8) -> Option<Action> {
        let k = layer_key(mode, sc);
        self.modes.iter().find(|(kk, _)| *kk == k).map(|(_, a)| *a)
    }
    /// overlay.combo3 のいずれかの2キー部分集合に {a,b} が一致するか（追加 combo3 の遅延判定）。
    pub fn has_combo3_prefix(&self, a: u8, b: u8) -> bool {
        let want = combo2_key(a, b);
        self.combo3.iter().any(|(k, _)| {
            let x = (*k & 0xff) as u8;
            let y = ((*k >> 8) & 0xff) as u8;
            let z = ((*k >> 16) & 0xff) as u8;
            combo2_key(x, y) == want || combo2_key(x, z) == want || combo2_key(y, z) == want
        })
    }
}
