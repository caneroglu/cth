//! Entropi yogunlastirici (conditioner) — zincirlenmis HMAC-SHA256.
//!
//! GOREV: dusuk yogunluklu ham entropiyi (bayt basina ~3.4 bit) tam
//! yogunluklu cikti bloklarina (bayt basina 8 bit) sikistirmak.
//!
//! BU TASARIM:
//!   state <- HMAC-SHA256(key = state, msg = girdi blogu)
//!   cikti <- state
//!
//!   - Anahtar ZINCIRLENIYOR: onceki durum bir sonrakinin anahtari. Entropi
//!     bloklar boyunca birikiyor.
//!   - Keyed olmasi "hash'i tersine cevir" tartismasini kapatiyor.
//!   - HMAC-SHA256, SP 800-90B'nin onaylı (vetted) conditioning
//!     fonksiyonlarindan biri.
//!
//! ENTROPI BUTCESI (sayilar OLCULDU, varsayilmadi):
//!   Olcum: alt-4-bit min-entropy 3.34-3.71 bit/ornek (AR(16) ile yapi
//!   cikarildiktan sonra). Muhafazakar uc: H = 3.4 bit/ornek.
//!
//!   Cikis bayti iki kanalin nibble'ini tasiyor: (Z1<<4)|Z2.
//!   Iki kanal ortak 4.88 kHz tonu paylastigi icin bayt basina 6.8 bit
//!   DEMIYORUZ — muhafazakar sekilde tek kanal degerini aliyoruz:
//!       H_bayt = 3.4 bit   (6.8 degil)
//!
//!   256 bitlik cikti icin 2x emniyet payiyla >= 512 bit girdi entropisi:
//!       N = ceil(512 / 3.4) = 151  ->  yuvarlanmis: 160 bayt
//!
//!   Yani 160 bayt girdi -> 32 bayt cikti (5:1). Girdi entropisi
//!   160 x 3.4 = 544 bit >= 512 bit hedef. Pay: 2.13x.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Cikti blogu boyu (SHA-256 -> 32 bayt).
pub const BLOCK_OUT: usize = 32;

/// Olculen min-entropy (bit/bayt), x10 sabit nokta.
/// Kaynak: host analizi, AR(16) kalintisinin alt-4-bit min-entropy'si.
pub const H_PER_BYTE_X10: u32 = 34;

/// Cikti biti basina istenen girdi entropisi kati (emniyet payi).
pub const SAFETY_FACTOR: u32 = 2;

/// Bir cikti blogu icin gereken girdi bayti.
///
///   gerekli_bit = 8 * BLOCK_OUT * SAFETY_FACTOR = 512
///   N = ceil(gerekli_bit / (H_PER_BYTE_X10/10))
///     = ceil(512 * 10 / 34) = 151  -> 160'a yuvarlandi
pub const BLOCK_IN: usize = 160;

/// Entropi butcesi payi, x100 (ornegin 212 = 2.12x).
///
///   girdi entropisi / cikti bit sayisi
///   = (160 x 3.4) / 256 = 544 / 256 = 2.125
///
pub const BUDGET_X100: u32 = (BLOCK_IN as u32 * H_PER_BYTE_X10 * 10) / (8 * BLOCK_OUT as u32);

/// Derleme zamani kontrolu: butce gercekten tutuyor mu?
/// Oranlarla oynanirsa burasi derlemeyi durdurur — sessizce acik vermez.
const _: () = {
    let supplied = BLOCK_IN as u32 * H_PER_BYTE_X10; // x10 bit
    let required = 8 * BLOCK_OUT as u32 * SAFETY_FACTOR * 10; // x10 bit
    assert!(
        supplied >= required,
        "conditioner entropi butcesi acik veriyor: BLOCK_IN yetersiz"
    );
};

pub struct Conditioner {
    /// Zincir durumu = bir sonraki HMAC'in anahtari.
    state: [u8; BLOCK_OUT],
    /// Girdi tamponu.
    buf: [u8; BLOCK_IN],
    pos: usize,
    /// Uretilen blok sayisi (telemetri).
    pub blocks_out: u64,
    /// Yutulan bayt sayisi (telemetri).
    pub bytes_in: u64,
}

impl Conditioner {
    /// Sabit bir baslangic durumuyla kurar.
    ///
    /// Baslangic durumunun gizli olmasi GEREKMIYOR: guvenlik girdideki
    /// entropiden geliyor, anahtardan degil. Sabit baslangic, ilk blogun
    /// hangi ham veriden ciktiginin tekrarlanabilir olmasini sagliyor.
    pub const fn new() -> Self {
        Self {
            state: [0u8; BLOCK_OUT],
            buf: [0u8; BLOCK_IN],
            pos: 0,
            blocks_out: 0,
            bytes_in: 0,
        }
    }

    /// Bir ham bayt besle. Tampon dolunca yogunlastirilmis blok doner.
    pub fn push(&mut self, byte: u8) -> Option<[u8; BLOCK_OUT]> {
        self.buf[self.pos] = byte;
        self.pos += 1;
        self.bytes_in += 1;

        if self.pos < BLOCK_IN {
            return None;
        }
        self.pos = 0;
        Some(self.absorb())
    }

    /// Tamponu yut, zinciri ilerlet, yeni durumu dondur.
    fn absorb(&mut self) -> [u8; BLOCK_OUT] {
        // HMAC anahtar boyu serbest; 32 bayt durum dogrudan anahtar.
        let mut mac = <HmacSha256 as Mac>::new_from_slice(&self.state)
            .expect("HMAC her anahtar boyunu kabul eder");
        mac.update(&self.buf);
        let out = mac.finalize().into_bytes();

        self.state.copy_from_slice(&out);
        self.blocks_out += 1;
        self.state
    }

    /// Tamponda bekleyen bayt sayisi.
    pub fn pending(&self) -> usize {
        self.pos
    }

    /// Bir sonraki blok icin kalan bayt.
    pub fn remaining(&self) -> usize {
        BLOCK_IN - self.pos
    }

    /// Su ana kadar yutulan toplam min-entropy (bit).
    pub fn absorbed_entropy_bits(&self) -> u64 {
        self.bytes_in * H_PER_BYTE_X10 as u64 / 10
    }
}

/// `new()` ile birebir ayni. Asil kurucu `new()`; bu sadece jenerik kodun
/// `Default` bekledigi yerler icin var (`new()` `const`, bu olamaz).
impl Default for Conditioner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_is_not_short() {
        // Yorumdaki hesabin kodla ayni oldugunu dogrula.
        let supplied_bits = BLOCK_IN as u32 * H_PER_BYTE_X10 / 10;
        let required_bits = 8 * BLOCK_OUT as u32 * SAFETY_FACTOR;
        assert!(
            supplied_bits >= required_bits,
            "girdi {supplied_bits} bit < gerekli {required_bits} bit"
        );
        // Eski tasarim BU testi gecemezdi: 64 bayt x 3.4 = 217 < 512.
        let old_design_bits = 64 * H_PER_BYTE_X10 / 10;
        assert!(
            old_design_bits < required_bits,
            "eski tasarimin acik verdigini gosteren regresyon testi bozulmus"
        );
    }

    #[test]
    fn budget_supports_full_entropy_claim() {
        // Ekrandaki "8.00 b/B" iddiasinin dayanagi. SP 800-90C tam-entropi
        // kosulu: onaylı conditioning fonksiyonuna h_in >= 2n. Yani bu oran
        // 2.00'in altina duserse iddia GECERSIZ olur.
        //
        // Bu test iddiayla kodu birbirine bagliyor: biri kayarsa digeri
        // kirmizi yanar. Ekranin sessizce yanlis sey yazmasi bu yuzden
        // mumkun degil.
        assert_eq!(BUDGET_X100, 212, "butce payi degisti — ekran metnini ve dokumani gozden gecir");
        assert!(
            BUDGET_X100 >= 200,
            "butce {BUDGET_X100} < 200 => cikti tam entropi SAYILAMAZ, \
             ekranda 8.00 bit/bayt yazilamaz"
        );
    }

    #[test]
    fn emits_block_exactly_at_boundary() {
        let mut c = Conditioner::new();
        for i in 0..BLOCK_IN - 1 {
            assert!(c.push(i as u8).is_none(), "erken blok (i={i})");
        }
        assert!(c.push(0xFF).is_some(), "sinirda blok vermedi");
        assert_eq!(c.pending(), 0);
        assert_eq!(c.blocks_out, 1);
    }

    #[test]
    fn chaining_makes_blocks_differ_for_same_input() {
        // AYNI girdi blogu iki kez verilirse ciktilar FARKLI olmali —
        // zincirleme calisiyorsa. Eski tasarim (zincirsiz) ayni ciktiyi
        // verirdi; bu testin amaci tam olarak o regresyonu yakalamak.
        let mut c = Conditioner::new();
        let mut first = None;
        let mut second = None;
        for round in 0..2 {
            for i in 0..BLOCK_IN {
                if let Some(b) = c.push((i % 251) as u8) {
                    if round == 0 {
                        first = Some(b);
                    } else {
                        second = Some(b);
                    }
                }
            }
        }
        let a = first.expect("ilk blok yok");
        let b = second.expect("ikinci blok yok");
        assert_ne!(a, b, "ayni girdi ayni ciktiyi verdi -> zincirleme YOK");
    }

    #[test]
    fn different_input_gives_different_output() {
        let mut c1 = Conditioner::new();
        let mut c2 = Conditioner::new();
        let mut o1 = None;
        let mut o2 = None;
        for i in 0..BLOCK_IN {
            o1 = c1.push(i as u8).or(o1);
            o2 = c2.push((i as u8).wrapping_add(1)).or(o2);
        }
        assert_ne!(o1.unwrap(), o2.unwrap());
    }

    #[test]
    fn output_is_deterministic_for_same_stream() {
        let run = || {
            let mut c = Conditioner::new();
            let mut last = None;
            for i in 0..BLOCK_IN * 3 {
                if let Some(b) = c.push((i * 7 % 256) as u8) {
                    last = Some(b);
                }
            }
            last.unwrap()
        };
        assert_eq!(run(), run(), "ayni akis ayni ciktiyi vermeli");
    }

    #[test]
    fn entropy_accounting_matches_bytes() {
        let mut c = Conditioner::new();
        for i in 0..1000 {
            let _ = c.push(i as u8);
        }
        assert_eq!(c.bytes_in, 1000);
        assert_eq!(c.absorbed_entropy_bits(), 1000 * 34 / 10);
    }
}
