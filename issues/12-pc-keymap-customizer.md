# [#12] PCからのキーマップ・設定カスタマイズ機能（USBリアルタイム／設定ツール）

- **優先度:** P3
- **状態:** 🟡 フェーズ3実装（物理レイアウト編集＋未使用キーの直接割当）。実機検証待ち
- **依存:** #01（USB-HID）, #02（レイヤYAML＋codegen）, #10（コンボ/マクロ/編集モード）
- **関連設計:** 要件 FR-7.1（パラメータ変更）, FR-7.3（配列定義の分離保持）, アーキ §（設定永続化）

## 目的
**再ビルド／UF2書込みなしに、PC からキーマップと各種設定をカスタマイズ**できるようにする。
現状はビルド時 codegen（レイヤYAML→keymap）で、**薙刀式のメインキーのみ**が対象。
**ファンクションキー・マクロキー・カスタムキー（コンボ/編集モード/固有名詞SC 等）が
PCから編集できない**ため、ユーザが配列を調整するたびにフルビルドが必要になっている。

理想：
- **USB接続時にリアルタイム更新**できる（書込み即反映、再起動不要が望ましい）、または
- **設定ツールで設定値を読み書き**できる（端末→ツールへエクスポート／ツール→端末へ反映）。

## スコープ（やること）
- [x] **設定モデルの確定**：ランタイムで差し替え可能なパラメータ／キーマップの範囲を定義
  - [x] 主要パラメータ（FR-7.1: 同時打鍵窓・デバウンス・リピート間隔(#11)・LED輝度）
  - [x] キーマップ（単打レイヤ／**シフト面**）
  - [x] **カスタムキー**（コンボ2/3・編集モード割当・F/マクロ＝Action::Keys）※フェーズ2
  - [x] **未使用物理キーの直接割当**（F1-12/Del/PageUp/Macro列/Win 等＝タップ出力）※フェーズ3
  - [ ] 固有名詞SC（左手マクロ系）→ 後続（モデルとしては Keys で表現可）
- [x] **永続ストレージ**：フラッシュに設定領域を確保（codegen 既定値＋ユーザ上書き層）
  - [x] 末尾から2番目の4KBセクタに IR を保存（手番セクタと非衝突）
  - [x] 起動時に「ユーザ設定があれば適用、無ければ codegen 既定」へフォールバック
- [x] **PC↔端末プロトコル**：USB上の設定チャネルを実装
  - [x] 案A採用：**vendor-defined HID生レポート**（usagePage 0xFF00, **64B** in/out, 追加ドライバ不要）
  - [x] INFO / READ_DEFAULTS / READ_OVERRIDES / READ_PARAMS / WRITE / WRITE_PARAMS / COMMIT / RESET / REBOOT
- [x] **設定ツール**：PC側でキーマップ・パラメータを編集し端末へ反映するUI
  - [x] WebHID ブラウザUI（`site/naginata-config/`、タブ: パラメータ/単打・シフト面/コンボ/編集モード）
  - [ ] 既存レイヤYAML（#02）との相互変換（FR-7.3）→ 後続
- [ ] （任意）**リアルタイム反映**：write時に再起動なしで稼働マップを切替 → フェーズ3
  （現状は **COMMIT→再起動で反映**。reboot コマンドで即再起動可）

## やらないこと
- 既存の codegen ビルド経路（#02）の廃止（既定値の出所として維持）
- クラウド同期・アカウント機能（ローカル完結）

## 受入条件（Done の定義）
- [x] PCの設定ツールから端末の**現在のキーマップ／パラメータを読み出せる**（INFO/READ_PARAMS/READ_KEYMAP）
- [x] **単打・シフト面・コンボ・編集モードのキー操作**を変更して端末へ反映できる（WRITE→COMMIT）
- [x] 反映後、**再ビルド/UF2なしで**新しい配列・パラメータで打鍵できる（**要再起動**: COMMIT→reboot）
- [x] ユーザ設定を消去すると codegen 既定へ戻る（RESET でセクタ消去→起動時フォールバック）
- [x] 既存の薙刀式動作（#03/#04/#10）が回帰しない（コア62テスト緑、空オーバーレイは既定と一致）

## 実装状況（フェーズ1, 2026-06-10）
**基盤＋パラメータ＋単打キーまで実装・ホストテスト＋thumbv6mビルド緑。**
**実機検証済み**: WebHIDツールで単打入れ替え→COMMIT→再起動で配列変更を確認（2026-06-10）。

- コア `naginata-core`:
  - `src/config.rs` … 設定IR（`Params`/`Config`＋`serialize`/`parse`＋CRC16、`no_std`）。中間表現の本体。
  - `build.rs` … `SINGLE_TAP_SCS`（単打かなsc一覧）を追加生成。`Params::default()` は生成 `WINDOW_MS/TAP_MS` を参照。
  - `src/engine.rs` … 単打オーバーライド層（`set_single_tap_overrides` ＋ `resolve()`）。空なら既定と完全一致。
- ファーム `firmware`:
  - `src/config_store.rs` … 末尾-2番目セクタへIR永続化（`handedness.rs`と同型）。
  - `src/usb_config.rs` … vendor HIDレポートディスクリプタ＋コマンド処理（INFO/READ/WRITE/COMMIT/RESET/REBOOT）。
  - `src/main.rs` … 2つ目のHIDインターフェース、`usb_task`内に設定チャネル、起動時にflash設定をengine/LEDへ適用。
- PCツール: `site/naginata-config/`（WebHID, 素のHTML/JS）。デプロイ先 `siska-tech.github.io/naginata-config/`（別途）。

## 実装状況（フェーズ2, 2026-06-10）
**全カテゴリ編集（シフト面/コンボ2・3/編集モードのキー操作）まで実装・ホスト62テスト＋thumbv6mビルド緑。実機検証待ち。**

- コア `naginata-core`:
  - `src/keymap.rs` … 汎用 `Overlay`（layers/combo2/combo3/modes の `&'static` スライス）＋ `EMPTY_OVERLAY`。
    `layer_key/combo2_key/combo3_key` を `pub const fn` 化。
  - `src/engine.rs` … `set_overlay()` ＋ 集約ルックアップ（`resolve/has_layer/combo2/combo3/mode_action/is_combo3_prefix`）。
    keymap 直接参照を全て overlay 先引きメソッドへ置換。空 overlay で既定と完全一致（回帰なし）。
  - `src/config.rs` … **IR v2**。`OverrideVal{Kana|Keys}` ＋ 4セクション。`serialize/parse`（セクション/CRC/境界）。
    v1 は非互換（v2初回フラッシュで旧設定1回リセット）。
- ファーム `firmware`:
  - `src/usb_config.rs` … **64Bレポート**へ拡張。既定は phf `.entries()` を直接走査して返す。
    INFO/READ_DEFAULTS/READ_OVERRIDES/READ_PARAMS/WRITE/WRITE_PARAMS/COMMIT/RESET/REBOOT。サイズ適応ページング。
  - `src/config_store.rs` … `FLASH_BUF` 3840 へ拡張。
  - `src/main.rs` … `build_overlay`（かなバイト＋KeyPress の2アリーナ）で `&'static Overlay` を構築し engine へ。
- PCツール `site/naginata-config/` … タブUI。かな=テキスト入力／キー操作=記号チップ列エディタ
  （記号表は `build.rs symbol_to_keys` のミラー）。既定(phf)＋差分(staged)を JS でマージ表示。コンボは新規追加可。

## 実装状況（フェーズ3, 2026-06-10）
**物理レイアウト編集＋未使用物理キーの直接割当（タップ出力）まで実装・ホスト63テスト＋thumbv6mビルド緑。実機検証待ち。**

- コア `naginata-core/src/config.rs` … IR に `SEC_EXTRA`（`Config.extra: (hand,row,col)→値`）を追加。
  VERSION=2 据置（セクション追加は前方互換、再リセット不要）。
- ファーム:
  - `src/main.rs` … `build_direct`（未使用キーの `(hand,row,col)→Action` を static アリーナへ）。
    `engine_task` は `matrix_to_sc`=None かつ押下なら direct を引いて `emit_action`（薙刀式バイパス, タップ）。
    全押下で `LAST_KEY`（AtomicU16）を記録＝learn-key。
  - `src/usb_config.rs` … `CMD_READ_MATRIX`（(hand,row,col,sc) 列挙）/`CMD_READ_LASTKEY`（learn）/
    `SEC_EXTRA` の read/write を追加。ARMv6-M は RMW 非対応のため LAST_KEY は load+条件store でクリア。
- PCツール `site/naginata-config/` … 「レイアウト」タブ。薙刀式キーは matrix で同定し物理配置グリッドで
  単打面を直接編集。未使用キーは「キーを学習」→端末で押す→ (hand,row,col) を取り込み→直接キー/マクロ割当
  （F1-12/Del/PageUp/矢印/Win 等を記号チップで）。

### 後続（未）
- 押しっぱなし修飾（Win長押し=GUI hold, BS/Shift デュアル）・Lower/Raise モメンタリレイヤ（QMK風, スコープC）。
- 固有名詞SC・左手マクロ系（モデル上は Keys で表現可、UIプリセット未整備）。
- 再起動なしライブ反映（override スライスの `AtomicPtr` swap）。
- 既存レイヤYAML（#02）との相互変換（FR-7.3）。
- マトリクス配線(row,col→sc)そのものの再編集（learn で位置同定はするが再配線はしない）。

## 実装メモ / 参照
- まず **設定の中間表現**（フラッシュ上のバイナリ／TLV 等）を decide。codegen はこの表現の
  「初期イメージ生成器」と位置づけると、ビルド時とランタイムで型を共有できる。
- VIA/Vial（HID vendor report）方式が前例として近い。追加ドライバ不要なのが利点。
- カスタムキー（コンボ/MODE_LAYERS/固有名詞SC）はテーブルサイズ可変 → 領域確保とバージョニングを設計初期に固める。
- #11（リピート）のパラメータも本機能の設定対象に含める。
