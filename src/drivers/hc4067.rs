//!!! Ekstra dirençler TAK, yer var - ara dirençlerde var artık.

//! HC4067 16 kanalli analog mux — 2. kat op-amp kazanc kontrolu
//!
//! Devre topolojisi:
//!
//!   zener --> [1. kat: sabit x51.0] --> [2. kat: R_f/R_in] --> ADC
//!                                          ^
//!                                          |
//!                              R_f = 2k0 (base) + mux ile secilen ek direnc
//!
//! Mux, 2. kat geri besleme direncine paralel bir direnc bankasi seciyor.
//! Kanal degistirmek kazanci degistirir; kazanc, zener gurultusunu ADC'nin
//! tam olcegine oturtmak icin kalibre edilir.

use embassy_stm32::gpio::Output;

/// Giris direnci — iki zener icin de ayni, sabit.
pub const R_IN_OHMS: u16 = 470;

/// Geri besleme direncinin baz degeri (mux CH0 = bypass).
pub const R_F_BASE_OHMS: u16 = 2000;

/// 1. kat kazanci x10 (sabit, her iki zener icin ayni) => 51.0x
pub const STAGE1_GAIN_X10: u16 = 510;

/// Kanal -> ek geri besleme direnci.
/// `None` = O KANALDA FIZIKSEL DIRENC YOK (not connected).
/// `Option` ile o hata artik sessiz kalamaz: `rf_total()` `None` doner,
/// cagiran taraf ele almak ZORUNDA.
/// Bagli kanallar: 0, 1, 2, 3, 5, 7, 10, 11, 14  (9 adet)
/// Bos kanallar  : 4, 6, 8, 9, 12, 13, 15
pub const GAIN_TABLE: [Option<u16>; 16] = [
    Some(0),     // CH0  -> R_f = 2.00k  (base, bypass)
    Some(220),   // CH1  -> R_f = 2.22k
    Some(470),   // CH2  -> R_f = 2.47k
    Some(1000),  // CH3  -> R_f = 3.00k
    None,        // CH4    bos  | hedef ~1.5k  (R_f = 3.5k)
    Some(2200),  // CH5  -> R_f = 4.20k
    None,        // CH6    bos  | hedef ~2.7k  (R_f = 4.7k)
    Some(3300),  // CH7  -> R_f = 5.30k
    None,        // CH8    bos  | hedef ~3.9k  (R_f = 5.9k)
    None,        // CH9    bos  | hedef ~4.3k  (R_f = 6.3k)
    Some(4700),  // CH10 -> R_f = 6.70k
    Some(5100),  // CH11 -> R_f = 7.10k
    None,        // CH12   bos  | hedef ~5.6k  (R_f = 7.6k)
    None,        // CH13   bos  | hedef ~6.2k  (R_f = 8.2k)
    Some(6800),  // CH14 -> R_f = 8.80k
    None,        // CH15   bos  | hedef ~8.2k+ (R_f = 10.2k, maks adim)
];

/// Fiziksel direnci takili kanallar — kalibrasyon taramasi bunlari gezer.
pub const CONNECTED_CHANNELS: &[u8] = &[0, 1, 2, 3, 5, 7, 10, 11, 14];

/// Kanal bagli mi?
pub const fn is_connected(ch: u8) -> bool {
    ch < 16 && GAIN_TABLE[ch as usize].is_some()
}

/// Toplam geri besleme direnci (ohm). Kanal bos ise `None`.
pub fn rf_total(ch: u8) -> Option<u16> {
    if ch >= 16 {
        return None;
    }
    GAIN_TABLE[ch as usize].map(|extra| R_F_BASE_OHMS + extra)
}

/// 2. kat kazanci x100 (tamsayi matematigi). Kanal bos ise `None`.
pub fn stage2_gain_x100(ch: u8) -> Option<u32> {
    rf_total(ch).map(|rf| (rf as u32 * 100) / R_IN_OHMS as u32)
}

/// Toplam kazanc x100 (1. kat x 2. kat). Kanal bos ise `None`.
pub fn total_gain_x100(ch: u8) -> Option<u32> {
    stage2_gain_x100(ch).map(|g2| (STAGE1_GAIN_X10 as u32 * g2) / 10)
}

/// HC4067 surucusu — 4 adres biti + 1 inhibit.
pub struct Hc4067<'d> {
    s0: Output<'d>,
    s1: Output<'d>,
    s2: Output<'d>,
    s3: Output<'d>,
    /// INH pini: HIGH = tum kanallar kapali (yuksek empedans).
    inh: Output<'d>,
    selected: Option<u8>,
}

impl<'d> Hc4067<'d> {
    /// Mux'u KAPALI baslatir.
    pub fn new(
        s0: Output<'d>,
        s1: Output<'d>,
        s2: Output<'d>,
        s3: Output<'d>,
        mut inh: Output<'d>,
    ) -> Self {
        inh.set_high(); // INH=HIGH -> kanallar kapali
        Self {
            s0,
            s1,
            s2,
            s3,
            inh,
            selected: None,
        }
    }

    /// Adres bitlerini kurar (mux'u acmaz).
    fn set_address(&mut self, ch: u8) {
        // HC4067 adresleme: S0 en anlamsiz bit.
        if ch & 0b0001 != 0 { self.s0.set_high() } else { self.s0.set_low() }
        if ch & 0b0010 != 0 { self.s1.set_high() } else { self.s1.set_low() }
        if ch & 0b0100 != 0 { self.s2.set_high() } else { self.s2.set_low() }
        if ch & 0b1000 != 0 { self.s3.set_high() } else { self.s3.set_low() }
    }

    /// Kanal sec ve mux'u ac.
    pub fn select(&mut self, ch: u8) -> Result<(), u8> {
        if !is_connected(ch) {
            return Err(ch);
        }
        self.set_address(ch);
        self.inh.set_low(); // ac
        self.selected = Some(ch);
        Ok(())
    }

    /// Tum kanallari ayir (yuksek empedans).
    pub fn disable(&mut self) {
        self.inh.set_high();
        self.selected = None;
    }

    /// Su an secili kanal (mux kapaliysa `None`).
    pub fn selected(&self) -> Option<u8> {
        self.selected
    }
}
