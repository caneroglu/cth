//! AT24C256 — 32 KB I2C EEPROM. Kalibrasyon kayitlarini tutar.
//!
//! Cihaz adresi 0x57 (A2A1A0 = 111). 2 baytlik bellek adresi.
//! I2C bus'i OLED ile PAYLASILIYOR (ikisi de I2C1 / PB8-PB9), o yuzden bu
//! surucu bus'i SAHIPLENMEZ — cagiran taraf `&mut I2c` verir. Task'lar
//! devreye girdiginde bus bir mutex arkasina alinacak.

use embassy_stm32::i2c::{I2c, Master};
use embassy_stm32::mode::Blocking;

/// I2C cihaz adresi.
pub const DEV_ADDR: u8 = 0x57;

/// Kalibrasyon kaydinin durdugu adres (en guncel kayit).
pub const CALIB_ADDR: u16 = 0x0000;

/// Genesis mesajinin adresi
pub const GENESIS_ADDR: u16 = 0x7000;

type Bus<'d> = I2c<'d, Blocking, Master>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, defmt::Format)]
pub enum CalibError {
    /// I2C okumasi basarisiz — cip yok, bus kilitli, ya da kablo problemi.
    Bus,
    /// Magic uyusmadi: bu adreste kalibrasyon kaydi yok (ya da EEPROM bos).
    BadMagic(u32),
    /// Magic dogru ama surum taninmiyor.
    UnknownVersion(u16),
    /// Kayit icindeki bir alan mantiksiz (mux kanali bos, DAC kodu tasmis...).
    Invalid,
}

/// EEPROM'daki v1 kalibrasyon kaydi (32 bayt, little-endian, packed).
///
#[derive(Debug, Clone, Copy, defmt::Format)]
pub struct CalibV1 {
    pub version: u16,
    /// Host'tan alinmis UNIX epoch (kalibrasyonun yapildigi an).
    pub timestamp_utc: u64,
    /// Kalibrasyon anindaki sicaklik, x10 (DS18B20).
    pub temp_c_x10: i16,
    pub rsense_ohms: u16,
    /// Z1 icin DAC kodu.
    pub dac_z1: u16,
    /// Z2 icin DAC kodu.
    pub dac_z2: u16,
    /// Z1 icin mux kazanc kanali.
    pub mux_z1: u8,
    /// Z2 icin mux kazanc kanali.
    pub mux_z2: u8,
    /// Kalibrasyon aninda olculen SHANNON entropisi (millibit). Referans icin.
    pub shannon_z1_mb: u32,
    pub shannon_z2_mb: u32,
}

/// "QRNG" ASCII, big-endian okunusla 0x51524E47.
pub const MAGIC: u32 = 0x5152_4E47;
pub const VERSION_1_0: u16 = 0x0100;
pub const VERSION_1_1: u16 = 0x0101;

const RECORD_LEN: usize = 32;

#[inline]
fn u16_le(b: &[u8]) -> u16 {
    u16::from_le_bytes([b[0], b[1]])
}
#[inline]
fn u32_le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}
#[inline]
fn u64_le(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Ham baytlari kayda cevirir.
///
/// Alan alan, acik little-endian okuma yapiyoruz — `repr(C, packed)` bir
/// struct'a `copy_nonoverlapping` ile bakmak yerine. Sebep: packed struct
/// alanlarina referans almak hizalama acisindan tehlikeli, ayrica offset'ler
/// boyle yazildiginda SEMA KODUN ICINDE OKUNABILIR halde duruyor.
///
/// Offset haritasi (v1, toplam 32 bayt):
///   0..4   magic          u32
///   4..6   version        u16
///   6..14  timestamp_utc  u64
///   14..16 temp_c_x10     i16
///   16..18 rsense_ohms    u16
///   18..20 dac_z1         u16
///   20..24 shannon_z1_mb  u32
///   24..26 dac_z2         u16
///   26..30 shannon_z2_mb  u32
///   30     mux_z1         u8
///   31     mux_z2         u8
pub fn parse_v1(buf: &[u8]) -> Result<CalibV1, CalibError> {
    if buf.len() < RECORD_LEN {
        return Err(CalibError::Invalid);
    }

    let magic = u32_le(&buf[0..4]);
    if magic != MAGIC {
        return Err(CalibError::BadMagic(magic));
    }

    // Eski firmware surumu HIC kontrol etmiyordu (sadece magic'e bakiyordu),
    // oysa iki farkli surum tanimliydi. v1.0 layout'unu v1.1 sanip okumak
    // sessizce kaymis alanlar demek.
    let version = u16_le(&buf[4..6]);
    if version != VERSION_1_0 && version != VERSION_1_1 {
        return Err(CalibError::UnknownVersion(version));
    }

    let rec = CalibV1 {
        version,
        timestamp_utc: u64_le(&buf[6..14]),
        temp_c_x10: u16_le(&buf[14..16]) as i16,
        rsense_ohms: u16_le(&buf[16..18]),
        dac_z1: u16_le(&buf[18..20]),
        shannon_z1_mb: u32_le(&buf[20..24]),
        dac_z2: u16_le(&buf[24..26]),
        shannon_z2_mb: u32_le(&buf[26..30]),
        mux_z1: buf[30],
        mux_z2: buf[31],
    };

    // Mantik kontrolu: DAC 12 bit, mux kanali fiziksel olarak bagli olmali.
    if rec.dac_z1 > 4095 || rec.dac_z2 > 4095 {
        return Err(CalibError::Invalid);
    }
    if !super::hc4067::is_connected(rec.mux_z1) || !super::hc4067::is_connected(rec.mux_z2) {
        return Err(CalibError::Invalid);
    }

    Ok(rec)
}

/// EEPROM'dan v1 kalibrasyon kaydini okur.
pub fn read_calib_v1(i2c: &mut Bus<'_>) -> Result<CalibV1, CalibError> {
    let mut buf = [0u8; RECORD_LEN];
    read_bytes(i2c, CALIB_ADDR, &mut buf)?;
    parse_v1(&buf)
}

/// Rastgele adresten okuma: 2 baytlik adres yaz, ardindan oku.
pub fn read_bytes(i2c: &mut Bus<'_>, addr: u16, out: &mut [u8]) -> Result<(), CalibError> {
    let a = [(addr >> 8) as u8, (addr & 0xFF) as u8];
    i2c.blocking_write_read(DEV_ADDR, &a, out)
        .map_err(|_| CalibError::Bus)
}
