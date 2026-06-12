//! BLE-HID（HID over GATT / HOGP, #07）。feature "ble" 時のみコンパイル。
//!
//! Pico W の CYW43439 を `cyw43`（BT HCI）で初期化し、`trouble-host` で
//! HID over GATT キーボードとして振る舞う。出力経路は usb_task 側で
//! 「USB 列挙中は USB、未列挙なら BLE」に切替（FR-5.2: USB優先/BLEフォールバック）。
//!
//! 役割方針（D.1 の拡張）:
//!   - BLE マスタになれるのは **左手番のみ**（右はスレーブ専従。両半身同一FWのまま）。
//!   - 起動後 3 秒以内に USB 列挙されたら BLE は起動しない（USB 運用）。
//!   - 未列挙なら広告開始。以降 USB が挿されても BLE は維持し、出力だけ USB 優先になる。
//!
//! リソース: CYW43 は PIO1(sm0) + DMA_CH2 + GP23/24/25/29（WS2812 は PIO0 + DMA_CH0 で非衝突）。
//!
//! 制限（PoC, issues/07 参照）:
//!   - ペアリングは Just Works（NoInputNoOutput）。ボンド情報は最小限の Flash store へ保存する。
//!   - 省電力（NFR-6）は未着手。

use core::sync::atomic::Ordering;

use bt_hci::param::Status;
use cyw43::{Aligned, A4};
use cyw43_pio::{PioSpi, DEFAULT_CLOCK_DIVIDER};
use embassy_futures::join::{join, join3};
use embassy_futures::select::{select3, Either3};
use embassy_rp::dma::Channel as DmaChannel;
use embassy_rp::gpio::{Level, Output};
use embassy_rp::peripherals::{DMA_CH2, PIN_23, PIN_24, PIN_25, PIN_29, PIO1};
use embassy_rp::pio::Pio;
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Timer};
use rand_core::RngCore;
use trouble_host::prelude::*;
use trouble_host::BondInformation;

use naginata_core::keymap::Hand;

use crate::{BLE_STATE, USB_CONFIGURED};

/// BLE 経由で送る HID レポート（usb_task の経路切替から受け取る。merge_held 適用済み）。
pub static BLE_REPORTS: Channel<ThreadModeRawMutex, [u8; 8], 16> = Channel::new();

pub enum BondStoreCommand {
    Save {
        local_address: Address,
        bond: BondInformation,
        cccd: [u16; 3],
    },
    Erase,
}

pub static BOND_STORE_COMMANDS: Channel<ThreadModeRawMutex, BondStoreCommand, 2> = Channel::new();

// BLE_STATE の値（0=off は初期値としてのみ使用）。
// 表示: WS2812（main.rs status_colors）と Pico W 本体 LED（WL_GPIO0, 下記 wl_led）。
#[allow(dead_code)]
pub const BLE_OFF: u8 = 0;
pub const BLE_ADVERTISING: u8 = 1;
pub const BLE_CONNECTED: u8 = 2; // 接続済み・ペアリング/購読待ち
pub const BLE_PAIRED: u8 = 3; // ペアリング完了（暗号化済み）
pub const BLE_PAIR_FAILED: u8 = 4; // 直近のペアリング失敗（切断後 3 秒保持）
pub const BLE_PAIR_STUCK: u8 = 5; // PassKey 入出力を要求された（NoInputNoOutput では想定外）
pub const BLE_SECURITY_REQUEST_FAILED: u8 = 6; // SMP Security Request が送れなかった
pub const BLE_DISCONNECTED_REMOTE: u8 = 7; // ホスト側から切断
pub const BLE_DISCONNECTED_AUTH: u8 = 8; // 認証/鍵関連で切断
pub const BLE_DISCONNECTED_TIMEOUT: u8 = 9; // 接続タイムアウト
pub const BLE_DISCONNECTED_OTHER: u8 = 10; // その他の切断理由

// --- CYW43 ファームウェアブロブ（4バイト整列必須） -------------------------
static WIFI_FW: &Aligned<A4, [u8]> = &Aligned(*cyw43_firmware::CYW43_43439A0);
static BT_FW: &Aligned<A4, [u8]> = &Aligned(*cyw43_firmware::CYW43_43439A0_BTFW);
static CLM: &[u8] = cyw43_firmware::CYW43_43439A0_CLM;
/// Pico W 用 NVRAM（embassy リポジトリ cyw43-firmware/nvram_rp2040.bin を同梱）。
static NVRAM: &Aligned<A4, [u8]> = &Aligned(*include_bytes!("nvram_rp2040.bin"));

/// デバイス名（GAP / 広告）。
const DEVICE_NAME: &str = "Naginata Keyboard";

/// BLE HID Report Map。Boot Keyboard 8B Input + 1B LED Output を記述する標準寄りの形。
///
/// USB 側の最小 descriptor は Output Report を持たないため、HOGP では別定義にする。
const BLE_KEYBOARD_REPORT_MAP: [u8; 63] = [
    0x05, 0x01, // Usage Page (Generic Desktop)
    0x09, 0x06, // Usage (Keyboard)
    0xA1, 0x01, // Collection (Application)
    0x05, 0x07, //   Usage Page (Keyboard)
    0x19, 0xE0, //   Usage Minimum (LeftControl)
    0x29, 0xE7, //   Usage Maximum (Right GUI)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x01, //   Logical Maximum (1)
    0x75, 0x01, //   Report Size (1)
    0x95, 0x08, //   Report Count (8)
    0x81, 0x02, //   Input (Data,Var,Abs) ; modifier byte
    0x95, 0x01, //   Report Count (1)
    0x75, 0x08, //   Report Size (8)
    0x81, 0x01, //   Input (Const) ; reserved byte
    0x95, 0x05, //   Report Count (5)
    0x75, 0x01, //   Report Size (1)
    0x05, 0x08, //   Usage Page (LEDs)
    0x19, 0x01, //   Usage Minimum (Num Lock)
    0x29, 0x05, //   Usage Maximum (Kana)
    0x91, 0x02, //   Output (Data,Var,Abs) ; LED bits
    0x95, 0x01, //   Report Count (1)
    0x75, 0x03, //   Report Size (3)
    0x91, 0x01, //   Output (Const) ; LED padding
    0x95, 0x06, //   Report Count (6)
    0x75, 0x08, //   Report Size (8)
    0x15, 0x00, //   Logical Minimum (0)
    0x25, 0x65, //   Logical Maximum (101)
    0x05, 0x07, //   Usage Page (Keyboard)
    0x19, 0x00, //   Usage Minimum (Reserved)
    0x29, 0x65, //   Usage Maximum (Keyboard Application)
    0x81, 0x00, //   Input (Data,Array) ; key array
    0xC0, // End Collection
];

// --- GATT サーバ定義（HOGP: HID + Battery + Device Information） -----------

// battery/dev_info はテーブル登録のみでフィールド参照しない（_ 前置で未使用警告を抑止）。
#[gatt_server]
struct Server {
    hid: HidService,
    _battery: BatteryService,
    _dev_info: DeviceInfoService,
}

/// HID Service (0x1812)。Boot Keyboard 互換の HOGP ディスクリプタ。
#[gatt_service(uuid = "1812")]
struct HidService {
    /// HID Information: bcdHID=1.11, bCountryCode=0, flags=NormallyConnectable(0x02)
    #[characteristic(uuid = "2a4a", read, value = [0x11, 0x01, 0x00, 0x02])]
    hid_info: [u8; 4],
    /// Report Map（Boot Keyboard Input + LED Output）
    #[characteristic(uuid = "2a4b", read, value = BLE_KEYBOARD_REPORT_MAP)]
    report_map: [u8; 63],
    /// HID Control Point（Suspend/ExitSuspend。受けるだけ）
    #[characteristic(uuid = "2a4c", write_without_response)]
    control_point: u8,
    /// Protocol Mode（1=Report。Boot へ切替されても入力は同形式）
    #[characteristic(uuid = "2a4e", read, write_without_response, value = 1)]
    protocol_mode: u8,
    /// Input Report（8B Boot Keyboard 形式）＋ Report Reference (id=0, type=Input)
    #[descriptor(uuid = "2908", read, value = [0u8, 1u8])]
    #[characteristic(uuid = "2a4d", read, notify)]
    input_report: [u8; 8],
    /// Output Report（LED 1B）＋ Report Reference (id=0, type=Output)
    #[descriptor(uuid = "2908", read, value = [0u8, 2u8])]
    #[characteristic(uuid = "2a4d", read, write, write_without_response)]
    output_report: u8,
    /// Boot Keyboard Input Report（キーボードは必須。同じ 8B を通知）
    #[characteristic(uuid = "2a22", read, notify)]
    boot_input: [u8; 8],
    /// Boot Keyboard Output Report（LED。受けるだけ）
    #[characteristic(uuid = "2a32", read, write, write_without_response)]
    boot_output: u8,
}

/// Battery Service (0x180F)。電池監視はないため固定 100%（HOGP ホストの要求対策）。
#[gatt_service(uuid = "180f")]
struct BatteryService {
    #[characteristic(uuid = "2a19", read, notify, value = 100)]
    level: u8,
}

/// Device Information Service (0x180A)。Windows のペアリングが PnP ID を参照する。
#[gatt_service(uuid = "180a")]
struct DeviceInfoService {
    /// PnP ID: source=USB(0x02), VID=0xc0de, PID=0xcafe, ver=0x0003（LE）
    #[characteristic(uuid = "2a50", read, value = [0x02, 0xde, 0xc0, 0xfe, 0xca, 0x03, 0x00])]
    pnp_id: [u8; 7],
    #[characteristic(uuid = "2a29", read, value = *b"Siska Tech Lab.")]
    manufacturer: [u8; 15],
}

/// ROSC の randombit を使う乱数源（SMP のシード用。PoC 品質で十分）。
struct RoscEntropy;

impl rand_core::RngCore for RoscEntropy {
    fn next_u32(&mut self) -> u32 {
        let mut v = 0u32;
        for _ in 0..32 {
            v = (v << 1) | (embassy_rp::pac::ROSC.randombit().read().randombit() as u32);
        }
        v
    }
    fn next_u64(&mut self) -> u64 {
        ((self.next_u32() as u64) << 32) | self.next_u32() as u64
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        for b in dest.iter_mut() {
            *b = self.next_u32() as u8;
        }
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), rand_core::Error> {
        self.fill_bytes(dest);
        Ok(())
    }
}
impl rand_core::CryptoRng for RoscEntropy {}

fn failure_state_for_disconnect(reason: Status) -> u8 {
    match reason {
        Status::REMOTE_USER_TERMINATED_CONN => BLE_DISCONNECTED_REMOTE,
        Status::AUTHENTICATION_FAILURE | Status::PIN_OR_KEY_MISSING => BLE_DISCONNECTED_AUTH,
        Status::CONN_TIMEOUT | Status::LMP_LL_RESPONSE_TIMEOUT => BLE_DISCONNECTED_TIMEOUT,
        _ => BLE_DISCONNECTED_OTHER,
    }
}

fn is_failure_state(state: u8) -> bool {
    matches!(
        state,
        BLE_PAIR_FAILED
            | BLE_PAIR_STUCK
            | BLE_SECURITY_REQUEST_FAILED
            | BLE_DISCONNECTED_REMOTE
            | BLE_DISCONNECTED_AUTH
            | BLE_DISCONNECTED_TIMEOUT
            | BLE_DISCONNECTED_OTHER
    )
}

fn try_enter_hid_ready<P: PacketPool>(
    conn: &GattConnection<'_, '_, P>,
    bond_matches_peer: bool,
) -> bool {
    let encrypted = conn
        .raw()
        .security_level()
        .map(|level| level.encrypted())
        .unwrap_or(false);
    if encrypted && bond_matches_peer {
        BLE_STATE.store(BLE_PAIRED, Ordering::Relaxed);
        true
    } else {
        false
    }
}

fn cccd_value<const N: usize>(table: &CccdTable<N>, handle: Option<u16>) -> u16 {
    if let Some(handle) = handle {
        for (entry_handle, value) in table.inner() {
            if *entry_handle == handle {
                return value.raw();
            }
        }
    }
    0
}

fn current_hid_cccd(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
) -> [u16; 3] {
    let table = server.get_cccd_table(conn.raw());
    table
        .as_ref()
        .map(|table| {
            [
                cccd_value(table, server.hid.input_report.cccd_handle),
                cccd_value(table, server.hid.boot_input.cccd_handle),
                cccd_value(table, server._battery.level.cccd_handle),
            ]
        })
        .unwrap_or([0; 3])
}

fn restore_hid_cccd(
    server: &Server<'_>,
    conn: &GattConnection<'_, '_, DefaultPacketPool>,
    saved: [u16; 3],
) {
    let Some(table) = server.get_cccd_table(conn.raw()) else {
        return;
    };
    let mut entries = *table.inner();
    let handles = [
        server.hid.input_report.cccd_handle,
        server.hid.boot_input.cccd_handle,
        server._battery.level.cccd_handle,
    ];
    for (handle, saved) in handles.into_iter().zip(saved) {
        if let Some(handle) = handle {
            for (entry_handle, value) in entries.iter_mut() {
                if *entry_handle == handle {
                    *value = CCCD::from(saved);
                }
            }
        }
    }
    server.set_cccd_table(conn.raw(), CccdTable::new(entries));
}

/// Pico W 本体 LED（CYW43 の WL_GPIO0）で BLE 状態を表示する。
/// 広告中=ゆっくり点滅 / 接続中（ペアリング・購読待ち）=速い点滅 /
/// ペアリング済み=点灯 / ペアリング失敗=非常に速い点滅 / その他=消灯。
async fn wl_led(mut control: cyw43::Control<'_>) -> ! {
    let mut on = false;
    loop {
        match BLE_STATE.load(Ordering::Relaxed) {
            BLE_ADVERTISING => {
                on = !on;
                control.gpio_set(0, on).await;
                Timer::after(Duration::from_millis(500)).await;
            }
            BLE_CONNECTED => {
                on = !on;
                control.gpio_set(0, on).await;
                Timer::after(Duration::from_millis(150)).await;
            }
            BLE_PAIR_FAILED
            | BLE_SECURITY_REQUEST_FAILED
            | BLE_DISCONNECTED_REMOTE
            | BLE_DISCONNECTED_AUTH
            | BLE_DISCONNECTED_TIMEOUT
            | BLE_DISCONNECTED_OTHER => {
                on = !on;
                control.gpio_set(0, on).await;
                Timer::after(Duration::from_millis(60)).await;
            }
            BLE_PAIR_STUCK => {
                on = !on;
                control.gpio_set(0, on).await;
                Timer::after(Duration::from_millis(300)).await;
            }
            BLE_PAIRED => {
                control.gpio_set(0, true).await;
                Timer::after(Duration::from_millis(200)).await;
            }
            _ => {
                control.gpio_set(0, false).await;
                Timer::after(Duration::from_millis(200)).await;
            }
        }
    }
}

#[embassy_executor::task]
pub async fn ble_task(
    pwr: embassy_rp::Peri<'static, PIN_23>,
    dio: embassy_rp::Peri<'static, PIN_24>,
    cs: embassy_rp::Peri<'static, PIN_25>,
    clk: embassy_rp::Peri<'static, PIN_29>,
    pio: embassy_rp::Peri<'static, PIO1>,
    dma: embassy_rp::Peri<'static, DMA_CH2>,
    hand: Hand,
    initial_bond: Option<(Address, BondInformation, [u16; 3])>,
) {
    // 役割: BLE マスタは左手番のみ（D.1 拡張）。
    if hand != Hand::Left {
        return;
    }
    // FR-5.2: USB 優先。起動猶予内に USB 列挙されたら BLE は起動しない。
    for _ in 0..30 {
        if USB_CONFIGURED.load(Ordering::Relaxed) {
            return;
        }
        Timer::after(Duration::from_millis(100)).await;
    }

    // --- CYW43 初期化（BT HCI 有効） ---------------------------------------
    let pwr = Output::new(pwr, Level::Low);
    let cs = Output::new(cs, Level::High);
    let mut pio = Pio::new(pio, crate::Irqs);
    let spi = PioSpi::new(
        &mut pio.common,
        pio.sm0,
        DEFAULT_CLOCK_DIVIDER,
        pio.irq0,
        cs,
        dio,
        clk,
        DmaChannel::new(dma, crate::Irqs),
    );
    let mut state = cyw43::State::new();
    let (_net, bt, mut control, cyw_runner) =
        cyw43::new_with_bluetooth(&mut state, pwr, spi, WIFI_FW, BT_FW, NVRAM).await;

    // cyw43 ランナーと BLE ホストを並走（どちらも戻らない）。
    join(cyw_runner.run(), async {
        control.init(CLM).await;

        // --- trouble-host（BLE ホストスタック） -----------------------------
        let controller: ExternalController<_, 10> = ExternalController::new(bt);
        let mut resources: HostResources<DefaultPacketPool, 1, 2> = HostResources::new();
        let mut rng = RoscEntropy;
        // Bond がある時は保存済みの local Static Random Address を使う。
        // Bond が無い時は起動ごとに新しいアドレスにして、ホスト側の古い鍵との衝突を避ける。
        let (address, initial_bond, initial_cccd) =
            if let Some((address, bond, cccd)) = initial_bond {
                (address, Some(bond), cccd)
            } else {
                let mut bytes = [0u8; 6];
                rng.fill_bytes(&mut bytes);
                bytes[5] = (bytes[5] & 0x3f) | 0xc0; // Static Random Address: most significant bits = 11
                (Address::random(bytes), None, [0; 3])
            };
        let stack = trouble_host::new(controller, &mut resources)
            .set_random_address(address)
            .set_random_generator_seed(&mut rng);
        stack.set_io_capabilities(IoCapabilities::NoInputNoOutput); // Just Works
        if let Some(bond) = initial_bond {
            let _ = stack.add_bond_information(bond);
        }
        let Host {
            mut peripheral,
            mut runner,
            ..
        } = stack.build();

        let server = match Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
            name: DEVICE_NAME,
            appearance: &appearance::human_interface_device::KEYBOARD,
        })) {
            Ok(s) => s,
            Err(_) => return, // 構築失敗（テーブル容量）はリンクエラー相当: BLE を諦める
        };

        // 広告データ（31B 制限内: Flags + HID UUID + 名前）。
        let mut adv_data = [0u8; 31];
        let adv_len = match AdStructure::encode_slice(
            &[
                AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
                AdStructure::ServiceUuids16(&[[0x12, 0x18]]), // 0x1812 HID (LE)
                AdStructure::CompleteLocalName(DEVICE_NAME.as_bytes()),
            ],
            &mut adv_data[..],
        ) {
            Ok(n) => n,
            Err(_) => return,
        };

        // ホストランナー・本体LED表示・「広告→接続→通知」ループを並走。
        let _ = join3(runner.run(), wl_led(control), async {
            loop {
                BLE_STATE.store(BLE_ADVERTISING, Ordering::Relaxed);
                // 広告開始 → 接続受理 → GATT サーバ接続
                let conn = async {
                    let advertiser = peripheral
                        .advertise(
                            &AdvertisementParameters::default(),
                            Advertisement::ConnectableScannableUndirected {
                                adv_data: &adv_data[..adv_len],
                                scan_data: &[],
                            },
                        )
                        .await?;
                    let conn = advertiser.accept().await?;
                    let _ = conn.set_bondable(true);
                    let conn = conn.with_attribute_server(&server)?;
                    Ok::<_, BleHostError<_>>(conn)
                }
                .await;
                let conn = match conn {
                    Ok(c) => c,
                    Err(_) => {
                        Timer::after(Duration::from_secs(1)).await;
                        continue;
                    }
                };
                BLE_STATE.store(BLE_CONNECTED, Ordering::Relaxed);

                let peer_identity = conn.raw().peer_identity();
                let mut bond_matches_peer = stack
                    .get_bond_information()
                    .iter()
                    .any(|bond| bond.identity.match_identity(&peer_identity));

                if !stack.get_bond_information().is_empty() && !bond_matches_peer {
                    let bonds = stack.get_bond_information();
                    for bond in bonds {
                        let _ = stack.remove_bond_information(bond.identity);
                    }
                    BOND_STORE_COMMANDS.send(BondStoreCommand::Erase).await;
                }
                if bond_matches_peer {
                    restore_hid_cccd(&server, &conn, initial_cccd);
                }

                // Bond が無い、または接続 peer と保存 bond が一致しない場合は初回扱いで
                // Security Request を送る。Bond が一致する場合だけ、まずホスト主導の
                // LE Start Encryption を待つ。
                if !bond_matches_peer {
                    if conn.raw().request_security().is_err() {
                        if conn
                            .raw()
                            .security_level()
                            .map(|level| level.encrypted())
                            .unwrap_or(false)
                        {
                            BLE_STATE.store(BLE_PAIRED, Ordering::Relaxed);
                        } else {
                            BLE_STATE.store(BLE_SECURITY_REQUEST_FAILED, Ordering::Relaxed);
                        }
                    }
                }
                let mut bond_encryption_fallback_at = if bond_matches_peer {
                    Instant::now() + Duration::from_secs(2)
                } else {
                    Instant::MAX
                };
                let mut hid_ready_check_at = Instant::now() + Duration::from_millis(100);
                let mut saved_cccd = initial_cccd;

                // 接続中: GATT イベント処理＋キーレポート通知。
                loop {
                    match select3(
                        conn.next(),
                        BLE_REPORTS.receive(),
                        Timer::at(hid_ready_check_at),
                    )
                    .await
                    {
                        Either3::First(event) => match event {
                            GattConnectionEvent::Disconnected { reason } => {
                                let encrypted = conn
                                    .raw()
                                    .security_level()
                                    .map(|level| level.encrypted())
                                    .unwrap_or(false);
                                if encrypted && bond_matches_peer {
                                    let cccd = current_hid_cccd(&server, &conn);
                                    if cccd != saved_cccd {
                                        for bond in stack.get_bond_information() {
                                            if bond.identity.match_identity(&peer_identity) {
                                                BOND_STORE_COMMANDS
                                                    .send(BondStoreCommand::Save {
                                                        local_address: address,
                                                        bond,
                                                        cccd,
                                                    })
                                                    .await;
                                                break;
                                            }
                                        }
                                    }
                                } else {
                                    BLE_STATE.store(
                                        failure_state_for_disconnect(reason),
                                        Ordering::Relaxed,
                                    );
                                    let bonds = stack.get_bond_information();
                                    for bond in bonds {
                                        let _ = stack.remove_bond_information(bond.identity);
                                    }
                                    BOND_STORE_COMMANDS.send(BondStoreCommand::Erase).await;
                                }
                                break;
                            }
                            GattConnectionEvent::Gatt { event } => {
                                // 読み書きは GATT テーブルの値で応答（特性側の処理は不要）。
                                if let Ok(reply) = event.accept() {
                                    reply.send().await;
                                }
                                if try_enter_hid_ready(&conn, bond_matches_peer) {
                                    let cccd = current_hid_cccd(&server, &conn);
                                    if cccd != saved_cccd {
                                        for bond in stack.get_bond_information() {
                                            if bond.identity.match_identity(&peer_identity) {
                                                BOND_STORE_COMMANDS
                                                    .send(BondStoreCommand::Save {
                                                        local_address: address,
                                                        bond,
                                                        cccd,
                                                    })
                                                    .await;
                                                saved_cccd = cccd;
                                                break;
                                            }
                                        }
                                    }
                                }
                            }
                            // 接続パラメータ更新要求は必ず応答する（無応答だと
                            // ホスト（特に Windows）がタイムアウトし切断/ペアリング失敗になる）。
                            GattConnectionEvent::RequestConnectionParams(req) => {
                                let _ = req.accept(None, &stack).await; // 要求値をそのまま受理
                            }
                            GattConnectionEvent::PairingComplete { bond, .. } => {
                                // PoC では 1 ホスト運用として、古い RAM/Flash ボンドを置き換える。
                                let bonds = stack.get_bond_information();
                                for bond in bonds {
                                    let _ = stack.remove_bond_information(bond.identity);
                                }
                                if let Some(bond) = bond {
                                    let cccd = current_hid_cccd(&server, &conn);
                                    BOND_STORE_COMMANDS
                                        .send(BondStoreCommand::Save {
                                            local_address: address,
                                            bond: bond.clone(),
                                            cccd,
                                        })
                                        .await;
                                    saved_cccd = cccd;
                                    let _ = stack.add_bond_information(bond);
                                    bond_matches_peer = true;
                                }
                                BLE_STATE.store(BLE_PAIRED, Ordering::Relaxed);
                            }
                            GattConnectionEvent::PairingFailed(_) => {
                                let bonds = stack.get_bond_information();
                                for bond in bonds {
                                    let _ = stack.remove_bond_information(bond.identity);
                                }
                                BOND_STORE_COMMANDS.send(BondStoreCommand::Erase).await;
                                BLE_STATE.store(BLE_PAIR_FAILED, Ordering::Relaxed);
                            }
                            // PassKey 系は NoInputNoOutput では想定外。LED で見える化して中断する。
                            GattConnectionEvent::PassKeyDisplay(_)
                            | GattConnectionEvent::PassKeyConfirm(_)
                            | GattConnectionEvent::PassKeyInput => {
                                BLE_STATE.store(BLE_PAIR_STUCK, Ordering::Relaxed);
                                let _ = conn.raw().pass_key_cancel();
                            }
                            // Phy/DataLength 更新は無視。
                            _ => {}
                        },
                        Either3::Second(report) => {
                            let _ = try_enter_hid_ready(&conn, bond_matches_peer);
                            // 未購読(CCCD off)等のエラーは無視（接続直後など）。
                            let _ = server.hid.input_report.notify(&conn, &report).await;
                            let _ = server.hid.boot_input.notify(&conn, &report).await;
                        }
                        Either3::Third(_) => {
                            if try_enter_hid_ready(&conn, bond_matches_peer) {
                                bond_encryption_fallback_at = Instant::MAX;
                                hid_ready_check_at = Instant::MAX;
                            } else if bond_matches_peer
                                && Instant::now() >= bond_encryption_fallback_at
                            {
                                let bonds = stack.get_bond_information();
                                for bond in bonds {
                                    let _ = stack.remove_bond_information(bond.identity);
                                }
                                BOND_STORE_COMMANDS.send(BondStoreCommand::Erase).await;
                                bond_matches_peer = false;
                                bond_encryption_fallback_at = Instant::MAX;
                                let _ = conn.raw().request_security();
                                hid_ready_check_at = Instant::now() + Duration::from_millis(100);
                            } else {
                                hid_ready_check_at = Instant::now() + Duration::from_millis(100);
                            }
                        }
                    }
                }
                // 失敗表示は切断後 3 秒保持してから広告再開（LED で観察できるように）。
                if is_failure_state(BLE_STATE.load(Ordering::Relaxed)) {
                    Timer::after(Duration::from_secs(3)).await;
                }
            }
        })
        .await;
    })
    .await;
}
