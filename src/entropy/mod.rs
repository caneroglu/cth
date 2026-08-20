//! Entropi hatti.
//!
//! Akis:
//!
//!   ADC 12-bit
//!      |  alt 4 bit (nibble)
//!      v
//!   [health]  RCT + APT, KANAL BASINA, ham nibble uzerinde
//!      |      FAIL -> cikis KESILIR
//!      v
//!   bayt = (Z1_nibble << 4) | Z2_nibble
//!      |
//!      v
//!   [conditioner]  160 bayt -> 32 bayt, zincirlenmis HMAC-SHA256
//!      |            (entropi butcesi: 160 x 3.4 = 544 bit >= 512 bit)
//!      +---> RAW modu: dogrudan cikis
//!      |
//!      v
//!   [drbg]  HMAC_DRBG, 1024 uretimde bir yeniden tohumlama
//!      |
//!      +---> DRBG modu: hizli cikis
//!
//! Saglik testleri neden conditioner'dan ONCE: sonrasinda her sey rastgele
//! gorunur, bozuk kaynak dahil. Eski firmware'in "whitening cikisi 7.9
//! okuyor demek ki kaynak iyi" hatasi tam buradan geliyordu.

pub mod conditioner;
pub mod drbg;
pub mod health;
