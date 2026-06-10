# [#01] firmware を thumbv6m で実ビルド＆USB-HID列挙

- **優先度:** P1
- **状態:** ✅ Done（2026-06-09 実機確認）
- **依存:** なし
- **関連設計:** アーキ設計書 §8 / 要件 FR-1,FR-4,FR-5 / README「ファーム」

## 目的
`firmware/` の参照スケルトンを **実際にビルドが通り、PCにUSBキーボードとして列挙される** 状態にする。
embassy のバージョン/API を確定し、以降のハード系Issue（05/06/07）の土台を作る。

## スコープ（やること）
- [x] `rustup target add thumbv6m-none-eabi`、`elf2uf2-rs` 導入
- [x] `firmware/Cargo.toml` の embassy 各クレートを**実在する整合版に固定**（下記版数）
- [x] `embassy-usb` で HID キーボードを構成（`usb_hid.rs` の descriptor を使用）
- [x] `main.rs`：USBタスクを spawn し `REPORTS` チャネルを購読 → `HidWriter.write`
- [x] マトリクススキャン → engine → emit の経路を結線（PoCシード配列）
- [x] **実機**で OS にキーボード認識＋打鍵で文字入力を確認（生マトリクステストで全確認）
- [x] BOOTSEL書込み（UF2）の実機手順確認
- [x] スキャン方向 = **COL2ROW 確定**（回路図 D1-35 ＋実機で確認）

## 確定した版数（Cargo.lock 固定）
- Rust **1.96.0**（embassy/`fixed`のMSRV 1.93+ を満たすため `rustup update` 実施）
- embassy-executor **0.10** / embassy-rp **0.10**(feat: rp2040,time-driver,critical-section-impl,unstable-pac)
  / embassy-usb **0.6** / embassy-time **0.5** / embassy-sync **0.8**
- `cortex-m` は **inline-asm 機能を外す**（ARMv6-M に BASEPRI 等が無くビルド不可のため）
- パニックは `panic-halt`（defmt版衝突回避。RTTログが要れば後で defmt-rtt+panic-probe へ）

## ハマりどころ（記録）
- `--manifest-path` 実行だと `firmware/.cargo/config.toml`(target指定)が読まれずホスト向けビルドになる
  → `firmware/` 内から `cargo build` する（`sev` 命令エラー等はこれが原因）。
- embassy-executor 0.10: task fn は `Result<SpawnToken,_>` を返し `spawn` は `()`。
  → `spawner.spawn(task(..).unwrap())`。
- USBは embassy-rp 0.10 の `Peri` 型 + `bind_interrupts!(USBCTRL_IRQ)`。

## やらないこと
- BLE（#07）、役割自動判定（#05）、分割連携（#06）、v15全配列（#04）
- この段階は **マスタ固定・片手・シード配列** で「列挙＋打鍵が出る」ところまで

## 受入条件（Done の定義）
- [x] `cargo build --release`（firmware内から）がエラー・警告なく完了
- [x] thumbv6m 向けにリンク成功し、**妥当な UF2（44KB）へ変換できる**（RP2040メモリ配置OK）
- [x] Pico に書込み後、OSにキーボードとして認識される ✅実機OK
- [x] 物理キー押下でホストに文字入力される（生マトリクステストで全キー確認）✅実機OK
- [x] 1ms周期スキャンで体感遅延なし（NFR-1）✅実機OK

## 成果物
- `firmware/naginata-firmware.uf2`（BOOTSEL中のPicoにドラッグで書込み可能）

## 実装メモ / 参照
- embassy 版固定が最大の難所。`embassy-rp`/`embassy-usb`/`embassy-executor` の版整合を最初に決める
- `emit_kana` の `try_send` 取りこぼし問題（アーキ §8 既知課題）はここで async送出へ是正
- ダイオード極性によるスキャン方向（行/列どちら駆動か）を実測確定（matrix.rs コメント参照）
