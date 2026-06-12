# Issues — タスク分割

今後の実装タスク。粒度は「1 Issue = 1 まとまった成果物」。
設計の根拠は [要件定義.md](../Documents/要件定義.md) / [アーキテクチャ設計書.md](../Documents/アーキテクチャ設計書.md) / [詳細設計書.md](../Documents/詳細設計書.md)。

## マイルストーン（2026-06-09）

**実機で薙刀式キーボードとして実用的に日本語が打てる状態に到達。**
両半身（TRRS連結・左USBマスタ）で 単打／センターシフト／濁音／半濁音／小書き／IME ON-OFF が動作。
LEDで役割＋シフト状態を表示。コア（naginata-core）24テスト緑。

## ボード

| #                                         | タイトル                                              | 優先 | 依存     | 状態                                               |
| ----------------------------------------- | ----------------------------------------------------- | ---- | -------- | -------------------------------------------------- |
| [01](01-firmware-build-usb-hid.md)        | firmware を thumbv6m で実ビルド＆USB-HID列挙          | P1   | –        | ✅ Done（実機確認）                                |
| [02](02-layout-schema-layer-migration.md) | 配列データをレイヤYAML＋codegenへ移行                 | P1   | –        | ✅ Done                                            |
| [03](03-engine-v2-shift-system.md)        | engine：薙刀式シフト体系（→v3 順序非依存）            | P1   | 02       | ✅ Done                                            |
| [04](04-v15-layout-transcription.md)      | 薙刀式v15配列の転記（実用データ）                     | P2   | 02       | ✅ Done（句読点まで。拗音1打鍵/外来音/編集は #10） |
| [05](05-role-detection-eehands.md)        | 役割判定（USB列挙）＋EE_HANDS                         | P2   | 01       | ✅ Done（実機確認）                                |
| [06](06-split-uart-protocol.md)           | 分割UARTプロトコル実装                                | P2   | 01       | ✅ Done（実機・両手動作）                          |
| [07](07-ble-hid.md)                       | BLE-HID（cyw43 + trouble-host）                       | P3   | 01,05,06 | ✅ 主要実機検証済み（Win/Android/Linux再接続安定。残: 省電力） |
| [08](08-txt-importer.md)                  | 薙刀式.txt 直読インポータ（上流追従）                 | P3   | 02       | 🗄 Backlog                                        |
| [09](09-led-status.md)                    | LEDステータス表示（WS2812B）                          | P3   | 01,06    | ✅ Done（実機）                                    |
| [10](10-combo-engine-extension.md)        | セット型コンボ・エンジン拡張（拗音1打鍵/外来音/編集） | P2   | 03,04    | ✅ Done（左手マクロのみ任意残）                    |
| [11](11-key-repeat.md)                    | キーリピート機能（長押しでBS/←/→等）                  | P2   | 03,10    | ✅ Done（実機）                                    |
| [12](12-pc-keymap-customizer.md)          | PCからのキーマップ/設定カスタマイズ（USB/設定ツール） | P3   | 01,02,10 | ✅ Done（fw 0.3 で全物理キー/形状読み出しまで実装。実機検証は UF2 書込後） |
| [13](13-modifier-hold-layers.md)          | 修飾キー長押し保持・モメンタリレイヤ（#12 スコープC） | P3   | 12       | ✅ Done（fw 0.3。mod-tap のみ任意残）              |

## 残り

**薙刀式の基本入力と主要BLE接続は実機で動作。** 残りは省力化・操作性／カスタマイズ向上。

```
07（BLE）          … 主要実機検証済み。残: 省電力・追加端末確認・複数ホスト切替
08（.txtインポータ）… 上流追従の省力化（任意・Backlog）
任意: 編集モードの左手マクロ（『』等の挿入）・固有名詞ショートカット（#10内）
任意: mod-tap（タップ=キー/長押し=修飾。#13 の残し）
```

## 状態凡例
🔲 Todo / 🟡 In Progress / ✅ Done / ⏸ Blocked / 🗄 Backlog

## Issue テンプレート
新規は [_template.md](_template.md) をコピーして作成。
