# [#02] 配列データをレイヤYAML＋codegenへ移行

- **優先度:** P1
- **状態:** ✅ Done（2026-06-09）
- **依存:** なし（#03 の前提）
- **関連設計:** 詳細設計書 B章 / 同 A.5,A.7

## 目的
現行の flat な `single`/`combo` スキーマを、薙刀式シフト体系を表現できる
**レイヤモデル（shift集合 → base sc → Action）** へ移行する。engine v2(#03) の前提。

## スコープ（やること）
- [x] `layout/naginata.yaml` を新スキーマへ（詳細設計 B.1）: `shifts` / `layers(when,kana,keys)`
- [x] `build.rs` を改修：
  - [x] `when`(シフト名集合) → `ShiftMask`(u8 bitOR) をキー化
  - [x] レイヤ群 → `LAYERS: phf::Map<u16 (mask<<8|sc), Action>`（flat化。nested phf回避）
  - [x] `keys` のシンボル（"LANG1","Enter" 等）→ `KeyPress` 列へ build時変換（symbol_to_keys 表）
  - [x] 旧 `pack`/`COMBO` を廃止
- [x] `keymap.rs`：`single_kana`/`combo_kana` を `layer_lookup(mask, sc) -> Action`／`shift_mask_of` に置換
- [x] `Action` enum（Kana/Keys/None）を core に追加
- [x] `hid::KeyPress` を `{usage, modifiers}` 化（Ctrl/Shift等に対応）＋ firmware 追従

## 実装結果メモ
- `LAYERS` は `phf::Map<u16, Action>`（キー=`(mask<<8)|sc`）の**フラット1表**。nested phf を避け生成を単純化。
- build.rs に **(mask,sc) 重複検出** を実装（衝突時 panic）。
- engine は窓+候補集合を「シフトmask×ベース」へ分解して `layer_lookup` で解決する形へ更新。
  → 単打/センター/前置/後置/二重（窓内同時）が成立。**連続シフト・tap/hold厳密判定は #03**。
- テスト 16件パス（layer解決・Action::Keys・センター/前置/後置含む）。

## やらないこと
- engine 本体のシフト判定ロジック（#03 で実装）
- v15 全データの転記（#04）。本Issueは**少数のサンプル**でスキーマ/codegenを成立させる

## 受入条件（Done の定義）
- [x] `cargo test -p naginata-core` がグリーン（既存テストは新APIに追従して更新）
- [x] サンプルYAMLから `LAYERS` が生成され、`layer_lookup` で単打面/シフト面が引ける
- [x] `keys`（例: IME ON の LANG1）が `Action::Keys` として取り出せるユニットテスト
- [x] MCU上で実行時パースが無いこと（生成は build.rs のみ）を維持

## 実装メモ / 参照
- ShiftMask は u8（bit0-5使用、最大8シフト）
- `LayerTable { kana: phf::Map<u8,&str>, keys: phf::Map<u8,&[KeyPress]> }`
- matrix_left/right 表は現行踏襲（変更不要）
