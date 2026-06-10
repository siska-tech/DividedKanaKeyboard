/* RP2040 メモリ配置（embassy-rp 例準拠）
 * 先頭 256B は第2段ブートローダ(BOOT2)。フラッシュは 2MB（Pico/Pico W）。*/
MEMORY {
    BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
    FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
    RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}

/* EE_HANDS 等の不揮発データ用に末尾セクタを温存したい場合は、
 * FLASH LENGTH を 4K 減らし、別領域を切る運用も可（本実装で検討）。*/
