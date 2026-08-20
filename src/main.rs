#![no_std]
#![no_main]

//! CTH 0.1 — KATMAN 1 + HAM ORNEK DOKUMU
//! ---
//!  Claude ile *birlikte* yazıldı. Ön kontrolü yaptım - yeterli. Optimizasyon ve bir kaç bug fix, lazım.
//! Genel olarak çalışılabilir durumda. (Driver'lar ve debug mekanizması gereksiz kısımlar çok. Kaliteli kütüphaneler kullanılabilir veya ilerde kendim driver'lar yazıp kullanabilirim. Şu an için gereksiz - 2.sürümde.)
//! ---
//! Bu firmware iki is yapiyor:
//!
//!  1. ACILIS TESHISI (donanim yolu dogrulamasi)
//!     EEPROM kalibrasyonu -> mux kazanci -> bias taramasi -> ADC register
//!     kontrolu -> kisa gecikmeli otokorelasyon. Hepsi HAM OLCUM, yorum yok.
//!
//!  2. HAM ORNEK DOKUMU (RTT kanal 1)
//!     12-bit ADC orneklerini cerceveler halinde host'a basar. Butun ciddi
//!     analiz (FFT, aliasing kontrolu, drift ayirma, min-entropy) PC'de
//!     tools/analyze.py ile yapilir. (Claude yazdı, tam olarak KONTROL etmedim.)
//!
//! NEDEN ANALIZ CIHAZDA DEGIL:
//! Cihazda sabit-nokta ile yapi cozmeye calistim ve ust uste guvenilmez
//! sonuc verdi; FFT ve AR cozumu icin gereken dinamik araligi 20 KB RAM ve
//! FPU'suz Cortex-M3 ile tutturmak mumkun olmadi. Host'ta float ile yapiliyor.
//!
//! DAC BIAS: DOGRULANDI.
//! MCP4922 VOUTA/VOUTB ve R_sense fiziksel olarak kontrol edildi; bias
//! zinciri calisiyor, kod taramasi zenerlerin calisma noktasini kaydiriyor.
//! Gurultunun kaynagi da zener olarak yerlesik: besleme hatti scope ile
//! bakildi, ferrit disinda bilesen yok. Acik olan tek sey mekanizmanin SAF
//! tunelleme olmadigi (mikroplazma + tunelleme) — bkz. docs/entropy.md
//! bolum 8. `report_scaling` bunu ayrica akim bagimliligiyla destekliyor.

mod board;
mod drivers;

// Entropi mantigi kutuphanede (src/lib.rs): host'ta `cargo test` ile
// dogrulanabilmesi icin donanimdan ayri tutuluyor.
#[cfg(not(feature = "dump"))]
use cth_0_1::entropy::{
    conditioner::{self, Conditioner},
    drbg::{self, HmacDrbg},
    health::{self, HealthState, HealthTest},
};

// Aslında "dump" pek gerekli değil. Fakat yinede koyalım.

#[cfg(not(feature = "dump"))]
mod ui;
#[cfg(not(feature = "dump"))]
use ssd1306::{I2CDisplayInterface, Ssd1306, prelude::*};
#[cfg(not(feature = "dump"))]
use ui::{ChannelView, OutMode, Snapshot, Ui};

use defmt::{error, info, warn};
use embassy_executor::Spawner;
use embassy_stm32::adc::SampleTime;
use embassy_time::{Instant, Timer};
use micromath::statistics::{Mean, Variance};
use rtt_target::ChannelMode::NoBlockSkip;
use rtt_target::{DownChannel, UpChannel, rtt_init, set_defmt_channel};

use panic_probe as _;

use board::Board;
use drivers::at24c256::{self, CalibV1};
use drivers::hc4067;
use drivers::mcp4922::Zener;

//  RTT KANALLARI — Embed.toml ile birebir ayni olmali
//  RAM maliyeti: 1024 + 2048 + 64 = 3136 bayt.
//  Kanal 1'i 2048 tuttum cunku bir dokum cercevesi 1040 bayt; iki cerceve
//  sigmasin ki host okumadiginda cerceve ATILSIN, yarim yazilmasin.
const LOG_BUF: usize = 1024;
const BYTES_BUF: usize = 2048;
const CMD_BUF: usize = 64;

// ==========================================================================
//  TESHIS PARAMETRELERI
// ==========================================================================
const PROBE_N: u32 = 4000;
const SWEEP_N: u32 = 2000;
const SWEEP_SETTLE_MS: u64 = 400;
const SWEEP_CODES: [u16; 8] = [0, 250, 500, 1000, 1638, 2004, 2850, 4095];

/// Kisa gecikmeli otokorelasyon penceresi (ham olcum, yorum yok).
const CORR_N: usize = 512;
const MAX_LAG: usize = 8; // 8 ms

/// Varsayilan ornekleme suresi (teshis icin). Eski firmware'in secimi;
/// kiyas tabanini korumak icin ayni.
const DEFAULT_SAMPLE_TIME: SampleTime = SampleTime::CYCLES239_5;

/// EEPROM okunamazsa. UYARI: bu degerler kalibre EDILMIS degil.
const FALLBACK_DAC_Z1: u16 = 2850;
const FALLBACK_DAC_Z2: u16 = 2892;
const FALLBACK_MUX: u8 = 0;

#[derive(Copy, Clone, PartialEq, Eq, defmt::Format)]
enum CalibSource {
    Eeprom,
    Fallback,
}

#[cfg(feature = "dump")]
const DUMP_SAMPLES: usize = 512;
#[cfg(feature = "dump")]
const FRAME_HEADER: usize = 16;
#[cfg(feature = "dump")]
const FRAME_LEN: usize = FRAME_HEADER + DUMP_SAMPLES * 2;

/// Ornekleme suresi tablosu: (register kodu, enum).
/// Aliasing sorusunu cevaplamak icin dokum bunlarin hepsini geziyor:
/// ornekleme periyodu degisince gorunen frekans
///   - Hz olarak sabit kalirsa   -> gercek sinyal
///   - ornek sayisi olarak sabit -> orneklemeye kilitli artefakt
///   - aliasing formuluyle kayarsa -> daha yuksek frekansin katlanmasi
#[cfg(feature = "dump")]
const SAMPLE_TIMES: [(u8, SampleTime); 8] = [
    (0, SampleTime::CYCLES1_5),
    (1, SampleTime::CYCLES7_5),
    (2, SampleTime::CYCLES13_5),
    (3, SampleTime::CYCLES28_5),
    (4, SampleTime::CYCLES41_5),
    (5, SampleTime::CYCLES55_5),
    (6, SampleTime::CYCLES71_5),
    (7, SampleTime::CYCLES239_5),
];

/// Dokum sirasinda gezilecek DAC kodlari.
/// 0 dahil: DAC kopuksa iki kod arasinda hicbir fark gorulmeyecek
#[cfg(feature = "dump")]
const DUMP_DAC_CODES: [u16; 2] = [0, 2004];

// Varyans, aslında kütüphane kullanabilirdim. TODO.
/// Var(x) = E[x^2] - E[x]^2, tamsayi. sum_sq u64 sart: 4095^2 * 4000 ~ 6.7e10.
///
/// NEDEN BURADA micromath YOK: micromath'in `Variance`/`StdDev` trait'leri
/// `&[T]` istiyor, yani tum orneklerin bellekte olmasini. `probe_channel`
/// 4000 ornegi AKARKEN topluyor (8 KB tampon gerekirdi, toplam RAM 20 KB).
/// Bu yuzden akan toplam. Karekok yine de elle degil: `u64::isqrt` core'da.
fn std_dev(sum: u64, sum_sq: u64, n: u64) -> u64 {
    if n == 0 {
        return 0;
    }
    let mean = sum / n;
    (sum_sq / n).saturating_sub(mean * mean).isqrt()
}

struct ChannelStats {
    min: u16,
    max: u16,
    mean: u16,
    std: u64,
    /// Alt 4 bit histogrami — kararlastirdigimiz bit genisligi.
    nib: [u32; 16],
    elapsed_us: u64,
}

impl ChannelStats {
    fn span(&self) -> u16 {
        self.max.saturating_sub(self.min)
    }
}

/// Tek kanaldan n ornek alip ozetler.
async fn probe_channel<T: embassy_stm32::adc::Instance>(
    adc: &mut embassy_stm32::adc::Adc<'static, T>,
    pin: &mut impl embassy_stm32::adc::AdcChannel<T>,
    n: u32,
    smp: SampleTime,
) -> ChannelStats {
    let mut min = u16::MAX;
    let mut max = 0u16;
    let mut sum = 0u64;
    let mut sum_sq = 0u64;
    let mut nib = [0u32; 16];

    let t0 = Instant::now();
    for _ in 0..n {
        let r = adc.read(pin, smp).await;
        min = min.min(r);
        max = max.max(r);
        sum += r as u64;
        sum_sq += (r as u64) * (r as u64);
        nib[(r & 0x0F) as usize] += 1;
    }

    let nn = n.max(1) as u64;
    ChannelStats {
        min,
        max,
        mean: (sum / nn) as u16,
        std: std_dev(sum, sum_sq, nn),
        nib,
        elapsed_us: t0.elapsed().as_micros(),
    }
}

/// Bias taramasinin sonucu: std akimla olcekleniyor mu?
/// Zener cig/shot gurultusu ~sqrt(I) buyur; yukseltec/besleme/ADC tabani
/// akimdan bagimsizdir.
fn report_scaling(name: &str, stds: &[u64]) {
    let base = stds[0];
    let top = *stds.last().unwrap_or(&0);

    if base == 0 {
        info!(
            "atif {=str}: kod=0'da std=0, kod=4095'te std={=u64}",
            name, top
        );
        return;
    }

    let ratio_x100 = (top * 100) / base;
    info!(
        "atif {=str}: std kod=0 -> {=u64} ; kod=4095 -> {=u64} ; oran={=u64}/100",
        name, base, top, ratio_x100
    );

    if ratio_x100 < 200 {
        warn!(
            "  {=str}: gurultu genligi bias akimiyla olceklenMIYOR. Kopuk DAC \
             cikisi, iletmeyen zener veya akimdan bagimsiz bir kaynak — ucu de \
             bu veriyle uyumlu. Ayirt etmek icin VOUTA/VOUTB ve R_sense'e \
             fiziksel prob gerekiyor.",
            name
        );
    }
}

/// Kisa gecikmeli otokorelasyon — HAM OLCUM, yorum yok.
fn report_acf(name: &str, buf: &[u16]) {
    let n = buf.len();
    if n < MAX_LAG + 2 {
        return;
    }

    // Ortalama/varyans micromath'tan; `buf` zaten tamponda oldugu icin
    // slice API'si dogrudan kullanilabiliyor (probe_channel'in aksine).
    //
    // PAYDA NOTU: micromath ORNEK varyansi veriyor (n-1), asagidaki kovaryans
    // ise n-lag ile bolunuyor. rho bu yuzden (n-1)/n kadar, yani n=512'de
    // %0.2 olceklenmis cikiyor. Ham teshis ciktisi icin onemsiz; buradan
    // karar uretilmiyor (yorum yok, sadece olcum).
    let mean = buf.iter().map(|&v| f32::from(v)).mean();
    let var = buf.variance();
    if var <= 0.0 {
        info!("  {=str} ACF: varyans yok", name);
        return;
    }

    let mut rho = [0i32; MAX_LAG + 1];
    for lag in 1..=MAX_LAG {
        let cov = (lag..n)
            .map(|i| (f32::from(buf[i]) - mean) * (f32::from(buf[i - lag]) - mean))
            .sum::<f32>()
            / (n - lag) as f32;
        rho[lag] = (cov / var * 1000.0) as i32;
    }

    // Ortalamayi kac kez kesiyor: ardisik ikililerde tarafin degismesi.
    let crossings = buf
        .windows(2)
        .filter(|w| (f32::from(w[0]) >= mean) != (f32::from(w[1]) >= mean))
        .count() as u32;

    info!(
        "  {=str} ACF x1000: k1={=i32} k2={=i32} k3={=i32} k4={=i32} k5={=i32} k6={=i32} k7={=i32} k8={=i32}",
        name, rho[1], rho[2], rho[3], rho[4], rho[5], rho[6], rho[7], rho[8]
    );
    info!(
        "  {=str} sifir gecisi={=u32}/{=usize} (beyaz gurultude ~{=usize})",
        name,
        crossings,
        n,
        n / 2
    );
}

/// ADC register'larini DOGRUDAN okur (unstable-pac).
/// Bir kosuda ADC2 35 us yerine 9 us okumustu; register'a bakmak
/// spekulasyonu bitiriyor.
fn dump_adc_regs() {
    use embassy_stm32::pac::{ADC1, ADC2};

    let s1 = ADC1.smpr2().read();
    let s2 = ADC2.smpr2().read();
    let c1 = ADC1.cr2().read();
    let c2 = ADC2.cr2().read();

    info!("--- ADC register kontrolu ---");
    info!(
        "  ADC1 SMPR2.smp[ch0]={=u8} adon={} cont={} swstart={}",
        s1.smp(0) as u8,
        c1.adon(),
        c1.cont(),
        c1.swstart()
    );
    info!(
        "  ADC2 SMPR2.smp[ch1]={=u8} adon={} cont={} swstart={}",
        s2.smp(1) as u8,
        c2.adon(),
        c2.cont(),
        c2.swstart()
    );
    info!("  (0=1.5c 1=7.5c 2=13.5c 3=28.5c 4=41.5c 5=55.5c 6=71.5c 7=239.5c)");
}

/// Cerceve basligini yazar, ornek alanini dondurur.
#[cfg(feature = "dump")]
fn write_header(
    frame: &mut [u8; FRAME_LEN],
    seq: u16,
    channel: u8,
    smp_code: u8,
    dac: u16,
    count: u16,
    elapsed_us: u32,
) {
    frame[0..4].copy_from_slice(b"CTH1");
    frame[4..6].copy_from_slice(&seq.to_le_bytes());
    frame[6] = channel;
    frame[7] = smp_code;
    frame[8..10].copy_from_slice(&dac.to_le_bytes());
    frame[10..12].copy_from_slice(&count.to_le_bytes());
    frame[12..16].copy_from_slice(&elapsed_us.to_le_bytes());
}

//  MAIN
#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    // RTT her seyden once: bundan onceki hicbir defmt cagrisi bir yere gitmez.
    let channels = rtt_init! {
        up: {
            // Ad "defmt" OLMAK ZORUNDA (yukaridaki LOG_NAME aciklamasi).
            // Makro burada literal istiyor, const kabul etmiyor.
            0: { size: LOG_BUF,   mode: NoBlockSkip, name: "defmt" }
            1: { size: BYTES_BUF, mode: NoBlockSkip, name: "bytes" }
        }
        down: {
            0: { size: CMD_BUF, name: "cmd" }
        }
    };
    set_defmt_channel(channels.up.0);

    let mut bytes: UpChannel = channels.up.1;
    // Host -> cihaz komut kanali (r/d/s/h). Dokum modunda kullanilmiyor
    #[cfg(not(feature = "dump"))]
    let mut cmd: DownChannel = channels.down.0;
    #[cfg(feature = "dump")]
    let _cmd: DownChannel = channels.down.0;

    info!("=== CTH 0.1 / teshis + ham dokum ===");
    info!("firmware : {=str}", env!("GIT_DESCRIBE"));
    if env!("GIT_DESCRIBE").ends_with('+') {
        warn!("firmware KIRLI agactan derlendi");
    }

    let mut b = Board::init();
    info!("saat 72 MHz, JTAG kapali, DAC=0, mux kapali");

    // --- EEPROM kalibrasyonu ---
    let (calib, source) = match at24c256::read_calib_v1(&mut b.i2c) {
        Ok(c) => {
            info!(
                "EEPROM: surum=0x{=u16:04x} zaman={=u64} rsense={=u16}",
                c.version, c.timestamp_utc, c.rsense_ohms
            );
            info!(
                "  Z1 dac={=u16} ({=u32} uA) mux=CH{=u8} shannon={=u32} mb",
                c.dac_z1,
                drivers::mcp4922::code_to_ua(c.dac_z1),
                c.mux_z1,
                c.shannon_z1_mb
            );
            info!(
                "  Z2 dac={=u16} ({=u32} uA) mux=CH{=u8} shannon={=u32} mb",
                c.dac_z2,
                drivers::mcp4922::code_to_ua(c.dac_z2),
                c.mux_z2,
                c.shannon_z2_mb
            );
            warn!("v1 kaydinda SAGLAMA YOK (v2'de crc32 gelecek)");
            (c, CalibSource::Eeprom)
        }
        Err(e) => {
            error!("EEPROM okunamadi: {}", e);
            warn!("FALLBACK — bu cihaz KALIBRE SAYILMAZ");
            (
                CalibV1 {
                    version: 0,
                    timestamp_utc: 0,
                    temp_c_x10: 0,
                    rsense_ohms: 1000,
                    dac_z1: FALLBACK_DAC_Z1,
                    dac_z2: FALLBACK_DAC_Z2,
                    mux_z1: FALLBACK_MUX,
                    mux_z2: FALLBACK_MUX,
                    shannon_z1_mb: 0,
                    shannon_z2_mb: 0,
                },
                CalibSource::Fallback,
            )
        }
    };
    info!("kalibrasyon kaynagi: {}", source);

    // --- Kazanc (taramadan ONCE; kazanc degisirse genlik de degisir) ---
    match b.mux_z1.select(calib.mux_z1) {
        Ok(()) => info!(
            "MUX Z1=CH{=u8} R_f={=u16} kazanc={=u32}/100",
            calib.mux_z1,
            hc4067::rf_total(calib.mux_z1).unwrap_or(0),
            hc4067::total_gain_x100(calib.mux_z1).unwrap_or(0)
        ),
        Err(ch) => error!("MUX Z1: CH{=u8} bos — kazanc bilinmiyor!", ch),
    }
    match b.mux_z2.select(calib.mux_z2) {
        Ok(()) => info!(
            "MUX Z2=CH{=u8} R_f={=u16} kazanc={=u32}/100",
            calib.mux_z2,
            hc4067::rf_total(calib.mux_z2).unwrap_or(0),
            hc4067::total_gain_x100(calib.mux_z2).unwrap_or(0)
        ),
        Err(ch) => error!("MUX Z2: CH{=u8} bos — kazanc bilinmiyor!", ch),
    }

    // --- Bias taramasi ---
    info!("--- bias taramasi ---");
    let mut std_z1 = [0u64; SWEEP_CODES.len()];
    let mut std_z2 = [0u64; SWEEP_CODES.len()];

    for (i, &code) in SWEEP_CODES.iter().enumerate() {
        b.dac.set(Zener::Z1, code);
        b.dac.set(Zener::Z2, code);
        Timer::after_millis(SWEEP_SETTLE_MS).await;

        let s1 = probe_channel(&mut b.adc1, &mut b.pin_z1, SWEEP_N, DEFAULT_SAMPLE_TIME).await;
        let s2 = probe_channel(&mut b.adc2, &mut b.pin_z2, SWEEP_N, DEFAULT_SAMPLE_TIME).await;
        std_z1[i] = s1.std;
        std_z2[i] = s2.std;

        info!(
            "  kod={=u16} akim={=u32}uA | Z1 std={=u64} ort={=u16} | Z2 std={=u64} ort={=u16}",
            code,
            drivers::mcp4922::code_to_ua(code),
            s1.std,
            s1.mean,
            s2.std,
            s2.mean
        );
    }
    report_scaling("Z1", &std_z1);
    report_scaling("Z2", &std_z2);

    dump_adc_regs();

    // --- Kalibrasyon degerleriyle olcum + kisa ACF ---
    b.dac.set(Zener::Z1, calib.dac_z1);
    b.dac.set(Zener::Z2, calib.dac_z2);
    Timer::after_millis(SWEEP_SETTLE_MS).await;

    let live1 = probe_channel(&mut b.adc1, &mut b.pin_z1, PROBE_N, DEFAULT_SAMPLE_TIME).await;
    let live2 = probe_channel(&mut b.adc2, &mut b.pin_z2, PROBE_N, DEFAULT_SAMPLE_TIME).await;
    info!("--- kalibre degerlerle ---");
    info!(
        "  olculen hiz: {=u64} us/ornek ({=u64} ornek/s)",
        live1.elapsed_us / PROBE_N as u64,
        (PROBE_N as u64 * 1_000_000) / live1.elapsed_us.max(1)
    );
    info!(
        "  Z1 min={=u16} max={=u16} bant={=u16} ort={=u16} std={=u64}",
        live1.min,
        live1.max,
        live1.span(),
        live1.mean,
        live1.std
    );
    info!(
        "  Z2 min={=u16} max={=u16} bant={=u16} ort={=u16} std={=u64}",
        live2.min,
        live2.max,
        live2.span(),
        live2.mean,
        live2.std
    );
    info!("  Z1 alt-4-bit: {:?}", live1.nib);
    info!("  Z2 alt-4-bit: {:?}", live2.nib);

    if live1.span() == 0 {
        error!("Z1 tamamen sabit — analog yol olu");
    }
    if live2.span() == 0 {
        error!("Z2 tamamen sabit — analog yol olu");
    }

    {
        let mut cbuf = [0u16; CORR_N];
        for v in cbuf.iter_mut() {
            *v = b.adc1.read(&mut b.pin_z1, DEFAULT_SAMPLE_TIME).await;
        }
        report_acf("Z1", &cbuf);
        for v in cbuf.iter_mut() {
            *v = b.adc2.read(&mut b.pin_z2, DEFAULT_SAMPLE_TIME).await;
        }
        report_acf("Z2", &cbuf);
    }

    #[cfg(feature = "dump")]
    {
        info!(
            "--- HAM DOKUM modu (feature=dump): cerceve {=usize} bayt ---",
            FRAME_LEN
        );
        info!(
            "  host: python3 tools/analyze.py (once), sonra cargo embed --release --features dump"
        );

        let mut frame = [0u8; FRAME_LEN];
        let mut seq: u16 = 0;
        let mut sent: u32 = 0;
        let mut skipped: u32 = 0;

        loop {
            for &dac_code in DUMP_DAC_CODES.iter() {
                b.dac.set(Zener::Z1, dac_code);
                b.dac.set(Zener::Z2, dac_code);
                Timer::after_millis(SWEEP_SETTLE_MS).await;

                for &(smp_code, smp) in SAMPLE_TIMES.iter() {
                    for ch in 1u8..=2u8 {
                        let t0 = Instant::now();
                        for i in 0..DUMP_SAMPLES {
                            let r = if ch == 1 {
                                b.adc1.read(&mut b.pin_z1, smp).await
                            } else {
                                b.adc2.read(&mut b.pin_z2, smp).await
                            };
                            let off = FRAME_HEADER + i * 2;
                            frame[off..off + 2].copy_from_slice(&r.to_le_bytes());
                        }
                        let elapsed = t0.elapsed().as_micros() as u32;
                        write_header(
                            &mut frame,
                            seq,
                            ch,
                            smp_code,
                            dac_code,
                            DUMP_SAMPLES as u16,
                            elapsed,
                        );

                        // TEK write: NoBlockSkip sayesinde ya tamami gider ya
                        // hicbiri — yarim cerceve olusmaz.
                        if bytes.write(&frame) == FRAME_LEN {
                            sent += 1;
                            b.led_data.toggle();
                        } else {
                            skipped += 1;
                        }
                        seq = seq.wrapping_add(1);
                    }
                }
            }
            b.led_status.toggle();
            info!(
                "dokum: gonderilen={=u32} atilan={=u32} (atilan = host yetismedi)",
                sent, skipped
            );
        }
    }

    // ======================================================================
    //  NORMAL CALISMA: hasat -> saglik -> conditioner -> (RAW | DRBG) -> RTT
    // ======================================================================
    #[cfg(not(feature = "dump"))]
    {
        // I2C'yi ekrana devret. Mutex'e gerek yok: EEPROM sadece acilista
        // okundu, bundan sonra bus'i tek kullanan ekran.
        let iface = I2CDisplayInterface::new(b.i2c);
        let mut disp = Ssd1306::new(iface, DisplaySize128x64, DisplayRotation::Rotate0)
            .into_buffered_graphics_mode();
        let ui = Ui::new();
        let oled_ok = disp.init().is_ok();
        if !oled_ok {
            warn!("OLED init basarisiz — ekransiz devam ediliyor");
        } else {
            let _ = ui.boot(&mut disp);
            let _ = disp.flush();
        }

        info!("--- entropi hatti ---");
        info!(
            "  saglik : RCT C={=u32}  APT C={=u16}/W={=u16}  acilis={=u32}",
            health::RCT_CUTOFF,
            health::APT_CUTOFF,
            health::APT_WINDOW,
            health::STARTUP_SAMPLES
        );
        info!(
            "  cond   : {=usize} bayt -> {=usize} bayt (butce {=u32} bit >= {=u32} bit)",
            conditioner::BLOCK_IN,
            conditioner::BLOCK_OUT,
            conditioner::BLOCK_IN as u32 * conditioner::H_PER_BYTE_X10 / 10,
            8 * conditioner::BLOCK_OUT as u32 * conditioner::SAFETY_FACTOR
        );
        info!(
            "  cikti  : pay {=u32}/100 >= 200 => TAM ENTROPI (8.00 bit/bayt, SP 800-90C)",
            conditioner::BUDGET_X100
        );
        info!("           bu bir OLCUM DEGIL, butce sonucu; gecerliligi saglik testlerine bagli");
        info!(
            "  drbg   : HMAC_DRBG, HER blokta yeniden tohumlama (zorunlu tavan: {=u64} uretim)",
            drbg::RESEED_INTERVAL
        );
        info!("  komut  : r=RAW  d=DRBG  s=durum  h=saglik sifirla");

        let mut hz1 = HealthTest::new();
        let mut hz2 = HealthTest::new();
        let mut cond = Conditioner::new();
        let mut rng = HmacDrbg::new();

        let mut mode = OutMode::Raw;
        let mut cmd_buf = [0u8; CMD_BUF];
        let mut drbg_out = [0u8; 256];

        let mut total_out: u64 = 0;
        let mut dropped: u64 = 0;
        let mut suppressed: u64 = 0;

        let mut win_start = Instant::now();
        let mut win_bytes: u32 = 0;
        let mut out_bps: u32 = 0;
        let mut last_ui = Instant::now();
        // Sabit cerceve (etiketler + ayrac cizgiler) cizildi mi.
        // Ilk UI karesine kadar acilis ekrani duruyor.
        let mut chrome_drawn = false;

        loop {
            // ---- ornek ----
            let r1 = b.adc1.read(&mut b.pin_z1, DEFAULT_SAMPLE_TIME).await;
            let r2 = b.adc2.read(&mut b.pin_z2, DEFAULT_SAMPLE_TIME).await;
            let n1 = (r1 & 0x0F) as u8;
            let n2 = (r2 & 0x0F) as u8;

            // ---- saglik: KANAL BASINA, ham nibble uzerinde ----
            let s1 = hz1.feed(n1);
            let s2 = hz2.feed(n2);
            let healthy = matches!(s1, HealthState::Ok) && matches!(s2, HealthState::Ok);

            if healthy {
                // Iki bagimsiz kanalin nibble'i tek bayta.
                if let Some(block) = cond.push((n1 << 4) | n2) {
                    match mode {
                        OutMode::Raw => {
                            // Her bit olculmus fiziksel entropiye dayanir.
                            if bytes.write(&block) == block.len() {
                                total_out += block.len() as u64;
                                win_bytes += block.len() as u32;
                                b.led_data.toggle();
                            } else {
                                dropped += block.len() as u64;
                            }
                        }
                        OutMode::Drbg => {
                            // Taze blok HER ZAMAN tohum olarak kullanilir;
                            // DRBG'nin fiziksel entropiden kopmasina izin yok.
                            rng.reseed(&block);
                            if rng.generate(&mut drbg_out).is_ok() {
                                if bytes.write(&drbg_out) == drbg_out.len() {
                                    total_out += drbg_out.len() as u64;
                                    win_bytes += drbg_out.len() as u32;
                                    b.led_data.toggle();
                                } else {
                                    dropped += drbg_out.len() as u64;
                                }
                            }
                        }
                    }
                }
            } else {
                suppressed += 1;
            }

            // ---- host komutu ----
            let n = cmd.read(&mut cmd_buf);
            for &c in cmd_buf[..n].iter() {
                match c {
                    b'r' => {
                        mode = OutMode::Raw;
                        info!("mod -> RAW (dogrudan conditioner cikisi)");
                    }
                    b'd' => {
                        mode = OutMode::Drbg;
                        info!("mod -> DRBG (hizli, deterministik genisletme)");
                    }
                    b'h' => {
                        hz1.reset();
                        hz2.reset();
                        warn!("saglik testleri SIFIRLANDI, acilis testi yeniden kosuyor");
                    }
                    b's' => {
                        report_health("Z1", &hz1);
                        report_health("Z2", &hz2);
                        info!(
                            "  mod={=str} toplam={=u64}B hiz={=u32}B/s atilan={=u64}B bastirilan={=u64}",
                            mode.as_str(),
                            total_out,
                            out_bps,
                            dropped,
                            suppressed
                        );
                        info!(
                            "  cond: {=u64} bayt / {=u64} blok | drbg: {=u64} tohum / {=u64} bayt",
                            cond.bytes_in, cond.blocks_out, rng.seed_count, rng.bytes_out
                        );
                        // Entropi birimleri acikca, paydalariyla. Ekranda yer
                        // yok; asil degerlendirme birimi (SP 800-90B: bit/
                        // ornek) burada duruyor ki rapor okuyan karistirmasin.
                        info!(
                            "  entropi: kaynak {=u16}/10 bit/ORNEK (SP 800-90B) | \
                             cond girdi {=u32}/10 bit/BAYT | cikti 8.0 bit/BAYT (turetildi)",
                            health::ASSUMED_H_X10,
                            conditioner::H_PER_BYTE_X10
                        );
                    }
                    _ => {}
                }
            }

            // ---- OLCULEN hiz (1 sn pencere; tahmin degil) ----
            if win_start.elapsed().as_millis() >= 1000 {
                out_bps = win_bytes;
                win_bytes = 0;
                win_start = Instant::now();
            }

            // !embedded-graphics yerine başka framework kullan.
            // ---- ekran ----
            if last_ui.elapsed().as_millis() >= 1000 {
                last_ui = Instant::now();
                b.led_status.toggle();

                if oled_ok {
                    // Sabit cerceve BIR KEZ, oncesinde TAM temizlik.
                    //
                    // Tam temizlik sart: `Ui::draw` sadece kendi alanlarini
                    // siliyor (x >= 26, artik satir L6). Acilis ekranindaki
                    // "entropy source" yazisi x=19'dan basliyor, yani ilk
                    // harfi hicbir alan temizligine girmiyor ve silinmezse
                    // ekranda KALICI olarak kaliyor. Olculdu: x=19..24,
                    // y=30..37 araligindaki 'e' glifi.
                    if !chrome_drawn {
                        disp.clear_buffer();
                        let _ = ui.frame(&mut disp);
                        chrome_drawn = true;
                    }

                    let snap = Snapshot {
                        z1: view_of(&hz1),
                        z2: view_of(&hz2),
                        // BAYT basina olan sabit; ekrandaki ok bayt/bayt.
                        // health::ASSUMED_H_X10 (bit/ORNEK) DEGIL — ikisi
                        // su an ayni sayiya esit ama birimleri farkli.
                        h_per_byte_x10: conditioner::H_PER_BYTE_X10 as u16,
                        budget_x100: conditioner::BUDGET_X100 as u16,
                        out_bps,
                        total_bytes: total_out,
                        mode,
                        seed_count: rng.seed_count,
                    };
                    let _ = ui.draw(&mut disp, &snap);
                    let _ = disp.flush();
                }
            }
        }
    }
}

/// HealthTest -> ekran gorunumu.
#[cfg(not(feature = "dump"))]
fn view_of(h: &HealthTest) -> ChannelView {
    let (state, startup_left) = match h.state() {
        HealthState::Startup { remaining } => (0u8, remaining),
        HealthState::Ok => (1u8, 0),
        HealthState::Failed(_) => (2u8, 0),
    };
    ChannelView {
        state,
        startup_left,
        rct_run: h.rct_max_run,
        rct_cutoff: health::RCT_CUTOFF,
        apt_peak: h.apt_max_count,
        apt_cutoff: health::APT_CUTOFF,
    }
}

/// Saglik durumunu loglar.
///
/// FailReason'da defmt::Format YOK (kutuphane host'ta test edilebilsin diye),
/// o yuzden alanlari elle basiyoruz.
#[cfg(not(feature = "dump"))]
fn report_health(name: &str, h: &HealthTest) {
    match h.state() {
        HealthState::Startup { remaining } => {
            info!(
                "  {=str} ACILIS: {=u32} ornek kaldi (cikis yok)",
                name, remaining
            );
        }
        HealthState::Ok => {
            info!(
                "  {=str} OK: {=u64} ornek | RCT {=u32}/{=u32} | APT {=u16}/{=u16}",
                name,
                h.total_samples,
                h.rct_max_run,
                health::RCT_CUTOFF,
                h.apt_max_count,
                health::APT_CUTOFF
            );
        }
        HealthState::Failed(reason) => match reason {
            health::FailReason::Rct { value, run } => {
                error!(
                    "  {=str} ARIZA/RCT: deger {=u8} ust uste {=u32} (esik {=u32}) — CIKIS KESILDI",
                    name,
                    value,
                    run,
                    health::RCT_CUTOFF
                );
            }
            health::FailReason::Apt { value, count } => {
                error!(
                    "  {=str} ARIZA/APT: deger {=u8} pencerede {=u16} (esik {=u16}) — CIKIS KESILDI",
                    name,
                    value,
                    count,
                    health::APT_CUTOFF
                );
            }
        },
    }
}
