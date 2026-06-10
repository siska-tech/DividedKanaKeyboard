# DividedKanaKeyboard — 薙刀式 分割キーボード ファーム

左右2分割・各 Raspberry Pi Pico W、TRRS(UART)連結の自作キーボード。
入力方式 **薙刀式 v15**（同時打鍵かな入力）を**ファーム側で実装**し、ローマ字シーケンスで出力するので、
ホストに専用ソフト不要（標準のローマ字IMEで動く）。

> **状態（2026-06）**: 実機で実用機能フル動作。USB-HID／分割両手入力／全かな（濁音・半濁音・小書き・
> 拗音1打鍵・外来音3キー）／句読点・Enter・IME ON-OFF／編集モード／LED表示／EE_HANDS。
> 残りは **BLE無線化(#07)** のみ。コア38テスト緑。

- 要件: [要件定義.md](Documents/要件定義.md) ／ 設計: [アーキテクチャ設計書.md](Documents/アーキテクチャ設計書.md) ・ [詳細設計書.md](Documents/詳細設計書.md)
- タスク/進捗: [issues/](issues/README.md)

## 構成

```
DividedKanaKeyboard/
├─ naginata-core/         ハード非依存コアロジック（no_std / ホストテスト 38件）
│  ├─ layout/naginata.yaml  薙刀式v15 配列定義（ビルド時に phf へコード生成。実行時パース無し）
│  ├─ build.rs             YAML → phf テーブル コード生成（LAYERS/COMBO2/COMBO3/MODE_*/MATRIX/SINGLE_TAP_SCS）
│  └─ src/
│     ├─ engine.rs         同時打鍵判定エンジン v3（シフト体系＋2/3キーコンボ＋編集モード, 大岡式遅延確定, 全カテゴリ Overlay #12）
│     ├─ romaji.rs         かな→ローマ字（訓令式ベース, 撥音=nn, 促音/拗音/長音）
│     ├─ hid.rs            ASCII/シンボル→USB HID（Ctrl/Shift対応）
│     ├─ config.rs         設定の中間表現(IR v2)＋serialize/parse（#12 PCカスタマイズの永続フォーマット）
│     └─ keymap.rs         生成テーブルのアクセサ（matrix→sc, layer/combo/mode lookup, Hand, ランタイム Overlay #12）
├─ firmware/              embassy(RP2040/Pico W)ファーム
│  └─ src/
│     ├─ main.rs           タスク統合（usb/scan/uart_tx/uart_rx/engine/led）＋役割判定＋EE_HANDS＋設定適用
│     ├─ matrix.rs         14col×6row COL2ROWスキャン（§2.1）
│     ├─ split.rs          左右UARTプロトコル（KeyEvent/Status, 自己同期）
│     ├─ handedness.rs     EE_HANDS（フラッシュに手番L/R保存）
│     ├─ config_store.rs   設定IRのフラッシュ永続化（#12, 末尾-2番目セクタ）
│     ├─ usb_config.rs     PC↔端末 設定チャネル（#12, vendor HID, INFO/READ/WRITE/COMMIT/RESET/REBOOT）
│     └─ usb_hid.rs        HID キーボードレポート
├─ issues/                タスク分割・進捗
└─ Documents/             設計ドキュメント（要件/アーキ/詳細）＋ 回路図 / BOM
```

> **PC設定ツール（#12, WebHID）** は別リポジトリ [siska-tech.github.io](https://github.com/siska-tech/siska-tech.github.io) の
> `naginata-config/` で公開（→ <https://siska-tech.github.io/naginata-config/>）。本リポジトリには含めない。
> 薙刀式v15 配列定義（大岡氏作成）は `Documents/Reference/`（**非公開・git管理外**）。

## ビルド & テスト

### コア（ホストで即実行）
```powershell
cargo test --manifest-path naginata-core/Cargo.toml   # 38 tests
```

### ファーム（RP2040, 要 Pico 実機で書込み）
```powershell
rustup target add thumbv6m-none-eabi
cargo install elf2uf2-rs
cd firmware                         # !! firmware内から（.cargo/config.toml の target を効かせる）
cargo build --release
elf2uf2-rs target/thumbv6m-none-eabi/release/naginata-firmware naginata-firmware.uf2
# Pico を BOOTSEL 押しながら接続 → naginata-firmware.uf2 をドラッグ
```
versions: Rust 1.96 / embassy-rp 0.10 / embassy-usb 0.6 等（詳細は issue #01）。

## 使い方（実機）

1. **両半身に同じ UF2 を書込み**、TRRSで連結。
2. **EE_HANDS 手番登録（各半身1回）**: キーを押しながらUSB挿し直し
   - スペース＋上段の外側(小指)キー → **Left** ／ スペース＋上段の内側(人差し)キー → **Right**
3. **どちらの半身をUSBに挿してもOK**（USB側がマスタ）。ホストの**ローマ字ひらがなIME**をON。
4. **LED**: 役割（マスタ=緑/スレーブ=青）＋シフト状態（センター=シアン/濁音=赤/半濁音=黄/小書き=青）。

### 入力できるもの
- 単打・**センターシフト**（スペース）・**濁音/半濁音**（逆手シフト）・**小書き**（Q）
- **拗音1打鍵**（き+や→きゃ）・**外来音3キー**（て+い+右半→てぃ 等, 大岡式遅延確定）
- 句読点（、。）・Enter（こ+な）・長音・促音・撥音・**IME ON/OFF**（HJ/FG）
- **編集モード**: D+F押しながら右手=カーソル/削除、C+V押しながら右手=コピペ/Undo/選択

## ステータス

| 機能                                                    | 状態                        |
| ------------------------------------------------------- | --------------------------- |
| コア（engine/romaji/hid/codegen, 38テスト）             | ✅                          |
| USB-HID（1000Hz）/ COL2ROWスキャン                      | ✅ 実機                     |
| 分割UART（両手統合）/ EE_HANDS（左右自動）              | ✅ 実機                     |
| 全かな・拗音1打鍵・外来音3キー・句読点・IME・編集モード | ✅ 実機                     |
| LEDステータス（WS2812B）                                | ✅ 実機                     |
| **BLE-HID（cyw43+trouble-host）**                       | 🔲 #07（次の大物・無線化） |
| **PCカスタマイズ（WebHID 設定ツール）**                 | 🟡 #12 フェーズ3（物理レイアウト編集＋未使用キー直接割当） |
| .txtインポータ / 編集モード左手マクロ・固有名詞SC       | 🗄 任意                    |

→ 詳細・各issueは [issues/README.md](issues/README.md)。
