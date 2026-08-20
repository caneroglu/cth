/* ==========================================================================
 *  memory.x — STM32F103C8T6 (Blue Pill) bellek haritasi
 * ==========================================================================
 *
 *  Bu dosyayi LINKER okur, derleyici degil. Zincir soyle isliyor:
 *
 *    .cargo/config.toml   "-C link-arg=-Tlink.x"
 *            |
 *            v
 *    cortex-m-rt/link.x   icinde "INCLUDE memory.x" var (link.x.in:22)
 *            |
 *            v
 *    build.rs             bu dosyayi OUT_DIR'e kopyalar ve OUT_DIR'i
 *                         "cargo:rustc-link-search" ile linker'in arama
 *                         yoluna ekler => INCLUDE memory.x cozulur.
 *
 *  Yani build.rs'teki o iki satir olmadan link "cannot find memory.x" ile
 *  patlar. Ucu birbirine bagli: config.toml -> link.x -> memory.x.
 *
 *  ONEMLI — TEK SAHIPLIK:
 *  embassy-stm32'nin "memory-x" feature'i da otomatik memory.x URETIR.
 *  Ikisi birden acik olursa linker hangisini bulacagi -L bayraklarinin
 *  sirasina kalir; bu belirsiz davranistir. O yuzden Cargo.toml'da
 *  embassy-stm32'nin "memory-x" feature'i KAPALI. Bellek haritasinin tek
 *  sahibi bu dosyadir.
 *
 *  --------------------------------------------------------------------------
 *  DONANIM — tahmin degil, probe uzerinden okundu (2026-08-17):
 *
 *    DBGMCU_IDCODE  0xE0042000 = 0x20036410
 *                                  ^^^^ DEV_ID 0x410 = F103 medium-density
 *                                  REV_ID 0x2003
 *    Flash size reg 0x1FFFF7E0 = 0x0040  -> 64 KB
 *    Unique ID      0x1FFFF7E8 = 0671FF55 56487866 87013149
 *
 *  128 KB MESELESI (Blue Pill folkloru):
 *  0x08010000 ve 0x0801F000 okundugunda bus fault gelmedi, 0xFFFFFFFF
 *  dondu. Bu "C8 aslinda CB die'inin kirpilmisi, 128 KB fiziken var"
 *  tezine IPUCTUR ama KANIT DEGILDIR: F1'de tanimsiz flash adresi de
 *  0xFF okur. Kanit icin 0x08010000'e yaz + geri oku + dogrula testi
 *  gerekir. Flash sikisirsa o testi kontrollu yapip asagidaki LENGTH'i
 *  128K'ya cikaririz. O test yapilmadan 64K'nin uzerine cikmak, gecen
 *  hafta calisan firmware'in bu hafta rastgele bricklenmesi demektir.
 * ========================================================================== */

MEMORY
{
  /* Dahili Flash.
   * Vector table tam 0x08000000'da baslar: reset sonrasi CPU ilk iki word'u
   * buradan okur (word0 = initial stack pointer, word1 = reset vector).
   * Bu yuzden ORIGIN'i kaydirmak istiyorsan (bootloader vs.) VTOR'u da
   * elle tasimak zorundasin. */
  FLASH : ORIGIN = 0x08000000, LENGTH = 64K

  /* Dahili SRAM.
   * Yerlesim: .data ve .bss alttan yukari buyur, stack tepeden asagi iner.
   * Ikisi ortada bulusursa STACK OVERFLOW olur. */
  RAM   : ORIGIN = 0x20000000, LENGTH = 20K
}

/* .text'in baslangici — 8 bayt hizali.
 *
 * Neden gerekli: Cortex-M3'te vector table = 16 sistem + 43 cihaz IRQ'su
 * = 59 word = 236 bayt (0xEC). cortex-m-rt varsayilan olarak .text'i hemen
 * ardina, yani 0x080000EC'ye koyuyor. Bu adres 4'un kati ama 8'in katI
 * DEGIL, ve LLVM .text icin 8 hizalama istedigi icin lld su uyariyi basiyor:
 *
 *   rust-lld: address (0x80000ec) of section .text is not a multiple
 *             of alignment (8)
 *
 * 0xF0'a cekince uyari kalkiyor, bedeli 4 bayt flash.
 *
 * Sabit deger yazmak neden guvenli: link.x:272'de su ASSERT var —
 *   ADDR(.vector_table) + SIZEOF(.vector_table) <= _stext
 * Yani vector table bir gun 0xF0'i asarsa build SESSIZCE bozulmaz, acik
 * hata mesajiyla durur. Sabit degeri koruyan bir bekcimiz var.
 *
 * (Not: bu uyari --nmagic'ten kaynaklanmiyor; --nmagic'siz de olusuyor.
 *  Olculdu, ikisinde de ayni.) */
_stext = ORIGIN(FLASH) + 0xF0;

/* Stack'in tepesi (asagi dogru buyur).
 * cortex-m-rt zaten varsayilan olarak RAM'in sonunu kullanir; burada acikca
 * yaziyoruz ki haritaya bakan kisi stack'in nerede oldugunu gorsun.
 *
 * DIKKAT: STM32F103'te stack guard yok (MPU var ama cortex-m-rt bunu
 * kurmuyor). Stack tasarsa donanim uyarmaz — sessizce .bss'i ezer ve hata
 * bambaska bir yerde, saatler sonra ortaya cikar.
 * Bu projede somut risk: her [u32; 256] histogram 1 KB stack yer.
 * Bu yuzden buyuk tamponlar stack'te degil static olarak tutulacak. */
_stack_start = ORIGIN(RAM) + LENGTH(RAM);
