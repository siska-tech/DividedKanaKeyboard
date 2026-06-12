# [#07] BLE-HID（cyw43 + trouble-host）

- **優先度:** P3
- **状態:** ✅ 主要実機検証済み（2026-06-12, Windows / Android / Linux 再接続安定）
- **依存:** #01, #05, #06
- **関連設計:** 要件 §8.1（Rust優先・不可ならC/C++）, FR-5 / アーキ §13

## 目的
マスタを **BLE-HIDキーボード**として動作させ、モバイルバッテリ給電でワイヤレス運用する。
**最大のリスク領域**（cyw43上のRust製BLEスタックは成熟度が低い）。

## スコープ（やること）
- [x] Pico W の CYW43 を `cyw43` ドライバで初期化（firmware blob は cyw43-firmware crate ＋ NVRAM 同梱）
- [x] BLEホストスタック（`trouble-host`）で HID over GATT を構成
- [x] USB(#01)と BLE の出力経路を切替（FR-5.2：USB優先/BLEフォールバック）
- [x] ペアリング保持（FR-5.3）— 1ホスト分の Flash bond store（LTK / peer identity / IRK /
      local Static Random Address / CCCD）を実装
- [ ] 省電力（NFR-6：アイドル時スキャン間引き・LED減光）— 未着手
- [x] LED状態表示（FR-6：USB緑 / BLE接続マゼンタ / BLE広告中 暗マゼンタ / スレーブ青）

## やらないこと
- 内蔵バッテリ/充電回路（PoC外。モバイルバッテリ給電）

## 受入条件（Done の定義）
- [x] PC/スマホと BLE ペアリングし、HIDキーボードとして打鍵が届く
- [x] Windows / Android / Linux で、キーボード電源断後もホスト側登録を消さず再接続できる
- [x] USB接続時はUSB、未接続時はBLEへ切替（FR-5.2）
- [ ] モバイルバッテリ給電で左右とも動作（TRRS +5V 供給, §3.2）

## 実装メモ / 参照（as-built 2026-06-12）
- **ビルド**: `cargo build --release --features ble`（既定ビルドは BLE 無し＝従来挙動のまま）。
  実装は [firmware/src/ble.rs](../firmware/src/ble.rs)（設計判断はモジュール冒頭コメント）。
- **構成**: cyw43 0.7（feature bluetooth）＋ cyw43-pio 0.10 ＋ cyw43-firmware 0.1（blob）＋
  trouble-host 0.6（security/default-packet-pool）＋ bt-hci 0.8。NVRAM は embassy リポジトリの
  `nvram_rp2040.bin` を src に同梱。ARMv6-M の CAS 非対応は `portable-atomic`(critical-section) で解決。
  embassy-sync は trouble に合わせ **0.7 へ固定**（gatt_server マクロが参照する NoopRawMutex の版一致）。
- **リソース**: CYW43 = PIO1(sm0)＋DMA_CH2＋GP23/24/25/29（WS2812 の PIO0/DMA_CH0 と非衝突）。
- **役割（D.1拡張）**: BLE マスタ = **左手番のみ**・起動3秒以内に USB 未列挙のとき広告開始。
  マスタ判定は `is_master() = USB_CONFIGURED || BLE_STATE != 0`。
  出力経路は usb_task 内で「USB列挙中→USB / 未列挙→BLE チャネル」（ホールド合成 merge_held 適用後）。
- **GATT**: HID(0x1812: HID Info / Report Map=BLE用63B / Control Point / Protocol Mode /
  Input Report+ReportRef / Output Report+ReportRef / Boot Keyboard In/Out) ＋ Battery(100%固定) ＋
  DIS(PnP ID=USB 0xc0de:0xcafe)。ペアリングは Just Works（NoInputNoOutput）。SMP 乱数は ROSC randombit。
- **Bond store**: 末尾から3番目の4KBセクタに1ホスト分を保存。
  保存対象は local Static Random Address / peer identity address / IRK / LTK / bonded flag /
  security level / HID Input, Boot Input, Battery の CCCD。config は末尾から2番目、EE_HANDS は末尾で非衝突。
- **撤退基準**: Rust(cyw43+trouble-host)で BLE-HID が安定しない場合、
  **C/C++（pico-sdk + BTstack）へフォールバック**（要件 §8.1）。
  その際もコア（engine/romaji/hid）は移植で流用（アーキ §13）

## 実機検証ログ
- **2026-06-12**: 広告 OK（スキャン一覧に表示）。ペアリング失敗 → 原因調査:
  `GattConnectionEvent::RequestConnectionParams` への**必須応答（accept/reject）が欠落**していた
  （Windows は接続直後に接続パラメータ更新を要求するため、無応答だとタイムアウト→切断/失敗）。
  → `req.accept(None, &stack)` で受理するよう修正。トラブルシュート用に状態表示を拡張:
  - **Pico W 本体 LED（WL_GPIO0）**: 広告中=遅い点滅(1Hz) / 接続中=速い点滅 /
    ペアリング済み=点灯 / ペアリング失敗=非常に速い点滅 / BLE無効=消灯。
  - **WS2812**: 広告中=暗マゼンタ / 接続中=マゼンタ / ペアリング済み=シアン /
    失敗=赤（切断後3秒保持）/ USBマスタ=緑 / スレーブ=青。
  - 失敗が「接続中（マゼンタ/速い点滅）のまま」か「PairingFailed（赤）」かで切り分け可能。
- **2026-06-12**: 一瞬シアン（ペアリング/暗号化完了）後、広告状態へ戻る症状を確認。
  trouble-host は接続の既定が non-bondable のため、`accept()` 直後に `set_bondable(true)` し、
  `PairingComplete` の `bond` を `stack.add_bond_information()` で RAM 保存するよう修正。
  これで通電中の再接続では LTK を返せる想定（フラッシュ永続化は残）。
- **2026-06-12**: Windows/Android/Linux いずれも接続直後に広告状態へ戻る症状を確認。
  ホスト共通なので HOGP の GATT/Report Map 不備を疑い、BLE 用 Report Map を USB 用 45B から
  標準寄りの 63B（Boot Keyboard Input + LED Output）へ分離。あわせて Report Protocol 側の
  Output Report characteristic（2A4D + Report Reference type=Output）を追加。
  ペアリング完了前に切断した場合は赤表示を 3 秒保持するようにして、即広告復帰と区別する。
- **2026-06-12**: 赤 3 秒（接続後、暗号化前に切断）を Windows/Linux で確認。
  `request_security()` 即時エラーと HCI 切断理由を LED 色で分ける診断を追加:
  橙=Security Request 送信失敗 / ピンク=Remote User Terminated / 赤=Authentication or PIN/Key Missing /
  明青=Connection Timeout / 白=その他。Linux では `sudo btmon` を取り、ペアリング試行中の
  SMP Pairing Request/Response と Disconn Complete reason を確認する。
- **2026-06-12**: Fedora `btmon` で、Central が `KeyboardDisplay + MITM + SC` を要求し、
  Peripheral が `NoInputNoOutput` なのに `MITM + SC` (`AuthReq=0x0d`) を返していることを確認。
  BlueZ は `Authentication requirements (0x03)` で Pairing Failed。
  原因は trouble-host 0.6.0 の `AuthReq::new()` が常に MITM を立てる実装だったため。
  `firmware/vendor/trouble-host-0.6.0` を追加し、PoC では `AuthReq::new()` を `Bonding + SC`
  (`0x09`) にパッチ。`Cargo.toml [patch.crates-io]` でローカル版を使用する。
  次回 `btmon` では `Pairing Response` の Authentication requirement が
  `Bonding, SC, No Keypresses (0x09)` になることを確認する。
- **2026-06-12**: Linux/Fedora でペアリング承認 UI から接続・打鍵成功を確認。
  Windows は未接続のため、Windows 互換切り分けとして PoC を `DisplayYesNo + MITM + SC` に変更し、
  Numeric Comparison の `PassKeyConfirm` をファーム側で自動承認する診断モードへ切替。
  実機には表示/確認 UI がないため、恒久対応では「表示/確認キーを追加する」か
  「Just Works のまま Windows 互換を別手段で確保する」か要判断。
- **2026-06-12**: `DisplayYesNo + MITM + SC` 診断モードは Windows でも NG、Linux でも接続不能化。
  Linux で打鍵成功済みの `NoInputNoOutput + Bonding + SC`（MITM なし, `AuthReq=0x09`）へ戻す。
  Windows はこの成功点を維持したまま、別途 Windows 側ログ/イベントビューアで原因追跡する。
- **2026-06-12**: Linux/Android で接続可能状態に復帰。ただしペアリング解除後の再ペアリングが不安定。
  ホスト側だけが bond を削除し、キーボード側 RAM に古い LTK が残る stale bond を疑う。
  PoC は 1 ホスト運用として、`PairingComplete` 時に既存 RAM bond を全削除して新 bond に置換し、
  `PairingFailed` または認証/鍵系切断（Authentication Failure / PIN or Key Missing）時にも
  RAM bond を全削除するよう修正。
- **2026-06-12**: Linux は安定化し、Android はペアリング承認 UI が確実に出る一方で承認ループ化。
  Android は失敗時に認証系 reason 以外で暗号化前切断する可能性があるため、
  「暗号化前に切断したら理由を問わず RAM bond を全削除」へ拡張。
  Windows はペアリング承認 UI 自体が出ないため、SMP 到達前に Windows が候補を棄却している可能性あり。
- **2026-06-12**: 最小 Flash bond store を追加。
  末尾から3番目の 4KB セクタに 1 ホスト分の `BondInformation`
  （LTK / peer identity address / IRK / bonded flag / security level）を保存し、起動時に `stack.add_bond_information()`
  で復元する。`trouble-host` 0.6 API では EDIV/Rand は公開されず、LESC 再暗号化も EDIV/Rand=0 で行うため、
  現時点の保存対象からは除外。`PairingComplete` で保存、`PairingFailed`/暗号化前切断で消去。
- **2026-06-12**: 再接続時の `Encryption Change: Authentication Failure` を調査。
  `btmon` では初回ペアリングで Linux が保存した LTK と、再接続時に `LE Start Encryption` で使う LTK が同一。
  したがってホスト側の鍵不整合ではなく、Peripheral 側が再接続時に保存済み LTK を正しく返せていない経路へ絞り込んだ。
  `trouble-host` 0.6.0 は `LE Long Term Key Request` の `Random Number` / `EDIV` を捨てていたため、
  vendored stack で `SecurityEventData::SendLongTermKey { handle, random_number, encrypted_diversifier }`
  に拡張。LE Secure Connections bond として `Rand=0, EDIV=0` の場合だけ `LeLongTermKeyRequestReply` で
  `ltk.to_le_bytes()` を返すようにした。これで Windows / Android / Linux で電源断後の再接続が安定。
- **2026-06-12**: Static Random Address と bond の整合性を修正。
  bond がある場合は保存済み local Static Random Address を使用し、bond が無い場合は起動ごとに新しい
  Random Static Address を生成する。これにより「同じBLEアドレスなのにデバイス側だけ鍵を忘れている」
  開発中の stale bond 事故を減らした。
- **2026-06-12**: stale bond 判定を追加。
  Flash に bond があるだけでは再接続扱いにせず、接続 peer identity と保存 bond が一致した場合だけ
  bonded reconnect とみなす。不一致なら RAM/Flash bond を消去し、初回接続として `request_security()` を送る。
  一致しているのに一定時間暗号化されない場合も stale とみなし、bond clear + `request_security()` fallback する。
- **2026-06-12**: HID ready 状態遷移を `PairingComplete` 依存から外した。
  再接続では `PairingComplete` が発生しないため、`connected && encrypted && bonded_peer_match` を
  `try_enter_hid_ready()` で共通判定し、初回ペアリング後・GATTイベント後・再接続後の定期確認・HID report送信前に呼ぶ。
  これにより、bond済み再接続後も LED がマゼンタからシアンへ遷移する。
- **2026-06-12**: CCCD 保存・復元を追加。
  Android はbond済み再接続時に CCCD を書き直さない場合があるため、HID Input Report / Boot Keyboard Input Report /
  Battery Level の CCCD 値を bond store に保存し、再接続時に `AttributeServer` の CCCD table へ復元する。
  Linux はLEDがマゼンタのままでも打鍵Notifyが通っていたためLED状態だけの問題も含んでいたが、
  Androidの再接続後Notify安定化には CCCD 復元が有効だった。
- **2026-06-12**: Windows / Android / Linux で、通常の電源断後にホスト側登録を削除せず再接続できることを確認。
  Android/Linux とも再接続後にキーボード側LEDがシアンへ入り、打鍵が通る。Windows でも再接続がスムーズ。

## trouble-host へのフィードバック候補
今回の安定化で、upstream PR / issue にできそうな点:

1. **`AuthReq::new()` が NoInputNoOutput でも MITM を立てる**
   - 現象: Pairing Response が `NoInputNoOutput` なのに `Bonding + MITM + SC` になり、BlueZ が
     `Authentication requirements (0x03)` で拒否する。
   - ローカル修正: `Bonding + SC`（MITMなし）へ変更。
   - PR候補: IO capability と pairing method に応じて MITM bit を決める。

2. **Peripheral の `LE Long Term Key Request` で Rand/EDIV を捨てている**
   - 現象: 再接続時に SC鍵かLegacy鍵かを判定できず、ログ/診断もしづらい。
   - ローカル修正: `SecurityEventData::SendLongTermKey` に `random_number` と `encrypted_diversifier` を持たせ、
     `Rand=0, EDIV=0` のSC鍵として扱う。
   - PR候補: LTK lookup API/イベントに Rand/EDIV を渡し、Legacy Pairing鍵も扱える設計にする。

3. **BondInformation に保存すべきメタ情報が不足**
   - 現状APIで復元できるのは LTK / peer identity / security level / bonded flag が中心。
   - 実運用では local identity/static address、key size、SC/authenticated、EDIV/Rand、CCCD も一緒に管理したい。
   - PR候補: `BondInformation` または別型で、永続化向けbond recordを提供する。

4. **CCCD永続化はアプリ責務だが、HOGPでは落とし穴になりやすい**
   - Androidなどがbond済み再接続時に CCCD を再書き込みしない場合、Notifyが送れない。
   - PR/ドキュメント候補: bonded GATT server は CCCD table の保存・復元が必要であることを例示する。

## 運用メモ
- **BLE 入り FW が必要なのは左半身のみ**（BLE マスタは左手番限定。右半身の ble_task は即終了し
  従来どおり UART スレーブ）。右半身は従来 FW のままでも互換。両方に入れても無害（同一FW運用可）。

## 残作業
1. モバイルバッテリ給電で左右動作を最終確認
2. iOS / iPadOS があれば追加確認
3. 省電力（NFR-6）・必要なら複数ホスト切替
4. upstream trouble-host へ issue/PR 化する場合、最小再現ログとローカルパッチを整理
