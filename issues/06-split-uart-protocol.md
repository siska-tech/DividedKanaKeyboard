# [#06] 分割UARTプロトコル実装

- **優先度:** P2
- **状態:** ✅ Done（2026-06-09 実機で両手フル動作確認）
- **依存:** #01
- **関連設計:** 詳細設計書 C章 / 要件 FR-2

## 目的
左右半身を UART0 で連結し、**スレーブのキーイベントをマスタへ低レイテンシ転送**、
マスタ側で自半身＋相方を統合してエンジンへ渡す。

## スコープ（やること）
- [x] `split.rs` のフレーム実装（C.2）：KeyEvent / Status、SYNCビット（先頭bit7）
- [x] 送信：スキャン変化を KeyEvent として送出（uart_tx_task）
- [x] 受信：マスタ時、`Hand::opposite()` の表で sc 化し同一 engine へ合流（アーキ §9.3）
- [x] 再同期ループ（C.3）：bit7=1 でフレーム頭捕捉、失敗時読み捨て
- [~] Handshake（C.4）：**USB列挙(`USB_CONFIGURED`)で役割判定に変更**。MASTER/SLAVE宣言PINGは不採用。
      代わりに LED用 Status フレームを追加（マスタ→スレーブ）。
- [x] embassy-rp UART（`BufferedUart`=割込み）でノンブロッキング送受信
- [~] ボーレート：**115200**/8N1（確実性優先。921600は将来）

## やらないこと
- 役割判定ロジック本体（#05）。本Issueは伝送路と統合配線

## 受入条件（Done の定義）
- [x] フレーム往復テスト（split.rs `key_roundtrip`/`status_roundtrip`）
- [~] 同期外れからの再同期：decodeで bit7 無しは None→読み捨て（専用テスト未。実機で問題なし）
- [x] 実機：両手で同時押し（濁音 が=f+j 等）が**左右跨ぎで成立** ✅2026-06-09
- [~] TRRS抜き差し後の自動復帰：ステートレス再同期で設計上OK（明示検証は未）
- [x] スレーブ単独（マスタ無し）で誤出力しない（スレーブは engine 非実行）

## 実装メモ / 参照
- 片道遅延 ≦2ms 目標（FR-2.2）。1イベント2byte＝帯域余裕
- 左右跨ぎ同時押しは「時刻整合」が要点：両半身の now を揃えるか、受信時刻で窓判定するか要検討
- TX/RXはコネクタでクロス済み（左右同一コード）

## 実装結果（2026-06-09）
- `BufferedUart`(UART0, 115200, 8N1) を split して TX/RX タスク化。`embedded-io-async 0.7`。
- タスク構成: usb / scan / uart_tx / uart_rx / engine の5本＋チャネル(REPORTS/ENGINE_IN/UART_TX_Q)。
- **時刻整合**: マスタの engine_task が受信/ローカル両方を「マスタのnow」でタイムスタンプ
  （UART遅延≪窓40ms なので受信時刻で同時押し判定して問題なし）。
- 役割: USB `Handler::configured` で `USB_CONFIGURED` を立て、マスタのみ engine 実行。
- 再同期: 先頭バイト bit7=1 を待つ方式（split.rs decode）。
- **MVP制約**: 手番=Left固定 → **左をUSBに挿す**。右はスレーブ。両側挿せるのは #05(EE_HANDS)後。
- thumbv6m: `static_cell` は atomic CAS 不可で不採用、UARTバッファは `static mut`+`addr_of_mut`。

## 残（実機）
- [x] 両半身に書込み＋TRRS接続＋左USBで、両手フルにかなが打てること ✅2026-06-09
- [x] 左右跨ぎ同時押し（濁音=逆手シフト）が成立すること ✅2026-06-09（が/ご等を確認）

## 凡例: [x]=完了 / [~]=設計から変更 or 部分対応（本文参照）/ [ ]=未
