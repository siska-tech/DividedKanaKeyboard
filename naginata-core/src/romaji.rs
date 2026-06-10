//! かな → ローマ字 変換（要件 FR-4.1 / 付録B）。
//!
//! 方針: 打鍵数最小の訓令式ベース（si/ti/tu/hu/zi…）、撥音は常に "nn"。
//! 出力は ASCII 小文字＋'-' のみ（標準ローマ字IMEで確定可能）。
//!
//! 注意: 拗音(きゃ等)は「い段かな＋小書きゃゅょ」の2文字で来る前提で結合する。
//! 本テーブルは PoC として主要モーラを網羅。IME差異の最終調整は別途プロファイルで。

use heapless::String;

/// かな1モーラぶんの基本ローマ字（拗音結合・促音・撥音は呼び出し側で処理）。
fn base(c: char) -> Option<&'static str> {
    let s = match c {
        // あ行
        'あ' => "a", 'い' => "i", 'う' => "u", 'え' => "e", 'お' => "o",
        // か行
        'か' => "ka", 'き' => "ki", 'く' => "ku", 'け' => "ke", 'こ' => "ko",
        'が' => "ga", 'ぎ' => "gi", 'ぐ' => "gu", 'げ' => "ge", 'ご' => "go",
        // さ行
        'さ' => "sa", 'し' => "si", 'す' => "su", 'せ' => "se", 'そ' => "so",
        'ざ' => "za", 'じ' => "zi", 'ず' => "zu", 'ぜ' => "ze", 'ぞ' => "zo",
        // た行
        'た' => "ta", 'ち' => "ti", 'つ' => "tu", 'て' => "te", 'と' => "to",
        'だ' => "da", 'ぢ' => "di", 'づ' => "du", 'で' => "de", 'ど' => "do",
        // な行
        'な' => "na", 'に' => "ni", 'ぬ' => "nu", 'ね' => "ne", 'の' => "no",
        // は行
        'は' => "ha", 'ひ' => "hi", 'ふ' => "hu", 'へ' => "he", 'ほ' => "ho",
        'ば' => "ba", 'び' => "bi", 'ぶ' => "bu", 'べ' => "be", 'ぼ' => "bo",
        'ぱ' => "pa", 'ぴ' => "pi", 'ぷ' => "pu", 'ぺ' => "pe", 'ぽ' => "po",
        // ま行
        'ま' => "ma", 'み' => "mi", 'む' => "mu", 'め' => "me", 'も' => "mo",
        // や行
        'や' => "ya", 'ゆ' => "yu", 'よ' => "yo",
        // ら行
        'ら' => "ra", 'り' => "ri", 'る' => "ru", 'れ' => "re", 'ろ' => "ro",
        // わ行
        'わ' => "wa", 'を' => "wo", 'ん' => "nn",
        // ヴ（カタカナ／ひらがな）
        'ゔ' => "vu", 'ヴ' => "vu",
        // 長音
        'ー' => "-",
        // 小書き単独（フォールバック）
        'ぁ' => "xa", 'ぃ' => "xi", 'ぅ' => "xu", 'ぇ' => "xe", 'ぉ' => "xo",
        'ゃ' => "xya", 'ゅ' => "xyu", 'ょ' => "xyo", 'ゎ' => "xwa", 'っ' => "xtu",
        _ => return None,
    };
    Some(s)
}

/// い段かな（拗音の頭になれる）の子音部を返す。例: き→"k", し→"sy", ち→"ty"。
/// 訓令式の拗音は「子音 + y + 母音」: きゃ=kya, しゃ=sya, ちゃ=tya。
fn youon_consonant(c: char) -> Option<&'static str> {
    let s = match c {
        'き' => "k", 'ぎ' => "g",
        'し' => "s", 'じ' => "z",
        'ち' => "t", 'ぢ' => "d",
        'に' => "n", 'ひ' => "h", 'び' => "b", 'ぴ' => "p",
        'み' => "m", 'り' => "r",
        _ => return None,
    };
    Some(s)
}

fn small_youon_vowel(c: char) -> Option<&'static str> {
    match c {
        'ゃ' => Some("ya"),
        'ゅ' => Some("yu"),
        'ょ' => Some("yo"),
        _ => None,
    }
}

/// かな文字列をローマ字へ変換する。バッファ上限超過分は切り捨て（PoC）。
pub fn kana_to_romaji<const N: usize>(kana: &str) -> String<N> {
    let mut out: String<N> = String::new();
    let mut chars = kana.chars().peekable();

    while let Some(c) = chars.next() {
        // 促音っ: 次のモーラの頭子音を重ねる。次が無い/重ねられない場合は "xtu"。
        if c == 'っ' {
            if let Some(&next) = chars.peek() {
                if let Some(r) = base(next) {
                    if let Some(first) = r.chars().next() {
                        if first.is_ascii_alphabetic() && first != 'a' && first != 'i'
                            && first != 'u' && first != 'e' && first != 'o'
                        {
                            let _ = push_char(&mut out, first);
                            continue;
                        }
                    }
                }
            }
            push_str(&mut out, "xtu");
            continue;
        }

        // 拗音結合: い段かな + 小書きゃゅょ
        if let Some(cons) = youon_consonant(c) {
            if let Some(&next) = chars.peek() {
                if let Some(yv) = small_youon_vowel(next) {
                    push_str(&mut out, cons);
                    push_str(&mut out, yv);
                    chars.next();
                    continue;
                }
            }
        }

        if let Some(r) = base(c) {
            push_str(&mut out, r);
        }
        // 未知の文字は無視（PoC）。本実装ではログ/フォールバック。
    }
    out
}

fn push_str<const N: usize>(out: &mut String<N>, s: &str) {
    for ch in s.chars() {
        let _ = push_char(out, ch);
    }
}

fn push_char<const N: usize>(out: &mut String<N>, c: char) -> Result<(), ()> {
    out.push(c).map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(s: &str) -> String<32> {
        kana_to_romaji::<32>(s)
    }

    #[test]
    fn basic_gojuon() {
        assert_eq!(r("かう").as_str(), "kau");
        assert_eq!(r("し").as_str(), "si");
        assert_eq!(r("て").as_str(), "te");
    }

    #[test]
    fn dakuten() {
        assert_eq!(r("が").as_str(), "ga");
        assert_eq!(r("づ").as_str(), "du");
    }

    #[test]
    fn hatsuon_is_always_double_n() {
        assert_eq!(r("ん").as_str(), "nn");
        assert_eq!(r("かんい").as_str(), "kanni");
    }

    #[test]
    fn youon() {
        assert_eq!(r("きゃ").as_str(), "kya");
        assert_eq!(r("しゃ").as_str(), "sya");
        assert_eq!(r("ちょ").as_str(), "tyo");
    }

    #[test]
    fn sokuon_doubles_consonant() {
        assert_eq!(r("かっこ").as_str(), "kakko");
        assert_eq!(r("っ").as_str(), "xtu"); // 単独
    }

    #[test]
    fn chouon() {
        assert_eq!(r("かー").as_str(), "ka-");
    }
}
