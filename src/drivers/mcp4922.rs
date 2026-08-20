//! MCP4922 — 12 bit, 2 kanalli SPI DAC. Zener bias akimini belirler.
//! VCCS (voltaj kontrollu akim kaynagi) topolojisi:
//!
//!   DAC --> op-amp (+)          I = V_DAC / R_sense
//!            op-amp (-) <-- R_sense
//!
//!   V_DAC = (kod / 4096) * V_REF
//!
//! Parametreler (donanimdan):
//!   V_REF   = 2.500 V   (MCP4922 dahili referans)
//!   R_sense = 1 kOhm
//!   V_Z     = 2.4 V     (zener breakdown)
//!   V_CC    = 5.0 V     -> op-amp cikis marji 2.6 V > 2.5 V, yeterli
//!
//! Akim araligi:
//!   kod=0     -> 0 uA
//!   kod=1638  -> ~1.0 mA   (tunelleme icin hedeflenen bolge)
//!   kod=4095  -> 2.5 mA    (V_REF limitli)
//!
//! Cozunurluk: 1 adim = 2500/4096 = 0.61 uA
//!
//! DIKKAT — bu cip OKUNAMAZ (write-only). Cipteki degeri sorgulamak mumkun
//! degil, o yuzden surucu yazdigi degeri kendisi tutuyor (`z1` / `z2`).
//! Rapor/telemetri o alanlardan okunmali; "DAC'a sordum" diye bir sey yok.

use embassy_stm32::gpio::Output;
use embassy_stm32::mode::Blocking;
use embassy_stm32::spi::{Spi, mode::Master};

/// Referans voltaji (mV) — akim hesabinda kullanilir.
pub const V_REF_MV: u32 = 2500;
/// Akim olcum direnci (ohm).
pub const R_SENSE_OHMS: u32 = 1000;
/// DAC tam olcek (12 bit).
pub const DAC_FULL_SCALE: u32 = 4096;

/// Hangi zener / hangi DAC kanali.
#[derive(Copy, Clone, PartialEq, Eq, defmt::Format)]
pub enum Zener {
    /// DAC A -> Zener 1 -> ADC1 / PA0
    Z1,
    /// DAC B -> Zener 2 -> ADC2 / PA1
    Z2,
}

/// DAC kodunu mikroampere cevirir.
///
/// I = (kod / 4096) * V_REF / R_sense
/// uA olarak: kod * 2500 / 4096   (R_sense = 1k oldugu icin mV = uA)
pub const fn code_to_ua(code: u16) -> u32 {
    (code as u32 * V_REF_MV) / DAC_FULL_SCALE
}

/// (mA tam kisim, yuzdelik kesir) — gosterim icin.
pub const fn code_to_ma_frac(code: u16) -> (u32, u32) {
    let ua = code_to_ua(code);
    (ua / 1000, (ua % 1000) / 10)
}

pub struct Mcp4922<'d> {
    spi: Spi<'d, Blocking, Master>,
    cs: Output<'d>,
    ldac: Output<'d>,
    /// Yazilan son degerler — cip okunamadigi icin tek gercek kaynak burasi.
    z1: u16,
    z2: u16,
}

impl<'d> Mcp4922<'d> {
    /// Her iki kanali 0'da baslatir.
    ///
    /// Neden 0: acilista zenerlere akim vermek istemiyoruz. Kalibrasyon
    /// degerleri EEPROM'dan okunduktan sonra bilincli olarak ayarlanacak.
    pub fn new(spi: Spi<'d, Blocking, Master>, cs: Output<'d>, ldac: Output<'d>) -> Self {
        let mut dac = Self {
            spi,
            cs,
            ldac,
            z1: 0,
            z2: 0,
        };
        dac.set(Zener::Z1, 0);
        dac.set(Zener::Z2, 0);
        dac
    }

    /// 16 bitlik komutu SPI'ya basar ve LDAC ile cikisa aktarir.
    ///
    /// Komut bitleri:
    ///   b15 : kanal   (0 = DAC_A, 1 = DAC_B)
    ///   b14 : buffer  (0 = unbuffered)
    ///   b13 : kazanc  (1 = 1x)
    ///   b12 : shutdown(1 = aktif)
    ///   b11..b0 : 12 bit veri
    fn write_cmd(&mut self, cmd: u16) {
        let bytes = [(cmd >> 8) as u8, (cmd & 0xFF) as u8];

        self.cs.set_low();
        let _ = self.spi.blocking_write(&bytes);
        self.cs.set_high();

        // LDAC dusen kenari: shift register'daki degeri cikisa aktarir.
        // Iki nop, MCP4922'nin minimum LDAC darbe genisligini (100 ns)
        // 72 MHz'de saglamak icin — 1 cevrim ~13.9 ns, 2 nop + gpio gecikmesi
        // rahat asiyor.
        self.ldac.set_low();
        cortex_m::asm::nop();
        cortex_m::asm::nop();
        self.ldac.set_high();
    }

    /// Kanali ayarla (0..4095; ustu kirpilir).
    pub fn set(&mut self, z: Zener, code: u16) {
        let code = code.min(4095);
        match z {
            // b15=0 (A), b14=0, b13=1 (1x), b12=1 (aktif) => 0x3000
            Zener::Z1 => {
                self.write_cmd(0x3000 | code);
                self.z1 = code;
            }
            // b15=1 (B), b14=0, b13=1, b12=1 => 0xB000
            Zener::Z2 => {
                self.write_cmd(0xB000 | code);
                self.z2 = code;
            }
        }
    }

    /// Yazilmis son kod.
    pub fn code(&self, z: Zener) -> u16 {
        match z {
            Zener::Z1 => self.z1,
            Zener::Z2 => self.z2,
        }
    }

    /// Yazilmis son kodun akim karsiligi (uA).
    pub fn current_ua(&self, z: Zener) -> u32 {
        code_to_ua(self.code(z))
    }

    /// Iki kanali da sifirla (zenerlerden akimi kes).
    pub fn shutdown(&mut self) {
        self.set(Zener::Z1, 0);
        self.set(Zener::Z2, 0);
    }
}
