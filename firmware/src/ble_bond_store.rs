//! BLE bond information flash store (#07).
//!
//! PoC scope: one bonded host, one 4KB sector. The active `trouble-host` 0.6 API exposes
//! LESC LTK + peer identity, but not EDIV/Rand, so this stores exactly what the stack can restore.

use bt_hci::param::BdAddr;
use embassy_rp::flash::ERASE_SIZE;
use trouble_host::prelude::{Address, BondInformation, Identity, SecurityLevel};
use trouble_host::{IdentityResolvingKey, LongTermKey};

use crate::handedness::{HandFlash, FLASH_SIZE};

const BOND_OFFSET: u32 = (FLASH_SIZE - 3 * ERASE_SIZE) as u32;
const MAGIC: [u8; 4] = *b"NGBD";
const VERSION: u8 = 3;
const RECORD_LEN: usize = 64;
const FLASH_WRITE_LEN: usize = 256;

const OFF_MAGIC: usize = 0;
const OFF_VERSION: usize = 4;
const OFF_FLAGS: usize = 5;
const OFF_SECURITY: usize = 6;
const OFF_ADDR: usize = 7;
const OFF_LTK: usize = 13;
const OFF_IRK: usize = 29;
const OFF_LOCAL_ADDR: usize = 45;
const OFF_CCCD: usize = 51;

const FLAG_BONDED: u8 = 0x01;
const FLAG_HAS_IRK: u8 = 0x02;

pub struct StoredBond {
    pub local_address: Address,
    pub bond: BondInformation,
    pub cccd: [u16; 3],
}

pub fn read_bond(flash: &mut HandFlash) -> Option<StoredBond> {
    let mut buf = [0u8; RECORD_LEN];
    flash.blocking_read(BOND_OFFSET, &mut buf).ok()?;
    if buf[OFF_MAGIC..OFF_MAGIC + 4] != MAGIC || buf[OFF_VERSION] != VERSION {
        return None;
    }

    let flags = buf[OFF_FLAGS];
    let security_level = match buf[OFF_SECURITY] {
        1 => SecurityLevel::Encrypted,
        2 => SecurityLevel::EncryptedAuthenticated,
        _ => return None,
    };

    let bd_addr = BdAddr::new(buf[OFF_ADDR..OFF_ADDR + 6].try_into().ok()?);
    let ltk = LongTermKey::from_le_bytes(buf[OFF_LTK..OFF_LTK + 16].try_into().ok()?);
    let irk = if flags & FLAG_HAS_IRK != 0 {
        Some(IdentityResolvingKey::from_le_bytes(
            buf[OFF_IRK..OFF_IRK + 16].try_into().ok()?,
        ))
    } else {
        None
    };

    let local_address_bytes: [u8; 6] = buf[OFF_LOCAL_ADDR..OFF_LOCAL_ADDR + 6].try_into().ok()?;
    if local_address_bytes == [0; 6]
        || local_address_bytes == [0xff; 6]
        || (local_address_bytes[5] & 0xc0) != 0xc0
    {
        return None;
    }
    let local_address = Address::random(local_address_bytes);
    let bond = BondInformation::new(
        Identity { bd_addr, irk },
        ltk,
        security_level,
        flags & FLAG_BONDED != 0,
    );
    let cccd = [
        u16::from_le_bytes(buf[OFF_CCCD..OFF_CCCD + 2].try_into().ok()?),
        u16::from_le_bytes(buf[OFF_CCCD + 2..OFF_CCCD + 4].try_into().ok()?),
        u16::from_le_bytes(buf[OFF_CCCD + 4..OFF_CCCD + 6].try_into().ok()?),
    ];
    Some(StoredBond {
        local_address,
        bond,
        cccd,
    })
}

pub fn write_bond(
    flash: &mut HandFlash,
    local_address: Address,
    bond: &BondInformation,
    cccd: [u16; 3],
) -> Result<(), ()> {
    let mut buf = [0xFFu8; FLASH_WRITE_LEN];
    buf[OFF_MAGIC..OFF_MAGIC + 4].copy_from_slice(&MAGIC);
    buf[OFF_VERSION] = VERSION;
    buf[OFF_FLAGS] = if bond.is_bonded { FLAG_BONDED } else { 0 };
    buf[OFF_SECURITY] = match bond.security_level {
        SecurityLevel::NoEncryption => return Err(()),
        SecurityLevel::Encrypted => 1,
        SecurityLevel::EncryptedAuthenticated => 2,
    };
    buf[OFF_ADDR..OFF_ADDR + 6].copy_from_slice(&bond.identity.bd_addr.into_inner());
    buf[OFF_LTK..OFF_LTK + 16].copy_from_slice(&bond.ltk.to_le_bytes());
    buf[OFF_LOCAL_ADDR..OFF_LOCAL_ADDR + 6].copy_from_slice(&local_address.addr.into_inner());
    for (i, value) in cccd.iter().enumerate() {
        let offset = OFF_CCCD + i * 2;
        buf[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
    }
    if let Some(irk) = bond.identity.irk {
        buf[OFF_FLAGS] |= FLAG_HAS_IRK;
        buf[OFF_IRK..OFF_IRK + 16].copy_from_slice(&irk.to_le_bytes());
    }

    flash
        .blocking_erase(BOND_OFFSET, BOND_OFFSET + ERASE_SIZE as u32)
        .map_err(|_| ())?;
    flash.blocking_write(BOND_OFFSET, &buf).map_err(|_| ())
}

pub fn erase_bond(flash: &mut HandFlash) -> Result<(), ()> {
    flash
        .blocking_erase(BOND_OFFSET, BOND_OFFSET + ERASE_SIZE as u32)
        .map_err(|_| ())
}
