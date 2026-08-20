//! CTH — entropi hattinin DONANIMDAN BAGIMSIZ kismi.
//!
//! Neden ayri bir kutuphane: buradaki mantik (saglik testleri, conditioner,
//! DRBG) saf hesap. Kutuphane olarak ayirinca HOST'ta `cargo test` ile
//! kosabiliyoruz — esikler, kilitlenme davranisi, yanlis alarm oranlari
//! donanim olmadan dogrulanabiliyor.
//!
//! Donanima bagli kisimlar (board, drivers, ADC dongusu) binary'de kaliyor.
//!
//! `#![cfg_attr(not(test), no_std)]`: hedefte no_std, host testinde std.
//! Test kosumcusunun (libtest) std'ye ihtiyaci var.

#![cfg_attr(not(test), no_std)]

pub mod entropy;
