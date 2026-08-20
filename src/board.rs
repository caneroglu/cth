//! Kart tanimi — pin haritasi, saat, periferik kurulumu.
//!
//! Bu dosya kartin TEK tanimi. Pin haritasi baska hicbir yerde tekrarlanmaz.
//!
//! Neden boyle: eski firmware'de pin haritasi 385 satirlik bir `main()` icine
//! dagilmisti. Analog bir kartta bu ciddi bir risk — mux adres pinlerinden
//! birini yanlis yazmak, geri besleme yoluna yanlis direnc sokup 2. kat
//! op-amp'i tanimsiz kazancta calistirir ve bunu HICBIR log soylemez.
//!
//! ======================= PIN HARITASI =======================
//!
//!  ANALOG GIRIS
//!    PA0   ADC1_IN0   Zener 1 (2. kat op-amp cikisi)
//!    PA1   ADC2_IN1   Zener 2 (2. kat op-amp cikisi)
//!
//!  SPI1 -> MCP4922 DAC (zener bias akimi)
//!    PA5   SPI1_SCK
//!    PA7   SPI1_MOSI
//!    PA4   CS         (aktif dusuk)
//!    PA3   LDAC       (dusen kenar = cikisa aktar)
//!    PA6   SHDN       (HIGH = cip aktif) -- surekli HIGH tutulmali
//!
//!  MUX Z1 -> HC4067 (Z1 kazanc bankasi)
//!    PB7   S0
//!    PB6   S1
//!    PB12  S2
//!    PA9   S3
//!    PA2   INH        (HIGH = tum kanallar kapali)
//!
//!  MUX Z2 -> HC4067 (Z2 kazanc bankasi)
//!    PA15  S0    <-- JTAG kapatilmadan KULLANILAMAZ
//!    PA10  S1
//!    PB5   S2
//!    PB4   S3    <-- JTAG kapatilmadan KULLANILAMAZ
//!    PB3   INH   <-- JTAG kapatilmadan KULLANILAMAZ
//!
//!  I2C1 (PAYLASIMLI: EEPROM + OLED)
//!    PB8   SCL        (remap)
//!    PB9   SDA        (remap)
//!                     AT24C256 @ 0x57 , SSD1306 @ 0x3C
//!
//!  SONRAKI KATMANLAR (pinler ayrilmis, bu katmanda kurulmuyor)
//!    PB10  DS18B20 1-Wire  (sicaklik task'i)
//!    PB0   TIM3_CH3 PWM    (sari LED, entropi kalitesi gostergesi)
//!
//!  GOSTERGE
//!    PB1   yesil LED  (veri aktivitesi)
//!    PC13  dahili LED (kart uzerinde, aktif DUSUK) -- canlilik
//!
//! ============================================================
//!
//! SWD ZORUNLU, JTAG DEGIL: PA15/PB4/PB3 mux Z2'ye ayrilmis durumda. JTAG
//! acik kalirsa o pinler debugger'in elinde olur ve Z2 kazanci kontrol
//! edilemez. `init()` icinde SWJ_CFG = JTAG_DISABLE yapiyoruz (SWD kalir).

use embassy_stm32::adc::Adc;
use embassy_stm32::gpio::{Level, Output, Speed};
use embassy_stm32::i2c::{I2c, Master as I2cMaster};
use embassy_stm32::mode::Blocking;
use embassy_stm32::peripherals::{ADC1, ADC2, PA0, PA1};
use embassy_stm32::rcc::{
    ADCPrescaler, AHBPrescaler, APBPrescaler, Hse, HseMode, Pll, PllMul, PllPreDiv, PllSource,
    Sysclk,
};
use embassy_stm32::spi::{Config as SpiConfig, Spi};
use embassy_stm32::time::Hertz;
use embassy_stm32::{Peri, bind_interrupts, adc};

use crate::drivers::hc4067::Hc4067;
use crate::drivers::mcp4922::Mcp4922;

// ADC1 ve ADC2 ayni interrupt vektorunu paylasiyor (ADC1_2).
//
// Bu blok "kullanilmiyor" gibi gorunur — `Irqs` tipi hicbir yerde gecmez —
// ama SILINEMEZ. bind_interrupts! makrosunun asil isi
// `#[no_mangle] extern "C" fn ADC1_2()` vektorunu URETMEK.
// `Adc::new()` interrupt'i kendisi enable ediyor (embassy-stm32 adc/f1.rs:65)
// ve async `read()` EOC interrupt'inin waker'i uyandirmasina guveniyor.
// Vektor bagli degilse DefaultHandler kosar, EOCIE hic temizlenmez ve
// firmware interrupt firtinasinda kilitlenir.
bind_interrupts!(struct Irqs {
    ADC1_2 => adc::InterruptHandler<ADC1>, adc::InterruptHandler<ADC2>;
});

/// ADC saati: 72 MHz / 8 = 9 MHz.
///
/// F103'te ADC saati en fazla 14 MHz olabilir; DIV6 ile 12 MHz de mumkundu.
/// DIV8 (9 MHz) eski firmware'in secimi, kiyas tabanini korumak icin ayni
/// tutuyoruz. Entropi/saniye olcumu yapabildigimizde bu bir deney degiskeni
/// olacak.
const ADC_PRESCALER: ADCPrescaler = ADCPrescaler::DIV8;

/// Kart: kurulmus ve kullanima hazir periferikler.
pub struct Board {
    pub dac: Mcp4922<'static>,
    pub mux_z1: Hc4067<'static>,
    pub mux_z2: Hc4067<'static>,

    /// I2C1 — EEPROM ve OLED ORTAK kullanir. Task'lar devreye girince
    /// mutex arkasina alinacak.
    pub i2c: I2c<'static, Blocking, I2cMaster>,

    pub adc1: Adc<'static, ADC1>,
    pub adc2: Adc<'static, ADC2>,
    /// Z1 analog girisi — `adc1.read(&mut pin_z1, ...)` icin.
    pub pin_z1: Peri<'static, PA0>,
    /// Z2 analog girisi — `adc2.read(&mut pin_z2, ...)` icin.
    pub pin_z2: Peri<'static, PA1>,

    /// Kart uzerindeki LED (PC13, aktif DUSUK).
    pub led_status: Output<'static>,
    /// Yesil LED (PB1) — veri aktivitesi.
    /// SADECE RTT'ye gercekten blok yazildiginda toggle ediliyor. Bosta ya da
    /// zamanlayiciyla yanip sonseydi "veri akiyor" yanilgisi yaratirdi; bu
    /// LED'in tek isi cikis gercekten aktigini gostermek.
    pub led_data: Output<'static>,

    /// MCP4922 SHDN pini. HIGH kalmak ZORUNDA; dusürse DAC cikisi kesilir
    /// ve zenerler akimsiz kalir. Sadece canli tutmak icin saklaniyor.
    _dac_shdn: Output<'static>,
}

impl Board {
    /// Saatleri kurar, JTAG'i kapatir, periferikleri baslatir.
    ///
    /// Cikista DAC iki kanali da 0'da (zenerlere akim YOK) ve iki mux da
    /// KAPALI. Kalibrasyon degerleri okunduktan sonra bilincli olarak
    /// ayarlanmalari gerekir.
    pub fn init() -> Self {
        let p = embassy_stm32::init(clock_config());

        // JTAG'i kapat -> PA15 / PB4 / PB3 serbest kalir (mux Z2 icin sart).
        // SWD acik kalir, yani probe baglantisini kaybetmiyoruz.
        embassy_stm32::pac::AFIO.mapr().modify(|w| {
            w.set_swj_cfg(embassy_stm32::pac::afio::vals::SwjCfg::JTAG_DISABLE);
        });

        // --- SPI1 + MCP4922 ---
        // 4 MHz: MCP4922 20 MHz'e kadar dayaniyor, 4 MHz bol bol yeterli ve
        // analog karta daha az anahtarlama gurultusu enjekte eder.
        let mut spi_cfg = SpiConfig::default();
        spi_cfg.frequency = Hertz(4_000_000);
        let dac_spi = Spi::new_blocking_txonly(p.SPI1, p.PA5, p.PA7, spi_cfg);

        // SHDN'i DAC'tan ONCE HIGH'a cek, yoksa ilk yazmalar yutulur.
        let dac_shdn = Output::new(p.PA6, Level::High, Speed::Low);
        let dac = Mcp4922::new(
            dac_spi,
            Output::new(p.PA4, Level::High, Speed::Low), // CS, aktif dusuk
            Output::new(p.PA3, Level::High, Speed::Low), // LDAC
        );

        // --- MUX Z1 --- (INH pini Hc4067::new icinde HIGH'a cekiliyor)
        let mux_z1 = Hc4067::new(
            Output::new(p.PB7, Level::Low, Speed::Low),
            Output::new(p.PB6, Level::Low, Speed::Low),
            Output::new(p.PB12, Level::Low, Speed::Low),
            Output::new(p.PA9, Level::Low, Speed::Low),
            Output::new(p.PA2, Level::High, Speed::Low),
        );

        // --- MUX Z2 --- (JTAG kapali oldugu icin PA15/PB4/PB3 kullanilabilir)
        let mux_z2 = Hc4067::new(
            Output::new(p.PA15, Level::Low, Speed::Low),
            Output::new(p.PA10, Level::Low, Speed::Low),
            Output::new(p.PB5, Level::Low, Speed::Low),
            Output::new(p.PB4, Level::Low, Speed::Low),
            Output::new(p.PB3, Level::High, Speed::Low),
        );

        // --- I2C1 (PB8/PB9) ---
        let i2c = I2c::new_blocking(p.I2C1, p.PB8, p.PB9, Default::default());

        // --- Dual ADC ---
        // Iki ayri ADC kullanmak Z1 ve Z2'yi bagimsiz orneklememizi sagliyor;
        // tek ADC ile multiplekslemek zorunda kalsaydik iki kanal arasinda
        // zorunlu bir zaman kaymasi olurdu.
        let adc1 = Adc::new(p.ADC1);
        let adc2 = Adc::new(p.ADC2);

        Self {
            dac,
            mux_z1,
            mux_z2,
            i2c,
            adc1,
            adc2,
            pin_z1: p.PA0,
            pin_z2: p.PA1,
            led_status: Output::new(p.PC13, Level::High, Speed::Low), // HIGH = sonuk
            led_data: Output::new(p.PB1, Level::Low, Speed::VeryHigh),
            _dac_shdn: dac_shdn,
        }
    }
}

/// Saat agaci: HSE 8 MHz -> PLL x9 -> 72 MHz.
///
/// Neden tam 72 MHz:
///   - F103'un azami sistem saati.
///   - Gerekirse USB: embassy'nin F1 RCC'si PLL=72 MHz gorunce USBPRE=DIV1_5'i
///     KENDISI ayarlayip 48 MHz USB saati uretiyor (rcc/f013.rs:299-305).
///     Yani USB'ye gecmek istersek saat tarafinda yapacak is yok.
///
/// Sinirlar: APB1 <= 36 MHz (=> DIV2), APB2 <= 72 MHz (=> DIV1),
///           ADC <= 14 MHz (=> DIV8 ile 9 MHz).
pub fn clock_config() -> embassy_stm32::Config {
    let mut config = embassy_stm32::Config::default();
    config.rcc.hse = Some(Hse {
        freq: Hertz(8_000_000),
        mode: HseMode::Oscillator,
    });
    config.rcc.pll = Some(Pll {
        src: PllSource::HSE,
        prediv: PllPreDiv::DIV1,
        mul: PllMul::MUL9,
    });
    config.rcc.sys = Sysclk::PLL1_P;
    config.rcc.ahb_pre = AHBPrescaler::DIV1;
    config.rcc.apb1_pre = APBPrescaler::DIV2;
    config.rcc.apb2_pre = APBPrescaler::DIV1;
    config.rcc.adc_pre = ADC_PRESCALER;
    config
}
