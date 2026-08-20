//! SSD1306 128x64 OLED —  
//!   - saglik testi durumu ve esiklere olan PAY (canli, her ornekte)
//!   - olculen min-entropy VARSAYIMI (nereden geldigi belgeli)
//!   - GERCEK olculmus cikti hizi
//!   - toplam uretim ve cikti modu
//! Tahmin edilen degil, o an olculen sayilar.

use core::fmt::Write;
use embedded_graphics::{
    mono_font::{MonoTextStyleBuilder, ascii::FONT_5X8, ascii::FONT_6X10},
    pixelcolor::BinaryColor,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use heapless::String;

/// Satir y konumlari (5x8 font + 1 px aralik).
const L0: i32 = 0; // baslik
const SEP1: i32 = 9;
const L1: i32 = 11; // Z1 saglik
const L2: i32 = 20; // Z2 saglik
const L3: i32 = 29; // min-entropy varsayimi
const L4: i32 = 38; // cikti hizi
const L5: i32 = 47; // toplam
const SEP2: i32 = 55;
const L6: i32 = 57; // mod / durum

/// Cihazin dis dunyaya ne verdigi.
#[derive(Copy, Clone, PartialEq, Eq)]
pub enum OutMode {
    /// Dogrudan conditioner cikisi — her bit olculmus fiziksel entropiye
    /// dayanir, yavas.
    Raw,
    /// HMAC_DRBG cikisi — hizli, kriptografik olarak saglam, ama
    /// deterministik genisletme.
    Drbg,
}

impl OutMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            OutMode::Raw => "RAW",
            OutMode::Drbg => "DRBG",
        }
    }
}

/// Bir kanalin ekrana yansiyan saglik ozeti.
pub struct ChannelView {
    /// 0 = acilis, 1 = ok, 2 = ariza
    pub state: u8,
    /// Acilista kalan ornek.
    pub startup_left: u32,
    /// En uzun ust uste tekrar / esik.
    pub rct_run: u32,
    pub rct_cutoff: u32,
    /// APT tepe / esik.
    pub apt_peak: u16,
    pub apt_cutoff: u16,
}

/// Conditioner ciktisinin TAM ENTROPI sayilabilmesi icin gereken en kucuk
/// butce payi, x100. SP 800-90C: onaylı conditioning fonksiyonunda
/// h_in >= 2n ise cikti tam entropidir.
pub const FULL_ENTROPY_BUDGET_X100: u16 = 200;

pub struct Snapshot {
    pub z1: ChannelView,
    pub z2: ChannelView,
    /// Conditioner GIRDISININ min-entropy'si, BAYT basina, x10
    /// (ornegin 34 = 3.4 bit/bayt).
    ///
    pub h_per_byte_x10: u16,
    /// Conditioner entropi butcesi payi x100 (ornegin 212 = 2.12x).
    pub budget_x100: u16,
    /// OLCULEN cikti hizi (bayt/saniye).
    pub out_bps: u32,
    /// Toplam uretilen bayt.
    pub total_bytes: u64,
    pub mode: OutMode,
    /// DRBG kac kez tohumlandi.
    pub seed_count: u64,
}

pub struct Ui;

impl Ui {
    pub fn new() -> Self {
        Self
    }

    pub fn boot<D>(&self, d: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        d.clear(BinaryColor::Off)?;
        let big = MonoTextStyleBuilder::new()
            .font(&FONT_6X10)
            .text_color(BinaryColor::On)
            .build();
        let small = MonoTextStyleBuilder::new()
            .font(&FONT_5X8)
            .text_color(BinaryColor::On)
            .build();

        Text::with_baseline("CTH 0.1", Point::new(34, 12), big, Baseline::Top).draw(d)?;
        Text::with_baseline("entropy source", Point::new(19, 30), small, Baseline::Top).draw(d)?;
        Text::with_baseline("self-test...", Point::new(28, 42), small, Baseline::Top).draw(d)?;
        Ok(())
    }

    /// BIR KEZ cizilir ve oncesinde ekran TAMAMEN temizlenmis olmali. Kendisi
    /// temizlik YAPMAZ; `draw()` de sadece kendi alanlarini (x >= 26 ve L6)
    /// siler. Yani `boot()` ekranindan kalan ve bu iki bolgeye dusmeyen
    /// pikseller — ornegin "entropy source" yazisinin x=19'dan baslayan ilk
    /// harfi — arada tam bir temizlik olmazsa ekranda kalici olarak kalir.
    pub fn frame<D>(&self, d: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        let t = MonoTextStyleBuilder::new()
            .font(&FONT_5X8)
            .text_color(BinaryColor::On)
            .build();
        let line = PrimitiveStyle::with_fill(BinaryColor::On);

        Text::with_baseline("CTH 0.1", Point::new(0, L0), t, Baseline::Top).draw(d)?;
        Rectangle::new(Point::new(0, SEP1), Size::new(128, 1))
            .into_styled(line)
            .draw(d)?;
        Rectangle::new(Point::new(0, SEP2), Size::new(128, 1))
            .into_styled(line)
            .draw(d)?;

        Text::with_baseline("Z1", Point::new(0, L1), t, Baseline::Top).draw(d)?;
        Text::with_baseline("Z2", Point::new(0, L2), t, Baseline::Top).draw(d)?;
        // "b/B" = bit/bayt. Satirin IKI ucu da bayt basina; ok ancak boyle
        // bir sey ifade ediyor. Kaynagin SP 800-90B birimi (bit/ornek)
        // ekranda degil, `s` durum raporunda ve docs/entropy.md'de.
        Text::with_baseline("ent", Point::new(0, L3), t, Baseline::Top).draw(d)?;
        Text::with_baseline("rate", Point::new(0, L4), t, Baseline::Top).draw(d)?;
        Text::with_baseline("out", Point::new(0, L5), t, Baseline::Top).draw(d)?;
        Ok(())
    }

    fn clear_fields<D>(&self, d: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        let c = PrimitiveStyle::with_fill(BinaryColor::Off);
        // baslik sagi
        Rectangle::new(Point::new(40, 0), Size::new(88, 8))
            .into_styled(c)
            .draw(d)?;
        for y in [L1, L2, L3, L4, L5] {
            Rectangle::new(Point::new(26, y), Size::new(102, 8))
                .into_styled(c)
                .draw(d)?;
        }
        Rectangle::new(Point::new(0, L6), Size::new(128, 8))
            .into_styled(c)
            .draw(d)?;
        Ok(())
    }

    /// Kanal satiri: durum + esige olan pay.
    ///
    /// "4/7" gibi bir sey yaziyoruz: soldaki OLCULEN en kotu deger, sagdaki
    /// ESIK. Kullanici bir bakista ne kadar paya sahip oldugunu goruyor.
    fn channel_line(buf: &mut String<32>, v: &ChannelView) {
        buf.clear();
        match v.state {
            0 => {
                let _ = write!(buf, "INIT {}", v.startup_left);
            }
            1 => {
                let _ = write!(
                    buf,
                    "OK  R{}/{} A{}/{}",
                    v.rct_run, v.rct_cutoff, v.apt_peak, v.apt_cutoff
                );
            }
            _ => {
                let _ = write!(buf, "** FAIL **");
            }
        }
    }

    pub fn draw<D>(&self, d: &mut D, s: &Snapshot) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = BinaryColor>,
    {
        self.clear_fields(d)?;

        let t = MonoTextStyleBuilder::new()
            .font(&FONT_5X8)
            .text_color(BinaryColor::On)
            .build();
        let mut buf: String<32> = String::new();

        // --- baslik: mod + genel durum ---
        let healthy = s.z1.state == 1 && s.z2.state == 1;
        buf.clear();
        let _ = write!(buf, "{} {}", s.mode.as_str(), if healthy { "LIVE" } else { "HOLD" });
        Text::with_baseline(&buf, Point::new(40, L0), t, Baseline::Top).draw(d)?;

        // --- kanallar ---
        Self::channel_line(&mut buf, &s.z1);
        Text::with_baseline(&buf, Point::new(26, L1), t, Baseline::Top).draw(d)?;
        Self::channel_line(&mut buf, &s.z2);
        Text::with_baseline(&buf, Point::new(26, L2), t, Baseline::Top).draw(d)?;

        // --- entropi satiri: KAYNAK -> CIKIS ---
        //
        // Soldaki sayi OLCULEN kaynak min-entropy'si, sagdaki CIKTININ
        // entropi yogunlugu. Sagdaki hicbir zaman olculmuyor — butceden
        // turetiliyor (bkz. modul basligi). Bu yuzden:
        //   - saglik testleri gecmiyorsa hicbir iddia YOK,
        //   - butce 2x'in altina duserse tam-entropi iddiasi YOK.
        // Yani ekran ancak arkasinda durabildigi seyi yaziyor.
        buf.clear();
        let h_int = s.h_per_byte_x10 / 10;
        let h_frac = s.h_per_byte_x10 % 10;
        if !healthy {
            let _ = write!(buf, "-- cikis yok --");
        } else {
            match s.mode {
                // RAW: dogrudan conditioner cikisi. Butce yeterliyse cikti
                // tam entropi, yani bayt basina 8.00 bit.
                OutMode::Raw if s.budget_x100 >= FULL_ENTROPY_BUDGET_X100 => {
                    let _ = write!(buf, "{h_int}.{h_frac} -> 8.00 b/B");
                    // 15 karakter. Alan 20 karakter (x=26..128, 5 px font).
                }
                // Butce yetmiyor: tam-entropi iddiasi dusuyor. Ne kadar
                // yogunlastigini yaziyoruz, "8.00" YAZMIYORUZ.
                OutMode::Raw => {
                    let _ = write!(buf, "{h_int}.{h_frac} -> kismi {}%", s.budget_x100 / 2);
                }
                // DRBG: deterministik genisletme. Cikti bilgi-kuramsal
                // anlamda tam entropi DEGIL; iddia kriptografik guc.
                OutMode::Drbg => {
                    let _ = write!(buf, "{h_int}.{h_frac} -> 256b CSPRNG");
                }
            }
        }
        Text::with_baseline(&buf, Point::new(26, L3), t, Baseline::Top).draw(d)?;

        // --- OLCULEN cikti hizi ---
        buf.clear();
        if s.out_bps >= 1000 {
            let _ = write!(buf, "{}.{} KB/s", s.out_bps / 1000, (s.out_bps % 1000) / 100);
        } else {
            let _ = write!(buf, "{} B/s", s.out_bps);
        }
        Text::with_baseline(&buf, Point::new(26, L4), t, Baseline::Top).draw(d)?;

        // --- toplam ---
        buf.clear();
        let kb = s.total_bytes / 1024;
        if kb >= 1024 {
            let _ = write!(buf, "{}.{} MB", kb / 1024, (kb % 1024) * 10 / 1024);
        } else {
            let _ = write!(buf, "{} KB", kb);
        }
        Text::with_baseline(&buf, Point::new(26, L5), t, Baseline::Top).draw(d)?;

        // --- alt satir: tohumlama / uyari ---
        buf.clear();
        if healthy {
            let _ = write!(buf, "seeds:{}", s.seed_count);
        } else {
            let _ = write!(buf, "OUTPUT SUPPRESSED");
        }
        Text::with_baseline(&buf, Point::new(0, L6), t, Baseline::Top).draw(d)?;

        Ok(())
    }
}
