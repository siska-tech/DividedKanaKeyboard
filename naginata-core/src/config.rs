//! 設定の中間表現 (IR) — フラッシュ永続化と PC 設定チャネルで共有するバイナリ表現（#12）。
//!
//! 位置づけ（Issue #12 実装メモ）:
//!   - codegen（build.rs → keymap_gen.rs）は **既定値の出所**。`Params::default()` は
//!     生成定数 `keymap::WINDOW_MS/TAP_MS` と `engine::REPEAT_*` を参照する。
//!   - フラッシュにはユーザ**差分**だけを保存し、起動時に「既定＋差分」をマージする。
//!   - この `serialize`/`parse` が中間表現のエンコード/デコードで、ビルド時とランタイムで型を共有する。
//!
//! フェーズ2スコープ: パラメータ＋**全カテゴリ**の差分（layers=単打/シフト面, combo2, combo3,
//! modes=編集モードのキー操作）。各エントリ値は かな(UTF-8) か キー列((usage,mod)ペア) のタグ付き。
//!
//! バージョン: v2。v1（単打のみ）の保存データは互換読込せず None（v2初回フラッシュで1回リセット）。
//!
//! `no_std` / no-alloc（`heapless`）。ホストテストでラウンドトリップを検証する。

use heapless::{String, Vec};

/// IR 先頭マジック（"NaGinata ConFig"）。
pub const MAGIC: [u8; 4] = *b"NGCF";
/// IR フォーマットバージョン（v2: 全カテゴリ）。互換を壊す変更で +1。
pub const VERSION: u8 = 2;

/// 1 かなの最大 UTF-8 バイト長（拗音合成「きゃ」「ヴぉ」等も許容）。
pub const MAX_KANA_LEN: usize = 8;
/// 1 エントリのキー操作の最大キー数（"S-Right*20" のような長い列はツール側で切詰め）。
pub const MAX_KEYS: usize = 12;

// 各セクションの差分上限（heapless Vec 容量）。
pub const MAX_LAYERS: usize = 96;
pub const MAX_COMBO2: usize = 24;
pub const MAX_COMBO3: usize = 64;
pub const MAX_MODES: usize = 32;
/// 直接キー割当（未使用物理キー, #12 フェーズ3）の上限。
pub const MAX_EXTRA: usize = 32;

/// LED 輝度スケールの既定値。255 = firmware の status_colors の素の色をそのまま使う。
pub const DEFAULT_LED_BRIGHTNESS: u8 = 255;

/// 値タグ。
pub const TAG_KANA: u8 = 0;
pub const TAG_KEYS: u8 = 1;

// セクション ID。
const SEC_END: u8 = 0;
const SEC_LAYERS: u8 = 1;
const SEC_COMBO2: u8 = 2;
const SEC_COMBO3: u8 = 3;
const SEC_MODES: u8 = 4;
const SEC_EXTRA: u8 = 5;

/// 1 かな文字列（UTF-8, 最大 [`MAX_KANA_LEN`] バイト）。
pub type KanaStr = String<MAX_KANA_LEN>;
/// キー操作列（(usage, modifiers) ペア, 最大 [`MAX_KEYS`] 個）。
pub type KeysVec = Vec<(u8, u8), MAX_KEYS>;

/// パラメータブロックのシリアライズ長（バージョン直後〜セクション直前）。
const PARAMS_LEN: usize = 11;
const HEADER_LEN: usize = 4 + 1 + PARAMS_LEN;

/// ランタイム差し替え可能なパラメータ（FR-7.1）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Params {
    pub window_ms: u16,
    pub tap_ms: u16,
    pub repeat_delay_ms: u16,
    pub repeat_interval_ms: u16,
    pub led_brightness: u8,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            window_ms: crate::keymap::WINDOW_MS as u16,
            tap_ms: crate::keymap::TAP_MS as u16,
            repeat_delay_ms: crate::engine::REPEAT_DELAY_MS as u16,
            repeat_interval_ms: crate::engine::REPEAT_INTERVAL_MS as u16,
            led_brightness: DEFAULT_LED_BRIGHTNESS,
        }
    }
}

/// 差分の値（かな or キー操作列）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverrideVal {
    Kana(KanaStr),
    Keys(KeysVec),
}

impl OverrideVal {
    /// UTF-8 文字列から Kana を作る（長すぎは Err）。
    pub fn kana(s: &str) -> Result<Self, ()> {
        let mut k = KanaStr::new();
        k.push_str(s).map_err(|_| ())?;
        Ok(OverrideVal::Kana(k))
    }
    /// (usage,mod) ペア列から Keys を作る（多すぎは Err）。
    pub fn keys(pairs: &[(u8, u8)]) -> Result<Self, ()> {
        let mut v = KeysVec::new();
        for &p in pairs {
            v.push(p).map_err(|_| ())?;
        }
        Ok(OverrideVal::Keys(v))
    }

    /// ワイヤ/IR 値のシリアライズ長（tag1 + len1 + payload）。USB プロトコルでも共有。
    pub fn enc_len(&self) -> usize {
        2 + match self {
            OverrideVal::Kana(k) => k.len(),
            OverrideVal::Keys(v) => v.len() * 2,
        }
    }

    /// buf[off..] へ書く。書いた末尾 off を返す（バッファ不足は呼び出し側で事前判定）。
    pub fn encode(&self, buf: &mut [u8], off: usize) -> usize {
        match self {
            OverrideVal::Kana(k) => {
                buf[off] = TAG_KANA;
                let b = k.as_bytes();
                buf[off + 1] = b.len() as u8;
                buf[off + 2..off + 2 + b.len()].copy_from_slice(b);
                off + 2 + b.len()
            }
            OverrideVal::Keys(v) => {
                buf[off] = TAG_KEYS;
                buf[off + 1] = (v.len() * 2) as u8;
                let mut p = off + 2;
                for &(u, m) in v {
                    buf[p] = u;
                    buf[p + 1] = m;
                    p += 2;
                }
                p
            }
        }
    }

    /// buf[off..] から読む。Some((val, new_off)) / 不整合は None。USB プロトコルでも共有。
    pub fn decode(buf: &[u8], off: usize) -> Option<(OverrideVal, usize)> {
        if off + 2 > buf.len() {
            return None;
        }
        let tag = buf[off];
        let len = buf[off + 1] as usize;
        let p = off + 2;
        if p + len > buf.len() {
            return None;
        }
        let val = match tag {
            TAG_KANA => {
                if len > MAX_KANA_LEN {
                    return None;
                }
                let s = core::str::from_utf8(&buf[p..p + len]).ok()?;
                OverrideVal::kana(s).ok()?
            }
            TAG_KEYS => {
                if len % 2 != 0 || len / 2 > MAX_KEYS {
                    return None;
                }
                let mut v = KeysVec::new();
                let mut q = p;
                while q < p + len {
                    v.push((buf[q], buf[q + 1])).ok()?;
                    q += 2;
                }
                OverrideVal::Keys(v)
            }
            _ => return None,
        };
        Some((val, p + len))
    }
}

/// 設定全体（パラメータ＋全カテゴリ差分）。フラッシュ1セクタに収まる小サイズ。
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Config {
    pub params: Params,
    /// layers 差分: (mask, sc, 値)。
    pub layers: Vec<(u8, u8, OverrideVal), MAX_LAYERS>,
    /// combo2 差分: ([sc0,sc1] sorted, 値)。
    pub combo2: Vec<([u8; 2], OverrideVal), MAX_COMBO2>,
    /// combo3 差分: ([sc0,sc1,sc2] sorted, 値)。
    pub combo3: Vec<([u8; 3], OverrideVal), MAX_COMBO3>,
    /// 編集モード差分: (mode, sc, 値)。
    pub modes: Vec<(u8, u8, OverrideVal), MAX_MODES>,
    /// 未使用物理キーの直接割当: (hand, row, col, 値)。薙刀式エンジンを通さずタップ出力（#12 フェーズ3）。
    pub extra: Vec<(u8, u8, u8, OverrideVal), MAX_EXTRA>,
}

fn sort2(a: u8, b: u8) -> [u8; 2] {
    if a <= b {
        [a, b]
    } else {
        [b, a]
    }
}
fn sort3(a: u8, b: u8, c: u8) -> [u8; 3] {
    let mut k = [a, b, c];
    k.sort_unstable();
    k
}

impl Config {
    // --- layers -----------------------------------------------------------
    pub fn find_layer(&self, mask: u8, sc: u8) -> Option<&OverrideVal> {
        self.layers
            .iter()
            .find(|(m, s, _)| *m == mask && *s == sc)
            .map(|(_, _, v)| v)
    }
    pub fn set_layer(&mut self, mask: u8, sc: u8, val: OverrideVal) -> Result<(), ()> {
        if let Some(slot) = self.layers.iter_mut().find(|(m, s, _)| *m == mask && *s == sc) {
            slot.2 = val;
            return Ok(());
        }
        self.layers.push((mask, sc, val)).map_err(|_| ())
    }
    pub fn remove_layer(&mut self, mask: u8, sc: u8) {
        if let Some(i) = self.layers.iter().position(|(m, s, _)| *m == mask && *s == sc) {
            self.layers.swap_remove(i);
        }
    }

    // --- combo2 -----------------------------------------------------------
    pub fn find_combo2(&self, a: u8, b: u8) -> Option<&OverrideVal> {
        let k = sort2(a, b);
        self.combo2.iter().find(|(kk, _)| *kk == k).map(|(_, v)| v)
    }
    pub fn set_combo2(&mut self, a: u8, b: u8, val: OverrideVal) -> Result<(), ()> {
        let k = sort2(a, b);
        if let Some(slot) = self.combo2.iter_mut().find(|(kk, _)| *kk == k) {
            slot.1 = val;
            return Ok(());
        }
        self.combo2.push((k, val)).map_err(|_| ())
    }
    pub fn remove_combo2(&mut self, a: u8, b: u8) {
        let k = sort2(a, b);
        if let Some(i) = self.combo2.iter().position(|(kk, _)| *kk == k) {
            self.combo2.swap_remove(i);
        }
    }

    // --- combo3 -----------------------------------------------------------
    pub fn find_combo3(&self, a: u8, b: u8, c: u8) -> Option<&OverrideVal> {
        let k = sort3(a, b, c);
        self.combo3.iter().find(|(kk, _)| *kk == k).map(|(_, v)| v)
    }
    pub fn set_combo3(&mut self, a: u8, b: u8, c: u8, val: OverrideVal) -> Result<(), ()> {
        let k = sort3(a, b, c);
        if let Some(slot) = self.combo3.iter_mut().find(|(kk, _)| *kk == k) {
            slot.1 = val;
            return Ok(());
        }
        self.combo3.push((k, val)).map_err(|_| ())
    }
    pub fn remove_combo3(&mut self, a: u8, b: u8, c: u8) {
        let k = sort3(a, b, c);
        if let Some(i) = self.combo3.iter().position(|(kk, _)| *kk == k) {
            self.combo3.swap_remove(i);
        }
    }

    // --- modes ------------------------------------------------------------
    pub fn find_mode(&self, mode: u8, sc: u8) -> Option<&OverrideVal> {
        self.modes
            .iter()
            .find(|(md, s, _)| *md == mode && *s == sc)
            .map(|(_, _, v)| v)
    }
    pub fn set_mode(&mut self, mode: u8, sc: u8, val: OverrideVal) -> Result<(), ()> {
        if let Some(slot) = self.modes.iter_mut().find(|(md, s, _)| *md == mode && *s == sc) {
            slot.2 = val;
            return Ok(());
        }
        self.modes.push((mode, sc, val)).map_err(|_| ())
    }
    pub fn remove_mode(&mut self, mode: u8, sc: u8) {
        if let Some(i) = self.modes.iter().position(|(md, s, _)| *md == mode && *s == sc) {
            self.modes.swap_remove(i);
        }
    }

    // --- extra（直接キー）-------------------------------------------------
    pub fn find_extra(&self, hand: u8, row: u8, col: u8) -> Option<&OverrideVal> {
        self.extra
            .iter()
            .find(|(h, r, c, _)| *h == hand && *r == row && *c == col)
            .map(|(_, _, _, v)| v)
    }
    pub fn set_extra(&mut self, hand: u8, row: u8, col: u8, val: OverrideVal) -> Result<(), ()> {
        if let Some(slot) = self
            .extra
            .iter_mut()
            .find(|(h, r, c, _)| *h == hand && *r == row && *c == col)
        {
            slot.3 = val;
            return Ok(());
        }
        self.extra.push((hand, row, col, val)).map_err(|_| ())
    }
    pub fn remove_extra(&mut self, hand: u8, row: u8, col: u8) {
        if let Some(i) = self
            .extra
            .iter()
            .position(|(h, r, c, _)| *h == hand && *r == row && *c == col)
        {
            self.extra.swap_remove(i);
        }
    }
}

/// CRC-16/CCITT-FALSE（poly=0x1021, init=0xFFFF）。改ざん/破損検出用。
fn crc16(data: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &b in data {
        crc ^= (b as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn put_u16(buf: &mut [u8], off: usize, v: u16) {
    buf[off] = (v & 0xff) as u8;
    buf[off + 1] = (v >> 8) as u8;
}
fn get_u16(buf: &[u8], off: usize) -> u16 {
    (buf[off] as u16) | ((buf[off + 1] as u16) << 8)
}

/// Config を IR バイナリへ書き出す。書き込んだ総バイト数を返す。バッファ不足は None。
pub fn serialize(cfg: &Config, buf: &mut [u8]) -> Option<usize> {
    // 必要長を見積もる。
    let mut need = HEADER_LEN;
    need += 3 + cfg.layers.iter().map(|(_, _, v)| 2 + v.enc_len()).sum::<usize>(); // id+count(2)+entries
    need += 3 + cfg.combo2.iter().map(|(_, v)| 2 + v.enc_len()).sum::<usize>();
    need += 3 + cfg.combo3.iter().map(|(_, v)| 3 + v.enc_len()).sum::<usize>();
    need += 3 + cfg.modes.iter().map(|(_, _, v)| 2 + v.enc_len()).sum::<usize>();
    need += 3 + cfg.extra.iter().map(|(_, _, _, v)| 3 + v.enc_len()).sum::<usize>();
    need += 1 + 2; // SEC_END + crc16
    if buf.len() < need {
        return None;
    }

    buf[0..4].copy_from_slice(&MAGIC);
    buf[4] = VERSION;
    let p = &cfg.params;
    put_u16(buf, 5, p.window_ms);
    put_u16(buf, 7, p.tap_ms);
    put_u16(buf, 9, p.repeat_delay_ms);
    put_u16(buf, 11, p.repeat_interval_ms);
    buf[13] = p.led_brightness;
    buf[14] = 0;
    buf[15] = 0;

    let mut off = HEADER_LEN;
    // LAYERS
    buf[off] = SEC_LAYERS;
    put_u16(buf, off + 1, cfg.layers.len() as u16);
    off += 3;
    for (mask, sc, v) in &cfg.layers {
        buf[off] = *mask;
        buf[off + 1] = *sc;
        off = v.encode(buf, off + 2);
    }
    // COMBO2
    buf[off] = SEC_COMBO2;
    put_u16(buf, off + 1, cfg.combo2.len() as u16);
    off += 3;
    for (k, v) in &cfg.combo2 {
        buf[off] = k[0];
        buf[off + 1] = k[1];
        off = v.encode(buf, off + 2);
    }
    // COMBO3
    buf[off] = SEC_COMBO3;
    put_u16(buf, off + 1, cfg.combo3.len() as u16);
    off += 3;
    for (k, v) in &cfg.combo3 {
        buf[off] = k[0];
        buf[off + 1] = k[1];
        buf[off + 2] = k[2];
        off = v.encode(buf, off + 3);
    }
    // MODES
    buf[off] = SEC_MODES;
    put_u16(buf, off + 1, cfg.modes.len() as u16);
    off += 3;
    for (mode, sc, v) in &cfg.modes {
        buf[off] = *mode;
        buf[off + 1] = *sc;
        off = v.encode(buf, off + 2);
    }
    // EXTRA
    buf[off] = SEC_EXTRA;
    put_u16(buf, off + 1, cfg.extra.len() as u16);
    off += 3;
    for (hand, row, col, v) in &cfg.extra {
        buf[off] = *hand;
        buf[off + 1] = *row;
        buf[off + 2] = *col;
        off = v.encode(buf, off + 3);
    }
    // END + CRC
    buf[off] = SEC_END;
    off += 1;
    let crc = crc16(&buf[0..off]);
    put_u16(buf, off, crc);
    off += 2;
    Some(off)
}

/// IR バイナリを Config へ復元する。マジック/バージョン/CRC/境界の不整合は None。
pub fn parse(buf: &[u8]) -> Option<Config> {
    if buf.len() < HEADER_LEN + 1 + 2 {
        return None;
    }
    if buf[0..4] != MAGIC || buf[4] != VERSION {
        return None;
    }
    let mut cfg = Config {
        params: Params {
            window_ms: get_u16(buf, 5),
            tap_ms: get_u16(buf, 7),
            repeat_delay_ms: get_u16(buf, 9),
            repeat_interval_ms: get_u16(buf, 11),
            led_brightness: buf[13],
        },
        ..Default::default()
    };

    let mut off = HEADER_LEN;
    loop {
        if off >= buf.len() {
            return None;
        }
        let sec = buf[off];
        if sec == SEC_END {
            off += 1;
            break;
        }
        if off + 3 > buf.len() {
            return None;
        }
        let count = get_u16(buf, off + 1) as usize;
        off += 3;
        for _ in 0..count {
            match sec {
                SEC_LAYERS => {
                    if off + 2 > buf.len() {
                        return None;
                    }
                    let (mask, sc) = (buf[off], buf[off + 1]);
                    let (v, no) = OverrideVal::decode(buf, off + 2)?;
                    off = no;
                    cfg.set_layer(mask, sc, v).ok()?;
                }
                SEC_COMBO2 => {
                    if off + 2 > buf.len() {
                        return None;
                    }
                    let (a, b) = (buf[off], buf[off + 1]);
                    let (v, no) = OverrideVal::decode(buf, off + 2)?;
                    off = no;
                    cfg.set_combo2(a, b, v).ok()?;
                }
                SEC_COMBO3 => {
                    if off + 3 > buf.len() {
                        return None;
                    }
                    let (a, b, c) = (buf[off], buf[off + 1], buf[off + 2]);
                    let (v, no) = OverrideVal::decode(buf, off + 3)?;
                    off = no;
                    cfg.set_combo3(a, b, c, v).ok()?;
                }
                SEC_MODES => {
                    if off + 2 > buf.len() {
                        return None;
                    }
                    let (mode, sc) = (buf[off], buf[off + 1]);
                    let (v, no) = OverrideVal::decode(buf, off + 2)?;
                    off = no;
                    cfg.set_mode(mode, sc, v).ok()?;
                }
                SEC_EXTRA => {
                    if off + 3 > buf.len() {
                        return None;
                    }
                    let (hand, row, col) = (buf[off], buf[off + 1], buf[off + 2]);
                    let (v, no) = OverrideVal::decode(buf, off + 3)?;
                    off = no;
                    cfg.set_extra(hand, row, col, v).ok()?;
                }
                _ => return None, // 未知セクション
            }
        }
    }

    if off + 2 > buf.len() {
        return None;
    }
    let want = get_u16(buf, off);
    if crc16(&buf[0..off]) != want {
        return None;
    }
    Some(cfg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_params_match_codegen() {
        let p = Params::default();
        assert_eq!(p.window_ms, crate::keymap::WINDOW_MS as u16);
        assert_eq!(p.tap_ms, crate::keymap::TAP_MS as u16);
        assert_eq!(p.led_brightness, DEFAULT_LED_BRIGHTNESS);
    }

    #[test]
    fn roundtrip_empty() {
        let cfg = Config::default();
        let mut buf = [0u8; 256];
        let n = serialize(&cfg, &mut buf).unwrap();
        assert_eq!(parse(&buf[0..n]).unwrap(), cfg);
    }

    #[test]
    fn roundtrip_all_sections() {
        let mut cfg = Config::default();
        cfg.params.window_ms = 55;
        cfg.params.led_brightness = 80;
        cfg.set_layer(0, 0x11, OverrideVal::kana("め").unwrap()).unwrap(); // 単打
        cfg.set_layer(0x01, 0x16, OverrideVal::kana("ＸＸ").unwrap()).unwrap(); // CENTER面
        cfg.set_layer(0x01, 0x2f, OverrideVal::keys(&[(0x28, 0)]).unwrap()).unwrap(); // keys面
        cfg.set_combo2(0x11, 0x23, OverrideVal::kana("きゃ").unwrap()).unwrap();
        cfg.set_combo3(0x32, 0x12, 0x25, OverrideVal::kana("てぃ").unwrap()).unwrap();
        cfg.set_mode(1, 0x24, OverrideVal::keys(&[(0x51, 0)]).unwrap()).unwrap();
        let mut buf = [0u8; 1024];
        let n = serialize(&cfg, &mut buf).unwrap();
        let back = parse(&buf[0..n]).unwrap();
        assert_eq!(back, cfg);
        assert_eq!(back.find_layer(0x01, 0x16), Some(&OverrideVal::kana("ＸＸ").unwrap()));
        assert_eq!(back.find_combo2(0x23, 0x11), back.find_combo2(0x11, 0x23)); // 順不同
        assert_eq!(back.find_mode(1, 0x24), Some(&OverrideVal::keys(&[(0x51, 0)]).unwrap()));
    }

    #[test]
    fn set_replace_and_remove() {
        let mut cfg = Config::default();
        cfg.set_layer(0, 0x11, OverrideVal::kana("め").unwrap()).unwrap();
        cfg.set_layer(0, 0x11, OverrideVal::kana("も").unwrap()).unwrap(); // 置換
        assert_eq!(cfg.layers.len(), 1);
        assert_eq!(cfg.find_layer(0, 0x11), Some(&OverrideVal::kana("も").unwrap()));
        cfg.remove_layer(0, 0x11);
        assert_eq!(cfg.find_layer(0, 0x11), None);
        assert_eq!(cfg.layers.len(), 0);
    }

    #[test]
    fn parse_rejects_bad_magic() {
        let mut buf = [0u8; 32];
        buf[0..4].copy_from_slice(b"XXXX");
        assert!(parse(&buf).is_none());
    }

    #[test]
    fn parse_rejects_v1() {
        // v1 マジックは同じだがバージョン違い → None（互換読込しない）。
        let mut buf = [0u8; 32];
        buf[0..4].copy_from_slice(&MAGIC);
        buf[4] = 1;
        assert!(parse(&buf).is_none());
    }

    #[test]
    fn parse_rejects_corrupt_crc() {
        let cfg = Config::default();
        let mut buf = [0u8; 256];
        let n = serialize(&cfg, &mut buf).unwrap();
        buf[5] ^= 0xff;
        assert!(parse(&buf[0..n]).is_none());
    }

    #[test]
    fn serialize_reports_buffer_too_small() {
        let mut cfg = Config::default();
        cfg.set_layer(0, 0x11, OverrideVal::kana("め").unwrap()).unwrap();
        let mut buf = [0u8; 8];
        assert!(serialize(&cfg, &mut buf).is_none());
    }

    #[test]
    fn roundtrip_extra() {
        let mut cfg = Config::default();
        cfg.set_extra(0, 0, 0, OverrideVal::keys(&[(0x3a, 0)]).unwrap()).unwrap(); // 左 r0c0 = F1
        cfg.set_extra(1, 0, 6, OverrideVal::keys(&[(0x4c, 0)]).unwrap()).unwrap(); // 右 r0c6 = Del
        cfg.set_extra(0, 1, 0, OverrideVal::kana("マ").unwrap()).unwrap(); // Macro位置にかな
        let mut buf = [0u8; 512];
        let n = serialize(&cfg, &mut buf).unwrap();
        let back = parse(&buf[0..n]).unwrap();
        assert_eq!(back, cfg);
        assert_eq!(back.find_extra(1, 0, 6), Some(&OverrideVal::keys(&[(0x4c, 0)]).unwrap()));
        cfg.remove_extra(1, 0, 6);
        assert_eq!(cfg.find_extra(1, 0, 6), None);
    }

    #[test]
    fn keys_roundtrip_multi() {
        let mut cfg = Config::default();
        cfg.set_mode(2, 0x27, OverrideVal::keys(&[(0x4f, 0x02); 12]).unwrap()).unwrap(); // 12キー上限
        let mut buf = [0u8; 256];
        let n = serialize(&cfg, &mut buf).unwrap();
        assert_eq!(parse(&buf[0..n]).unwrap(), cfg);
    }
}
