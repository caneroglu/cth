//! HMAC_DRBG (SP 800-90A, HMAC-SHA256) — hiz katmani.
//!
//! NE ISE YARIYOR: conditioner'in cikisi kaynagin hizina bagli (~13.6 KB/s
//! ham, 5:1 sikistirma sonrasi ~2.7 KB/s). DRBG bu tohumdan kriptografik
//! olarak guclu, cok daha hizli bir akis uretir.
//!
//! NE ISE YARAMIYOR: entropi URETMEZ. Cikisi DETERMINISTIK bir genislemedir.
//! Bu yuzden cihaz iki modu ayri sunuyor ve dokumantasyonda ayrimi net
//! yaziyoruz:
//!
//!   RAW  : dogrudan conditioner cikisi. Her bit olculmus fiziksel
//!          entropiye dayanir. Yavas.
//!   DRBG : bu modul. Hizli, kriptografik olarak saglam, ama bitler
//!          genisletilmis. /dev/urandom modeli.
//!
//! ALGORITMA (SP 800-90A 10.1.2):
//!
//!   update(data):
//!       K = HMAC(K, V || 0x00 || data)
//!       V = HMAC(K, V)
//!       if data != bos:
//!           K = HMAC(K, V || 0x01 || data)
//!           V = HMAC(K, V)
//!
//!   instantiate(seed):  K = 0x00*32 ; V = 0x01*32 ; update(seed)
//!   generate(out):      V = HMAC(K,V) tekrar tekrar -> cikti ; update(bos)
//!   reseed(seed):       update(seed) ; sayac = 1
//!
//! `generate` sonrasi `update(bos)` cagrisi ONEMLI: ileri gizlilik
//! (backtracking resistance) bundan geliyor. Cikti alindiktan sonra durum
//! degisiyor, yani ele gecen bir durumdan GECMIS cikti uretilemiyor.

use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

const SEEDLEN: usize = 32;

/// SP 800-90A'nin izin verdigi ust sinir 2^48; biz cok daha agresifiz cunku
/// kaynak surekli akiyor ve tohum bedava. Amac: DRBG ciktisinin fiziksel
/// entropiden uzun sure kopuk kalmamasi.
pub const RESEED_INTERVAL: u64 = 1024;

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum DrbgError {
    /// Tohumlanmadan uretim istendi.
    NotSeeded,
    /// Yeniden tohumlama araligi doldu, taze entropi bekleniyor.
    ReseedRequired,
}

pub struct HmacDrbg {
    k: [u8; SEEDLEN],
    v: [u8; SEEDLEN],
    seeded: bool,
    /// Son tohumlamadan beri yapilan uretim sayisi.
    pub reseed_counter: u64,
    /// Toplam uretilen bayt (telemetri).
    pub bytes_out: u64,
    /// Kac kez tohumlandi (telemetri).
    pub seed_count: u64,
}

impl HmacDrbg {
    pub const fn new() -> Self {
        Self {
            k: [0u8; SEEDLEN],
            v: [0u8; SEEDLEN],
            seeded: false,
            reseed_counter: 0,
            bytes_out: 0,
            seed_count: 0,
        }
    }

    fn hmac(key: &[u8; SEEDLEN], parts: &[&[u8]]) -> [u8; SEEDLEN] {
        let mut mac =
            <HmacSha256 as Mac>::new_from_slice(key).expect("HMAC her anahtar boyunu kabul eder");
        for p in parts {
            mac.update(p);
        }
        let out = mac.finalize().into_bytes();
        let mut r = [0u8; SEEDLEN];
        r.copy_from_slice(&out);
        r
    }

    /// SP 800-90A update fonksiyonu.
    fn update(&mut self, data: Option<&[u8]>) {
        let d = data.unwrap_or(&[]);

        self.k = Self::hmac(&self.k, &[&self.v, &[0x00], d]);
        self.v = Self::hmac(&self.k, &[&self.v]);

        if !d.is_empty() {
            self.k = Self::hmac(&self.k, &[&self.v, &[0x01], d]);
            self.v = Self::hmac(&self.k, &[&self.v]);
        }
    }

    /// Ilk tohumlama.
    pub fn instantiate(&mut self, seed: &[u8]) {
        self.k = [0x00u8; SEEDLEN];
        self.v = [0x01u8; SEEDLEN];
        self.update(Some(seed));
        self.seeded = true;
        self.reseed_counter = 1;
        self.seed_count += 1;
    }

    /// Taze entropiyle yeniden tohumla.
    pub fn reseed(&mut self, seed: &[u8]) {
        if !self.seeded {
            self.instantiate(seed);
            return;
        }
        self.update(Some(seed));
        self.reseed_counter = 1;
        self.seed_count += 1;
    }

    pub fn is_seeded(&self) -> bool {
        self.seeded
    }

    pub fn needs_reseed(&self) -> bool {
        !self.seeded || self.reseed_counter >= RESEED_INTERVAL
    }

    /// Cikti uret.
    ///
    /// Aralik dolduysa `ReseedRequired` doner ve HICBIR SEY uretmez —
    /// "biraz daha idare et" yok. Bir DRBG'nin taze entropiden kopmasina
    /// izin vermek, tam da kacinmak istedigimiz sey.
    pub fn generate(&mut self, out: &mut [u8]) -> Result<(), DrbgError> {
        if !self.seeded {
            return Err(DrbgError::NotSeeded);
        }
        if self.reseed_counter >= RESEED_INTERVAL {
            return Err(DrbgError::ReseedRequired);
        }

        let mut written = 0;
        while written < out.len() {
            self.v = Self::hmac(&self.k, &[&self.v]);
            let n = core::cmp::min(SEEDLEN, out.len() - written);
            out[written..written + n].copy_from_slice(&self.v[..n]);
            written += n;
        }

        // Ileri gizlilik: uretimden sonra durumu ilerlet.
        self.update(None);
        self.reseed_counter += 1;
        self.bytes_out += out.len() as u64;
        Ok(())
    }
}

/// `new()` ile birebir ayni: TOHUMSUZ bir DRBG. `generate()` bu haldeyken
/// `NotSeeded` doner, yani "varsayilan" bir DRBG kazara cikti veremez.
/// Asil kurucu `new()`; bu sadece jenerik kodun `Default` bekledigi yerler
/// icin var (`new()` `const`, bu olamaz).
impl Default for HmacDrbg {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> HmacDrbg {
        let mut d = HmacDrbg::new();
        d.instantiate(&[0x42u8; 32]);
        d
    }

    #[test]
    fn refuses_before_seeding() {
        let mut d = HmacDrbg::new();
        let mut buf = [0u8; 16];
        assert_eq!(d.generate(&mut buf), Err(DrbgError::NotSeeded));
        assert_eq!(buf, [0u8; 16], "tohumsuz uretimde tampon degismemeli");
    }

    #[test]
    fn same_seed_same_stream() {
        let mut a = seeded();
        let mut b = seeded();
        let (mut x, mut y) = ([0u8; 96], [0u8; 96]);
        a.generate(&mut x).unwrap();
        b.generate(&mut y).unwrap();
        assert_eq!(x, y, "DRBG deterministik olmali");
    }

    #[test]
    fn different_seed_different_stream() {
        let mut a = HmacDrbg::new();
        a.instantiate(&[1u8; 32]);
        let mut b = HmacDrbg::new();
        b.instantiate(&[2u8; 32]);
        let (mut x, mut y) = ([0u8; 64], [0u8; 64]);
        a.generate(&mut x).unwrap();
        b.generate(&mut y).unwrap();
        assert_ne!(x, y);
    }

    #[test]
    fn successive_calls_differ() {
        let mut d = seeded();
        let (mut x, mut y) = ([0u8; 64], [0u8; 64]);
        d.generate(&mut x).unwrap();
        d.generate(&mut y).unwrap();
        assert_ne!(x, y, "ardisik uretimler ayni cikti vermemeli");
    }

    #[test]
    fn reseed_changes_stream() {
        let mut a = seeded();
        let mut b = seeded();
        let mut junk = [0u8; 32];
        a.generate(&mut junk).unwrap();
        b.generate(&mut junk).unwrap();
        b.reseed(&[0x99u8; 32]);
        // Sayac tohumlamadan HEMEN SONRA 1 olmali; asagidaki generate onu
        // 2'ye cikaracak, o yuzden kontrol burada.
        assert_eq!(b.reseed_counter, 1, "reseed sayaci sifirlamali");
        assert_eq!(b.seed_count, 2);

        let (mut x, mut y) = ([0u8; 64], [0u8; 64]);
        a.generate(&mut x).unwrap();
        b.generate(&mut y).unwrap();
        assert_ne!(x, y, "yeniden tohumlama akisi degistirmeli");
        assert_eq!(b.reseed_counter, 2, "uretim sayaci ilerletmeli");
    }

    #[test]
    fn reseed_interval_is_enforced() {
        let mut d = seeded();
        let mut buf = [0u8; 8];
        // instantiate sayaci 1 yapti; INTERVAL'a kadar uretebilmeli.
        for i in 1..RESEED_INTERVAL {
            d.generate(&mut buf)
                .unwrap_or_else(|e| panic!("{i}. uretimde beklenmeyen hata: {e:?}"));
        }
        assert_eq!(
            d.generate(&mut buf),
            Err(DrbgError::ReseedRequired),
            "aralik dolunca uretim reddedilmeli"
        );
        d.reseed(&[7u8; 32]);
        assert!(
            d.generate(&mut buf).is_ok(),
            "tohumlamadan sonra devam etmeli"
        );
    }

    #[test]
    fn arbitrary_output_length_is_handled() {
        // 32'nin kati olmayan uzunluklar da dogru doldurulmali.
        let mut d = seeded();
        for len in [1usize, 7, 31, 32, 33, 100, 255] {
            let mut buf = [0u8; 255];
            d.generate(&mut buf[..len]).unwrap();
            assert!(
                buf[..len].iter().any(|&b| b != 0),
                "len={len} icin cikti tamamen sifir"
            );
            assert!(
                buf[len..].iter().all(|&b| b == 0),
                "len={len} icin tamponun disina yazildi"
            );
        }
    }

    #[test]
    fn output_is_statistically_flat() {
        // Kripto testi degil, kaba saglik kontrolu: 256 kovanin hicbiri
        // asiri sapmasin. 64 KB'da kova basina beklenen 256.
        let mut d = seeded();
        let mut hist = [0u32; 256];
        let mut buf = [0u8; 256];
        for _ in 0..256 {
            d.generate(&mut buf).unwrap();
            for &b in buf.iter() {
                hist[b as usize] += 1;
            }
        }
        let expected = 65536f64 / 256.0;
        let chi2: f64 = hist
            .iter()
            .map(|&o| {
                let d = o as f64 - expected;
                d * d / expected
            })
            .sum();
        // df=255, %99.9 kritik deger ~330. Bunun ustu ciddi bozukluk demek.
        assert!(chi2 < 330.0, "chi-kare cok yuksek: {chi2:.1}");
    }
}
