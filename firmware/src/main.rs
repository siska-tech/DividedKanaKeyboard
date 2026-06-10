//! 薙刀式 分割キーボード ファーム（RP2040 / Pico W, embassy）。
//!
//! 役割（§3.1 / #05,#06）:
//!   - USB列挙した側 = マスタ: 自分(Left)＋相方(Right)を統合 → engine → USB HID
//!   - 未列挙側 = スレーブ: 自分のキーイベントを UART 送信するだけ
//!   両半身は同一FW。USB_CONFIGURED で役割を判定する。
//!
//! MVP制約(#06): 手番は Left 固定（EE_HANDS #05 未実装）。
//!   → **左半身をUSBに挿す**こと。右半身はスレーブとしてUARTで左へ送る。
//!
//! 検証済みの中核ロジックは naginata-core（ホストテスト）にある。

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, Ordering};

use embassy_executor::Spawner;
use embassy_futures::join::join3;
use embassy_futures::select::{select, Either};
use embassy_rp::bind_interrupts;
use embassy_rp::dma;
use embassy_rp::gpio::{Input, Level, Output, Pull};
use embassy_rp::peripherals::{DMA_CH0, PIO0, UART0, USB};
use embassy_rp::pio::{InterruptHandler as PioInterruptHandler, Pio};
use embassy_rp::pio_programs::ws2812::{PioWs2812, PioWs2812Program};
use embassy_rp::uart::{BufferedInterruptHandler, BufferedUart, BufferedUartRx, BufferedUartTx, Config as UartConfig};
use embassy_rp::usb::{Driver, InterruptHandler as UsbInterruptHandler};
use embassy_sync::blocking_mutex::raw::ThreadModeRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::{Duration, Instant, Ticker, Timer};
use embassy_usb::class::hid::{self, HidReaderWriter, HidWriter, State};
use embassy_usb::{Builder, Config as UsbConfig, Handler};
use embedded_io_async::{Read, Write};
use panic_halt as _;
use smart_leds::RGB8;

use naginata_core::config::Config;
use naginata_core::{hid as nghid, keymap, romaji, Action, Engine};
use keymap::Hand;

mod config_store;
mod handedness;
#[allow(dead_code)]
mod matrix;
mod split;
mod usb_config;
#[allow(dead_code)]
mod usb_hid;

use matrix::Matrix;
use split::KeyEvent;

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => UsbInterruptHandler<USB>;
    UART0_IRQ => BufferedInterruptHandler<UART0>;
    PIO0_IRQ_0 => PioInterruptHandler<PIO0>;
    DMA_IRQ_0 => dma::InterruptHandler<DMA_CH0>;
});

/// ShiftMask ビット（layout/naginata.yaml の bit と一致）。LED表示用。
const SH_CENTER: u8 = 0x01; // space
const SH_DAKUTEN: u8 = 0x02 | 0x04; // 右濁(j) | 左濁(f)
const SH_HANDAKU: u8 = 0x08 | 0x10; // 右半(m) | 左半(v)
const SH_SMALL: u8 = 0x20; // 小(q)

/// マスタの現在シフト状態（LED表示用）。engine_task が更新（display_shifts）。
static HELD_SHIFTS: AtomicU8 = AtomicU8::new(0);
/// マスタから受信したシフト状態（スレーブのLED表示用）。uart_rx が更新。
static REMOTE_SHIFTS: AtomicU8 = AtomicU8::new(0);

/// 生マトリクス・テストモード（ブリングアップ用, #01）。
/// true で配列/エンジンを通さず押下位置を `r{行}c{列} ` と打鍵する。
const RAW_MATRIX_TEST: bool = false;

/// USB が Configured（ホストに列挙）されているか = この半身がマスタか。
static USB_CONFIGURED: AtomicBool = AtomicBool::new(false);

/// learn-key（#12 フェーズ3）: 最後に押された物理位置 `(hand<<12)|(row<<4)|col`。
/// bit15=fresh（READ_LASTKEY で取得時にクリア）。未使用キーの (hand,row,col) 同定に使う。
pub static LAST_KEY: AtomicU16 = AtomicU16::new(0);

/// 物理位置を u16 にパック（hand: Left=0 / Right=1, row<4bit, col<4bit）。
pub fn pack_pos(hand: Hand, row: u8, col: u8) -> u16 {
    let h = match hand {
        Hand::Left => 0u16,
        Hand::Right => 1u16,
    };
    (h << 12) | ((row as u16) << 4) | (col as u16)
}

/// 8バイト HID レポート: engine → USB writer。
static REPORTS: Channel<ThreadModeRawMutex, [u8; 8], 16> = Channel::new();
/// 統合キーイベント（自半身＋相方）: scan/uart_rx → engine。
static ENGINE_IN: Channel<ThreadModeRawMutex, MatrixEv, 32> = Channel::new();
/// 送信フレーム: scan → uart_tx。
static UART_TX_Q: Channel<ThreadModeRawMutex, [u8; 2], 32> = Channel::new();

/// 手番付きキーイベント。
#[derive(Clone, Copy)]
struct MatrixEv {
    hand: Hand,
    row: u8,
    col: u8,
    pressed: bool,
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());

    // --- §2.1 ピンアサイン（COL2ROW: 列駆動・行センス）---------------------
    let cols = [
        Output::new(p.PIN_2, Level::Low),
        Output::new(p.PIN_3, Level::Low),
        Output::new(p.PIN_4, Level::Low),
        Output::new(p.PIN_5, Level::Low),
        Output::new(p.PIN_6, Level::Low),
        Output::new(p.PIN_7, Level::Low),
        Output::new(p.PIN_8, Level::Low),
        Output::new(p.PIN_9, Level::Low),
        Output::new(p.PIN_10, Level::Low),
        Output::new(p.PIN_11, Level::Low),
        Output::new(p.PIN_12, Level::Low),
        Output::new(p.PIN_13, Level::Low),
        Output::new(p.PIN_14, Level::Low),
        Output::new(p.PIN_15, Level::Low),
    ];
    let rows = [
        Input::new(p.PIN_16, Pull::Down),
        Input::new(p.PIN_17, Pull::Down),
        Input::new(p.PIN_18, Pull::Down),
        Input::new(p.PIN_19, Pull::Down),
        Input::new(p.PIN_20, Pull::Down),
        Input::new(p.PIN_21, Pull::Down),
    ];
    let mut matrix = Matrix::new(cols, rows);

    // --- EE_HANDS 手番（#05）----------------------------------------------
    // 起動時プロビジョニング: スペース＋外側上キー=Left / スペース＋内側上キー=Right。
    // （誤爆防止に2キー同時。一度設定すればフラッシュに残り、以降は自動。）
    let mut flash: handedness::HandFlash = handedness::new_flash(p.FLASH);
    if matrix.is_held(4, 5).await {
        if matrix.is_held(1, 1).await {
            let _ = handedness::write_hand(&mut flash, Hand::Left);
        } else if matrix.is_held(1, 5).await {
            let _ = handedness::write_hand(&mut flash, Hand::Right);
        }
    }
    // 保存値を読む。未設定なら暫定 Left（要プロビジョニング）。
    let local_hand = handedness::read_hand(&mut flash).unwrap_or(Hand::Left);

    // --- PC カスタマイズ設定（#12）----------------------------------------
    // flash のユーザ設定を読む（未保存/破損なら codegen 既定へフォールバック）。
    // パラメータと単打オーバーライドを engine へ、設定本体は config_task(usb) へ渡す。
    // 反映は起動時のみ。COMMIT 後の再反映は再起動（REBOOT コマンド）で行う。
    let cfg = config_store::read_config(&mut flash).unwrap_or_default();
    let overlay = build_overlay(&cfg);
    let direct = build_direct(&cfg);
    let params = cfg.params;

    // --- 分割UART（UART0: GP0=TX, GP1=RX。TRRSでクロス配線済）---------------
    // バッファは 'static 必須。main で一度だけ初期化し他で触らないので static mut で安全。
    static mut TX_BUF: [u8; 64] = [0; 64];
    static mut RX_BUF: [u8; 64] = [0; 64];
    let tx_buf: &'static mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(TX_BUF) };
    let rx_buf: &'static mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(RX_BUF) };
    let mut uart_config = UartConfig::default();
    uart_config.baudrate = 115200; // 確実性優先（FR-2.3）
    let uart = BufferedUart::new(p.UART0, p.PIN_0, p.PIN_1, Irqs, tx_buf, rx_buf, uart_config);
    let (uart_tx, uart_rx) = uart.split();

    // --- USB-HID ----------------------------------------------------------
    let driver = Driver::new(p.USB, Irqs);

    spawner.spawn(usb_task(driver, flash, cfg).unwrap());
    spawner.spawn(uart_tx_task(uart_tx).unwrap());
    spawner.spawn(uart_rx_task(uart_rx, local_hand).unwrap());
    spawner.spawn(scan_task(matrix, local_hand).unwrap());
    spawner.spawn(engine_task(
        params.window_ms as u32,
        params.repeat_delay_ms as u32,
        params.repeat_interval_ms as u32,
        overlay,
        direct,
    ).unwrap());

    // --- WS2812B ステータスLED（GP28, 2個）--------------------------------
    // PIO0+DMA で駆動。pio.common はローカル借用だが main は無限ループするので問題なし。
    let Pio { mut common, sm0, .. } = Pio::new(p.PIO0, Irqs);
    let program = PioWs2812Program::new(&mut common);
    let mut ws: PioWs2812<'_, PIO0, 0, 2, _> =
        PioWs2812::new(&mut common, sm0, p.DMA_CH0, Irqs, p.PIN_28, &program);

    let led_brightness = params.led_brightness; // #12: 設定の LED 輝度スケール（255=素の色）
    let mut ticker = Ticker::every(Duration::from_millis(20)); // 更新間隔（応答性）
    let mut latch: u8 = 0; // 直近のシフト状態
    let mut hold: u8 = 0; // ラッチ保持の残りtick（一瞬のシフトも見えるように, 3*20=60ms）
    let mut last_sent: u8 = 0xff; // 相方へ最後に送ったシフト状態（強制初回送信）
    let mut heartbeat: u8 = 0;
    loop {
        let master = USB_CONFIGURED.load(Ordering::Relaxed);
        // マスタは自分の状態、スレーブはマスタから受信した状態。
        let cur = if master {
            HELD_SHIFTS.load(Ordering::Relaxed)
        } else {
            REMOTE_SHIFTS.load(Ordering::Relaxed)
        };
        // マスタは自分のシフト状態を相方へ送る（変化時＋約0.5sハートビート）。
        if master {
            heartbeat += 1;
            if cur != last_sent || heartbeat >= 25 {
                // 変化時 即送信 ＋ 約0.5s(25*20ms) ハートビート
                let _ = UART_TX_Q.try_send(split::encode_status(cur));
                last_sent = cur;
                heartbeat = 0;
            }
        }
        // ラッチ: シフトが立ったら最低 4tick(=200ms) 保持して表示する。
        if cur != 0 {
            latch = cur;
            hold = 3; // 60ms 保持（一瞬の濁音同時押しも見える最小限）
        } else if hold > 0 {
            hold -= 1;
        } else {
            latch = 0;
        }
        let cols = status_colors(master, latch);
        ws.write(&[scale_rgb(cols[0], led_brightness), scale_rgb(cols[1], led_brightness)])
            .await;
        ticker.next().await;
    }
}

/// RGB を輝度 b(0..=255) でスケール（255 で素の色）。#12 の LED 輝度設定。
fn scale_rgb(c: RGB8, b: u8) -> RGB8 {
    let s = |v: u8| ((v as u16 * b as u16) / 255) as u8;
    RGB8::new(s(c.r), s(c.g), s(c.b))
}

/// OverrideVal を static アリーナ上の `Action`（'static 参照）へ変換する（#12 フェーズ2）。
/// 容量超過時は `Action::None`。boot で一度だけ呼ぶ前提（raw ポインタで 'static 構築）。
unsafe fn oval_to_action(
    v: &naginata_core::config::OverrideVal,
    bytes: *mut u8,
    bcap: usize,
    bpos: &mut usize,
    kps: *mut nghid::KeyPress,
    kcap: usize,
    kpos: &mut usize,
) -> Action {
    use naginata_core::config::OverrideVal;
    match v {
        OverrideVal::Kana(s) => {
            let b = s.as_bytes();
            if *bpos + b.len() > bcap {
                return Action::None;
            }
            core::ptr::copy_nonoverlapping(b.as_ptr(), bytes.add(*bpos), b.len());
            let sl = core::slice::from_raw_parts(bytes.add(*bpos) as *const u8, b.len());
            *bpos += b.len();
            Action::Kana(core::str::from_utf8_unchecked(sl))
        }
        OverrideVal::Keys(kv) => {
            let n = kv.len();
            if *kpos + n > kcap {
                return Action::None;
            }
            for (i, &(u, m)) in kv.iter().enumerate() {
                *kps.add(*kpos + i) = nghid::KeyPress { usage: u, modifiers: m };
            }
            let sl = core::slice::from_raw_parts(kps.add(*kpos) as *const nghid::KeyPress, n);
            *kpos += n;
            Action::Keys(sl)
        }
    }
}

/// 起動時に Config の全カテゴリ差分を `static` アリーナへ展開し、engine 用 `&'static Overlay` を返す。
/// boot で **一度だけ** 呼ぶ前提（static の単一初期化）。
fn build_overlay(cfg: &Config) -> &'static keymap::Overlay {
    use naginata_core::config as cfgmod;
    const BCAP: usize = 1024; // かなバイトアリーナ
    const KCAP: usize = 512; // KeyPress アリーナ
    static mut BYTES: [u8; BCAP] = [0u8; BCAP];
    static mut KPS: [nghid::KeyPress; KCAP] = [nghid::KeyPress { usage: 0, modifiers: 0 }; KCAP];
    static mut OV_LAYERS: [(u16, Action); cfgmod::MAX_LAYERS] =
        [(0u16, Action::None); cfgmod::MAX_LAYERS];
    static mut OV_COMBO2: [(u16, Action); cfgmod::MAX_COMBO2] =
        [(0u16, Action::None); cfgmod::MAX_COMBO2];
    static mut OV_COMBO3: [(u32, Action); cfgmod::MAX_COMBO3] =
        [(0u32, Action::None); cfgmod::MAX_COMBO3];
    static mut OV_MODES: [(u16, Action); cfgmod::MAX_MODES] =
        [(0u16, Action::None); cfgmod::MAX_MODES];
    static mut OVERLAY: keymap::Overlay = keymap::Overlay {
        layers: &[],
        combo2: &[],
        combo3: &[],
        modes: &[],
    };

    let mut bpos = 0usize;
    let mut kpos = 0usize;
    unsafe {
        let bytes = core::ptr::addr_of_mut!(BYTES) as *mut u8;
        let kps = core::ptr::addr_of_mut!(KPS) as *mut nghid::KeyPress;

        // layers
        let lp = core::ptr::addr_of_mut!(OV_LAYERS) as *mut (u16, Action);
        let mut nl = 0usize;
        for (mask, sc, v) in &cfg.layers {
            if nl >= cfgmod::MAX_LAYERS {
                break;
            }
            let a = oval_to_action(v, bytes, BCAP, &mut bpos, kps, KCAP, &mut kpos);
            *lp.add(nl) = (keymap::layer_key(*mask, *sc), a);
            nl += 1;
        }
        // combo2
        let c2 = core::ptr::addr_of_mut!(OV_COMBO2) as *mut (u16, Action);
        let mut n2 = 0usize;
        for (k, v) in &cfg.combo2 {
            if n2 >= cfgmod::MAX_COMBO2 {
                break;
            }
            let a = oval_to_action(v, bytes, BCAP, &mut bpos, kps, KCAP, &mut kpos);
            *c2.add(n2) = (keymap::combo2_key(k[0], k[1]), a);
            n2 += 1;
        }
        // combo3
        let c3 = core::ptr::addr_of_mut!(OV_COMBO3) as *mut (u32, Action);
        let mut n3 = 0usize;
        for (k, v) in &cfg.combo3 {
            if n3 >= cfgmod::MAX_COMBO3 {
                break;
            }
            let a = oval_to_action(v, bytes, BCAP, &mut bpos, kps, KCAP, &mut kpos);
            *c3.add(n3) = (keymap::combo3_key(k[0], k[1], k[2]), a);
            n3 += 1;
        }
        // modes
        let mp = core::ptr::addr_of_mut!(OV_MODES) as *mut (u16, Action);
        let mut nm = 0usize;
        for (mode, sc, v) in &cfg.modes {
            if nm >= cfgmod::MAX_MODES {
                break;
            }
            let a = oval_to_action(v, bytes, BCAP, &mut bpos, kps, KCAP, &mut kpos);
            *mp.add(nm) = (keymap::layer_key(*mode, *sc), a);
            nm += 1;
        }

        let ov = core::ptr::addr_of_mut!(OVERLAY);
        (*ov).layers = core::slice::from_raw_parts(lp as *const (u16, Action), nl);
        (*ov).combo2 = core::slice::from_raw_parts(c2 as *const (u16, Action), n2);
        (*ov).combo3 = core::slice::from_raw_parts(c3 as *const (u32, Action), n3);
        (*ov).modes = core::slice::from_raw_parts(mp as *const (u16, Action), nm);
        &*ov
    }
}

/// 起動時に Config.extra（未使用物理キーの直接割当, #12 フェーズ3）を `static` アリーナへ展開し、
/// `(pack_pos(hand,row,col), Action)` の `&'static` スライスを返す。boot で一度だけ。
fn build_direct(cfg: &Config) -> &'static [(u16, Action)] {
    use naginata_core::config as cfgmod;
    const BCAP: usize = 256;
    const KCAP: usize = 256;
    static mut BYTES: [u8; BCAP] = [0u8; BCAP];
    static mut KPS: [nghid::KeyPress; KCAP] = [nghid::KeyPress { usage: 0, modifiers: 0 }; KCAP];
    static mut TABLE: [(u16, Action); cfgmod::MAX_EXTRA] = [(0u16, Action::None); cfgmod::MAX_EXTRA];
    let mut bpos = 0usize;
    let mut kpos = 0usize;
    let mut n = 0usize;
    unsafe {
        let bytes = core::ptr::addr_of_mut!(BYTES) as *mut u8;
        let kps = core::ptr::addr_of_mut!(KPS) as *mut nghid::KeyPress;
        let tp = core::ptr::addr_of_mut!(TABLE) as *mut (u16, Action);
        for (hand, row, col, v) in &cfg.extra {
            if n >= cfgmod::MAX_EXTRA {
                break;
            }
            let a = oval_to_action(v, bytes, BCAP, &mut bpos, kps, KCAP, &mut kpos);
            let h = if *hand == 0 { Hand::Left } else { Hand::Right };
            *tp.add(n) = (pack_pos(h, *row, *col), a);
            n += 1;
        }
        core::slice::from_raw_parts(tp as *const (u16, Action), n)
    }
}

/// ステータス → 2個のWS2812B 色（両LED同じ＝LED1個でもシフトが見える）。
/// シフトなし=役割色（マスタ緑/スレーブ青）、シフト中=シフト色。
fn status_colors(master: bool, s: u8) -> [RGB8; 2] {
    let role = if master {
        RGB8::new(0, 24, 0) // 緑: マスタ
    } else {
        RGB8::new(0, 0, 24) // 青: スレーブ
    };
    let c = if s != 0 { shift_color(s) } else { role };
    [c, c]
}

/// シフトマスク → 色（センター=シアン, 濁音=赤, 半濁音=黄, 小書き=青）。
fn shift_color(s: u8) -> RGB8 {
    let (mut r, mut g, mut b) = (0u8, 0u8, 0u8);
    if s & SH_DAKUTEN != 0 {
        r = 40; // 濁音 → 赤
    }
    if s & SH_HANDAKU != 0 {
        r = 40;
        g = 32; // 半濁音 → 黄
    }
    if s & SH_CENTER != 0 {
        g = 32;
        b = 40; // センター → シアン
    }
    if s & SH_SMALL != 0 {
        b = 40; // 小書き → 青
    }
    RGB8::new(r, g, b)
}

// ===========================================================================
// USB HID（マスタのみ実効。列挙状態で USB_CONFIGURED を更新し役割を決める）
// ===========================================================================

/// USB列挙状態を USB_CONFIGURED へ反映する Handler。
struct StateHandler;
impl Handler for StateHandler {
    fn configured(&mut self, configured: bool) {
        USB_CONFIGURED.store(configured, Ordering::Relaxed);
    }
    fn reset(&mut self) {
        USB_CONFIGURED.store(false, Ordering::Relaxed);
    }
    fn suspended(&mut self, suspended: bool) {
        if suspended {
            USB_CONFIGURED.store(false, Ordering::Relaxed);
        }
    }
}

#[embassy_executor::task]
async fn usb_task(driver: Driver<'static, USB>, mut flash: handedness::HandFlash<'static>, initial_cfg: Config) {
    let mut config = UsbConfig::new(0xc0de, 0xcafe);
    config.manufacturer = Some("Siska Tech Lab.");
    config.product = Some("Naginata Keyboard");
    config.serial_number = Some("0001");
    config.max_power = 100;
    config.max_packet_size_0 = 64;

    let mut config_descriptor = [0u8; 256];
    let mut bos_descriptor = [0u8; 256];
    let mut msos_descriptor = [0u8; 0];
    let mut control_buf = [0u8; 64];
    let mut kbd_state = State::new();
    let mut cfg_state = State::new();
    let mut handler = StateHandler;

    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buf,
    );
    builder.handler(&mut handler);

    // IF0: HID キーボード（既存・出力専用, 1000Hz）。
    let kbd_config = hid::Config {
        report_descriptor: usb_hid::KEYBOARD_REPORT_DESCRIPTOR,
        request_handler: None,
        poll_ms: 1, // 1000Hz: ローマ字の多打鍵でも遅延を最小化（NFR-1）
        max_packet_size: 8,
        hid_subclass: hid::HidSubclass::No,
        hid_boot_protocol: hid::HidBootProtocol::None,
    };
    let mut writer = HidWriter::<_, 8>::new(&mut builder, &mut kbd_state, kbd_config);

    // IF1: vendor HID 設定チャネル（#12, VIA 類似, 32B in/out）。
    let cfg_hid_config = hid::Config {
        report_descriptor: usb_config::VENDOR_REPORT_DESCRIPTOR,
        request_handler: None,
        poll_ms: 10, // 設定は低頻度で十分
        max_packet_size: usb_config::REPORT_LEN as u16,
        hid_subclass: hid::HidSubclass::No,
        hid_boot_protocol: hid::HidBootProtocol::None,
    };
    let cfg_rw =
        HidReaderWriter::<_, { usb_config::REPORT_LEN }, { usb_config::REPORT_LEN }>::new(
            &mut builder,
            &mut cfg_state,
            cfg_hid_config,
        );

    let mut usb = builder.build();
    let usb_fut = usb.run();
    let hid_fut = async {
        loop {
            let report = REPORTS.receive().await;
            let _ = writer.write(&report).await;
        }
    };
    // 設定チャネル: OUT を受信 → コマンド処理（ステージング更新/COMMIT/RESET）→ IN で応答。
    let (mut cfg_reader, mut cfg_writer) = cfg_rw.split();
    let mut staged = initial_cfg;
    let cfg_fut = async {
        let mut buf = [0u8; usb_config::REPORT_LEN];
        loop {
            if cfg_reader.read(&mut buf).await.is_err() {
                continue;
            }
            let outcome = usb_config::handle_command(&buf, &mut staged, &mut flash);
            let _ = cfg_writer.write(&outcome.resp).await;
            if outcome.reboot {
                Timer::after(Duration::from_millis(50)).await; // 応答送出を待つ
                cortex_m::peripheral::SCB::sys_reset();
            }
        }
    };
    join3(usb_fut, hid_fut, cfg_fut).await;
}

// ===========================================================================
// 分割UART
// ===========================================================================

/// 送信: UART_TX_Q のフレームを相方へ送る。
#[embassy_executor::task]
async fn uart_tx_task(mut tx: BufferedUartTx) {
    loop {
        let frame = UART_TX_Q.receive().await;
        let _ = tx.write(&frame).await;
    }
}

/// 受信: 相方のキーイベントを復号し、相方手番(opposite)で ENGINE_IN へ。
#[embassy_executor::task]
async fn uart_rx_task(mut rx: BufferedUartRx, local_hand: Hand) {
    let mut b = [0u8; 1];
    loop {
        // 同期: 先頭バイト(bit7=1)を待つ（§C.3 再同期）
        if rx.read(&mut b).await.is_err() {
            continue;
        }
        let b0 = b[0];
        if b0 & 0x80 == 0 {
            continue; // 同期外れ → 読み捨て
        }
        if rx.read(&mut b).await.is_err() {
            continue;
        }
        match split::decode(b0, b[0]) {
            // 相方のキーイベント → 相方手番(opposite)で engine へ
            Some(split::Frame::Key(ev)) => {
                let _ = ENGINE_IN.try_send(MatrixEv {
                    hand: local_hand.opposite(),
                    row: ev.row,
                    col: ev.col,
                    pressed: ev.pressed,
                });
            }
            // マスタからのシフト状態 → スレーブのLED用
            Some(split::Frame::Status(mask)) => {
                REMOTE_SHIFTS.store(mask, Ordering::Relaxed);
            }
            None => {} // 同期外れ
        }
    }
}

// ===========================================================================
// スキャン: 自半身の変化を ① 相方へ送信 ② 自分の engine 入力 へ
// ===========================================================================

#[embassy_executor::task]
async fn scan_task(mut matrix: Matrix<'static>, local_hand: Hand) {
    loop {
        matrix
            .scan(&mut |row, col, pressed| {
                if RAW_MATRIX_TEST {
                    if pressed {
                        emit_raw_position(row, col);
                    }
                    return;
                }
                // ① 相方へ送信（スレーブ時の主目的。マスタ時は相方が無視）
                let _ = UART_TX_Q.try_send(KeyEvent { row, col, pressed }.encode());
                // ② 自半身として engine 入力へ（マスタのみ engine_task が処理）
                let _ = ENGINE_IN.try_send(MatrixEv {
                    hand: local_hand,
                    row,
                    col,
                    pressed,
                });
            })
            .await;
        Timer::after(Duration::from_millis(1)).await; // FR-1.2
    }
}

// ===========================================================================
// エンジン: マスタ時のみ、統合イベントを薙刀式変換して HID へ
// ===========================================================================

#[embassy_executor::task]
async fn engine_task(
    window_ms: u32,
    repeat_delay_ms: u32,
    repeat_interval_ms: u32,
    overlay: &'static keymap::Overlay,
    direct: &'static [(u16, Action)],
) {
    let mut engine = Engine::new();
    // #12: flash 設定（パラメータ＋全カテゴリ オーバーレイ）を起動時に適用。
    engine.set_window(window_ms);
    engine.set_repeat(repeat_delay_ms, repeat_interval_ms);
    engine.set_overlay(overlay);
    let mut ticker = Ticker::every(Duration::from_millis(5));
    loop {
        match select(ENGINE_IN.receive(), ticker.next()).await {
            Either::First(ev) => {
                if !USB_CONFIGURED.load(Ordering::Relaxed) {
                    continue; // スレーブ: 自分では変換しない
                }
                let now = Instant::now().as_millis() as u32;
                // learn-key(#12): 押下のたびに最後の物理位置を記録（未使用キー同定用）。
                if ev.pressed {
                    LAST_KEY.store(pack_pos(ev.hand, ev.row, ev.col) | 0x8000, Ordering::Relaxed);
                }
                if let Some(sc) = keymap::matrix_to_sc(ev.hand, ev.row, ev.col) {
                    let mut sink = |a: Action| emit_action(a);
                    if ev.pressed {
                        engine.press(sc, now, &mut sink);
                    } else {
                        engine.release(sc, now, &mut sink);
                    }
                    HELD_SHIFTS.store(engine.display_shifts(), Ordering::Relaxed); // LED用
                } else if ev.pressed {
                    // 薙刀式 sc が無い物理キー → 直接キー割当(#12 フェーズ3, タップ出力)。
                    let pk = pack_pos(ev.hand, ev.row, ev.col);
                    if let Some((_, a)) = direct.iter().find(|(k, _)| *k == pk) {
                        emit_action(*a);
                    }
                }
            }
            Either::Second(_) => {
                if !USB_CONFIGURED.load(Ordering::Relaxed) {
                    continue;
                }
                let now = Instant::now().as_millis() as u32;
                engine.tick(now, &mut |a| emit_action(a));
            }
        }
    }
}

/// Action → HID キーストローク（押下/解放レポート）→ チャネル送出。
fn emit_action(action: Action) {
    match action {
        Action::Kana(kana) => {
            let romaji = romaji::kana_to_romaji::<16>(kana);
            nghid::romaji_to_hid(romaji.as_str(), &mut send_stroke);
        }
        Action::Keys(keys) => {
            for &kp in keys {
                send_stroke(kp);
            }
        }
        Action::None => {}
    }
}

/// 1キーの押下→解放を1ストロークとしてチャネルへ送る。
fn send_stroke(kp: nghid::KeyPress) {
    let _ = REPORTS.try_send(usb_hid::report_for(kp));
    let _ = REPORTS.try_send(usb_hid::release_report());
}

// --- 生テスト（RAW_MATRIX_TEST）-------------------------------------------
fn emit_raw_position(row: u8, col: u8) {
    let mut put = |c: char| {
        if let Some(kp) = nghid::ascii_to_hid(c) {
            send_stroke(kp);
        }
    };
    put('r');
    emit_number(row, &mut put);
    put('c');
    emit_number(col, &mut put);
    put(' ');
}

fn emit_number(mut n: u8, put: &mut dyn FnMut(char)) {
    if n >= 10 {
        put((b'0' + n / 10) as char);
        n %= 10;
    }
    put((b'0' + n) as char);
}
