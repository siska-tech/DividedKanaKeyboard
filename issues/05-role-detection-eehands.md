# [#05] 役割判定（USB列挙）＋EE_HANDS

- **優先度:** P2
- **状態:** ✅ Done（2026-06-10 実機確認：右をUSBに挿しても正しく打鍵）
- **依存:** #01
- **関連設計:** 詳細設計書 D章 / アーキ §3 / 要件 FR-2.5

## 目的
左右同一FWのまま、**USB列挙でマスタ自動判定**し、**EE_HANDSで手番(左右)**を確定する。

## スコープ（やること）
- [x] 役割判定：USB `Handler::configured` → `USB_CONFIGURED` でマスタ自動化（#06で実装）
- [x] USB Suspend/Reset で false へ（再判定）
- [x] EE_HANDS フラッシュR/W（D.2）：末尾4KBセクタ、magic"NGHD"+hand（handedness.rs）
- [x] `handedness.rs` の `read_hand`/`write_hand` を embassy-rp `Flash::new_blocking` で実装
- [x] プロビジョニング手順（D.3）：**起動時2キー**（スペース＋外側上=Left / スペース＋内側上=Right）
- [x] 未設定時の既定（D.4）：暫定 Left（要プロビジョニング）

## 実装結果（2026-06-10）
- `handedness.rs`: 末尾4KBセクタ(0x1FF000)に `"NGHD"+'L'/'R'` を保存/読込。
  書込みは1ページ(256B)消去→書込み。`blocking_erase/write` は内部 `in_ram`
  （core1停止＋critical_section＋DMA待ち）でXIP無効化を安全処理。
- `matrix.rs`: 起動時用の `is_held(row,col)`（デバウンス無し即時読み）追加。
- `main.rs`: 起動時にプロビジョニング判定→ `read_hand` で `local_hand` を決め、
  scan_task/uart_rx_task へ渡す（`LOCAL_HAND` const 廃止）。これで**左右どちらをUSBに挿してもOK**。

## 実機確認（2026-06-10 ✅）
- [x] プロビジョニング: スペース＋外側上キー押しながら起動→Left, ＋内側上→Right が書込まれる
- [x] 再起動後も手番が保持される（フラッシュ永続）
- [x] 左右入れ替えて挿しても正しくマップ（左USB=Left基点 / 右USB=Right基点）

## やらないこと
- UART宣言の送受信そのもの（#06 と協調。本Issueは役割ロジック側）
- USB-CDC方式のプロビジョニング（任意・後回し可）

## 受入条件（Done の定義）
- [x] USBを挿した側がMASTER、他方がSLAVEになる（実機, #06で確認済）
- [~] EE_HANDS read/write 往復テスト：実フラッシュ依存のためユニットテスト無し。実機で検証する
- [x] 役割判定：列挙→MASTER（#06）。UART宣言方式は不採用（USB列挙で代替）
- [ ] プロビジョニング後、再起動で手番が保持される（**要実機**）

## 実装メモ / 参照
- フラッシュ書込みは XIP 停止のクリティカル区間で（embassy-rp flash 注意点）
- `memory.x` で末尾セクタを予約する案も（handedness.rs / memory.x コメント）
- 役割と手番は独立2軸（アーキ §3）。混同しない
