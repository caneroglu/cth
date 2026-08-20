//! SP 800-90B surekli saglik testleri: RCT + APT + acilis testi.
//!
//! Bu testler HAM kaynak ciktisi uzerinde kosar — conditioner'dan ONCE.
//! Sebep basit: conditioner'dan sonra her sey rastgele gorunur, bozuk kaynak
//! dahil. Eski firmware'in "VN satiri 7.9 okuyor demek ki iyi" hatasi tam
//! buydu.
//!
//! ORNEK ALFABESI: 4 bit (nibble, 0..15).
//! Neden nibble: 12 bitin alt 4'unu aliyoruz (olculdu — ADC INL/DNL ve kazanc
//! kaymasi ust bitlerde birikir). Esikler de bu alfabeye gore hesaplandi.
//!
//! ESIKLER — uydurulmadi, hesaplandi (H = 3.4 bit/ornek, alpha = 2^-20):
//!
//!   H nereden geliyor: host analizinde AR(16) ile yapiyi cikardiktan sonra
//!   kalintinin alt-4-bit min-entropy'si 3.34-3.71 bit olctuk. Muhafazakar
//!   ucu (3.4) aliyoruz.
//!
//!   RCT:  C = 1 + ceil(-log2(alpha) / H) = 1 + ceil(20 / 3.4) = 7
//!         Yani ayni nibble 7 kez UST USTE gelirse kaynak arizali.
//!         Saglam kaynakta bunun olasiligi ~2^-20 (milyonda bir).
//!
//!   APT:  W = 512 pencere, p = 2^-H = 0.09473
//!         ortalama = 48.5 , std = 6.63
//!         Tam binom toplamiyla: P(X >= C) <= alpha veren en kucuk C = 84
//!         (= ortalama + 5.36 sigma)
//!
//!   Referans tablo (kaynak bozulup H dususe esikler DEGISIR — biz H=3.4
//!   varsayiyoruz ve sapmayi bu testler yakaliyor):
//!       H=2.0 -> RCT 11, APT 177
//!       H=2.5 -> RCT  9, APT 135
//!       H=3.0 -> RCT  8, APT 103
//!       H=3.4 -> RCT  7, APT  84   <- kullandigimiz
//!       H=3.7 -> RCT  7, APT  72
//!
//! ARIZA POLITIKASI: kilitlenir. Bir kez FAIL olunca `reset()` cagrilana
//! kadar FAIL kalir ve cikis kesilir. Bir RNG'de sessizce kotu veri vermek,
//! hic veri vermemekten cok daha kotudur — kullanici farkinda olmaz.

/// Ornek alfabesi genisligi (bit).
pub const SAMPLE_BITS: u8 = 4;
/// Alfabe boyu.
pub const ALPHABET: usize = 1 << SAMPLE_BITS;

/// Varsayilan min-entropy varsayimi (bit/ornek), x10 olarak.
/// Olculen aralik 3.34-3.71; muhafazakar uc.
pub const ASSUMED_H_X10: u16 = 34;

/// RCT kesme degeri: ayni deger kac kez ust uste gelirse ariza.
pub const RCT_CUTOFF: u32 = 7;

/// APT pencere boyu.
pub const APT_WINDOW: u16 = 512;
/// APT kesme degeri: pencerede referans deger kac kez gorulurse ariza.
pub const APT_CUTOFF: u16 = 84;

/// Acilis testi: cikis vermeye baslamadan once kac ornek temiz gecmeli.
/// SP 800-90B 1024 ornek istiyor.
pub const STARTUP_SAMPLES: u32 = 1024;

/// NOT: defmt::Format YOK, bilerek. Bu kutuphane host'ta `cargo test` ile
/// kosuyor ve orada defmt'in global_logger'i olmadigi icin link patlar.
/// Binary tarafi bu tipleri elle logluyor (alanlari okuyup info!/error! ile).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FailReason {
    /// Ayni deger RCT_CUTOFF kez ust uste geldi — kaynak takildi/oldu.
    Rct { value: u8, run: u32 },
    /// Pencerede bir deger APT_CUTOFF'tan fazla gorundu — dagilim bozuldu.
    Apt { value: u8, count: u16 },
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum HealthState {
    /// Acilis testi suruyor, cikis HENUZ verilmemeli.
    Startup { remaining: u32 },
    /// Testler geciyor, cikis verilebilir.
    Ok,
    /// Ariza kilitlendi, cikis KESILMELI.
    Failed(FailReason),
}

pub struct HealthTest {
    // --- RCT durumu ---
    rct_last: u8,
    rct_run: u32,
    rct_primed: bool,

    // --- APT durumu ---
    apt_ref: u8,
    apt_count: u16,
    apt_pos: u16,
    apt_primed: bool,

    // --- acilis ---
    startup_remaining: u32,

    // --- kilitli ariza ---
    failure: Option<FailReason>,

    // --- sayaclar (telemetri) ---
    pub total_samples: u64,
    pub rct_max_run: u32,
    pub apt_max_count: u16,
}

impl HealthTest {
    pub const fn new() -> Self {
        Self {
            rct_last: 0,
            rct_run: 0,
            rct_primed: false,
            apt_ref: 0,
            apt_count: 0,
            apt_pos: 0,
            apt_primed: false,
            startup_remaining: STARTUP_SAMPLES,
            failure: None,
            total_samples: 0,
            rct_max_run: 0,
            apt_max_count: 0,
        }
    }

    /// Bir ornek besle. Donen durum cikisin verilip verilemeyecegini soyler.
    ///
    /// `sample` 0..15 araliginda olmali (alt 4 bit). Ustu maskelenir.
    pub fn feed(&mut self, sample: u8) -> HealthState {
        // Kilitli ariza: hicbir sey degistirmeden FAIL don.
        if let Some(r) = self.failure {
            return HealthState::Failed(r);
        }

        let s = sample & (ALPHABET as u8 - 1);
        self.total_samples += 1;

        // ---- RCT: ust uste tekrar sayaci ----
        if !self.rct_primed {
            self.rct_last = s;
            self.rct_run = 1;
            self.rct_primed = true;
        } else if s == self.rct_last {
            self.rct_run += 1;
            if self.rct_run > self.rct_max_run {
                self.rct_max_run = self.rct_run;
            }
            if self.rct_run >= RCT_CUTOFF {
                let r = FailReason::Rct {
                    value: s,
                    run: self.rct_run,
                };
                self.failure = Some(r);
                return HealthState::Failed(r);
            }
        } else {
            self.rct_last = s;
            self.rct_run = 1;
        }

        // ---- APT: pencerede referans degerin sayimi ----
        // Pencerenin ILK ornegi referans olur, kalan W-1 ornekte kac kez
        // tekrar ettigi sayilir. Pencere dolunca yeni referansla bastan.
        if !self.apt_primed {
            self.apt_ref = s;
            self.apt_count = 1;
            self.apt_pos = 1;
            self.apt_primed = true;
        } else {
            if s == self.apt_ref {
                self.apt_count += 1;
                if self.apt_count > self.apt_max_count {
                    self.apt_max_count = self.apt_count;
                }
                if self.apt_count >= APT_CUTOFF {
                    let r = FailReason::Apt {
                        value: s,
                        count: self.apt_count,
                    };
                    self.failure = Some(r);
                    return HealthState::Failed(r);
                }
            }
            self.apt_pos += 1;
            if self.apt_pos >= APT_WINDOW {
                // yeni pencere: bir sonraki ornek referans olacak
                self.apt_primed = false;
            }
        }

        // ---- acilis testi ----
        if self.startup_remaining > 0 {
            self.startup_remaining -= 1;
            return HealthState::Startup {
                remaining: self.startup_remaining,
            };
        }

        HealthState::Ok
    }

    /// Su anki durum (ornek beslemeden).
    pub fn state(&self) -> HealthState {
        if let Some(r) = self.failure {
            HealthState::Failed(r)
        } else if self.startup_remaining > 0 {
            HealthState::Startup {
                remaining: self.startup_remaining,
            }
        } else {
            HealthState::Ok
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self.state(), HealthState::Ok)
    }

    /// Arizayi temizle ve acilis testini yeniden basla.
    ///
    /// BILINCLI olarak "otomatik toparlanma" YOK: ariza kendiliginden
    /// gecmez, birinin (host komutu veya reset) kararla temizlemesi gerekir.
    /// Aksi halde arizali bir kaynak arada bir test gecip veri sizdirabilir.
    pub fn reset(&mut self) {
        *self = Self::new();
    }
}

/// `new()` ile birebir ayni: acilis testi bekleyen, arizasiz bir test.
/// Asil kurucu `new()`; bu sadece jenerik kodun `Default` bekledigi yerler
/// icin var (`new()` `const`, bu olamaz).
impl Default for HealthTest {
    fn default() -> Self {
        Self::new()
    }
}

// ==========================================================================
//  TESTLER — donanim gerektirmez, `cargo test` host'ta kosar
// ==========================================================================
#[cfg(test)]
mod tests {
    use super::*;

    fn drain_startup(h: &mut HealthTest) {
        // Acilis testini deterministik ama tekrar etmeyen bir dizi ile gec.
        // 0,1,2,...,15,0,1,... : RCT tetiklemez (ust uste tekrar yok),
        // APT tetiklemez (her deger pencerede W/16 = 32 kez, esik 84).
        let mut i = 0u8;
        while !h.is_ok() {
            let s = h.feed(i % 16);
            i = i.wrapping_add(1);
            if let HealthState::Failed(_) = s {
                panic!("acilis sirasinda beklenmeyen ariza");
            }
        }
    }

    #[test]
    fn startup_blocks_output_then_passes() {
        let mut h = HealthTest::new();
        assert!(matches!(h.state(), HealthState::Startup { .. }));
        drain_startup(&mut h);
        assert!(h.is_ok());
        assert!(h.total_samples >= STARTUP_SAMPLES as u64);
    }

    #[test]
    fn rct_fires_at_cutoff() {
        let mut h = HealthTest::new();
        drain_startup(&mut h);
        // Ayni degeri ust uste bas. RCT_CUTOFF'a ulasinca FAIL beklenir.
        let mut fired_at = None;
        for k in 1..=RCT_CUTOFF + 2 {
            if let HealthState::Failed(FailReason::Rct { run, .. }) = h.feed(9) {
                fired_at = Some((k, run));
                break;
            }
        }
        let (k, run) = fired_at.expect("RCT tetiklenmedi");
        assert_eq!(run, RCT_CUTOFF, "kesme degerinde tetiklenmeli");
        assert!(k <= RCT_CUTOFF, "cok gec tetiklendi: {k}");
    }

    #[test]
    fn rct_does_not_fire_below_cutoff() {
        let mut h = HealthTest::new();
        drain_startup(&mut h);
        // CUTOFF-1 kez tekrar, sonra farkli deger -> sayac sifirlanmali
        for _ in 0..RCT_CUTOFF - 1 {
            assert!(!matches!(h.feed(3), HealthState::Failed(_)));
        }
        assert!(!matches!(h.feed(4), HealthState::Failed(_)));
        for _ in 0..RCT_CUTOFF - 1 {
            assert!(!matches!(h.feed(3), HealthState::Failed(_)));
        }
        assert!(h.is_ok(), "esigin altinda ariza vermemeli");
    }

    #[test]
    fn apt_fires_on_skewed_distribution() {
        let mut h = HealthTest::new();
        drain_startup(&mut h);
        // Referansi 5 yap, sonra 5'i sik ama ust uste OLMAYACAK sekilde bas
        // (yoksa RCT once tetiklenir ve APT'yi test etmemis olurduk).
        let mut failed = None;
        for i in 0..APT_WINDOW * 2 {
            let s = if i % 2 == 0 { 5 } else { (i % 16) as u8 | 1 };
            let s = if s == 5 && i % 2 == 1 { 7 } else { s };
            if let HealthState::Failed(r) = h.feed(s) {
                failed = Some(r);
                break;
            }
        }
        match failed {
            Some(FailReason::Apt { count, .. }) => {
                assert_eq!(count, APT_CUTOFF, "APT kesme degerinde tetiklenmeli");
            }
            Some(FailReason::Rct { run, .. }) => {
                panic!("APT beklenirken RCT tetiklendi (run={run}) — test kurgusu hatali");
            }
            None => panic!("APT tetiklenmedi"),
        }
    }

    #[test]
    fn uniform_stream_stays_healthy() {
        // Duz dagilimli, tekrarsiz akis uzun sure saglikli kalmali.
        let mut h = HealthTest::new();
        drain_startup(&mut h);
        // Basit LCG — deterministik ama dagilimi duz.
        let mut x: u32 = 0x1234_5678;
        for _ in 0..200_000 {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let s = ((x >> 16) & 0x0F) as u8;
            if matches!(h.feed(s), HealthState::Failed(_)) {
                panic!("saglikli akista yanlis alarm (ornek #{})", h.total_samples);
            }
        }
        assert!(h.is_ok());
    }

    #[test]
    fn failure_latches_until_reset() {
        let mut h = HealthTest::new();
        drain_startup(&mut h);
        for _ in 0..RCT_CUTOFF {
            let _ = h.feed(1);
        }
        assert!(matches!(h.state(), HealthState::Failed(_)));
        // Saglikli veri beslesek de FAIL kalmali
        for i in 0..1000u32 {
            assert!(matches!(h.feed((i % 16) as u8), HealthState::Failed(_)));
        }
        h.reset();
        assert!(matches!(h.state(), HealthState::Startup { .. }));
    }
}
