//! Donanim surucüleri.
//!
//! Kural: hicbir surucu I2C/SPI bus'ini SAHIPLENMEZ eger o bus paylasimliysa.
//! I2C1'de EEPROM + OLED birlikte durdugu icin at24c256 `&mut I2c` aliyor.
//! SPI1'de sadece DAC var, o yuzden mcp4922 spi'yi sahipleniyor.

#![allow(dead_code)]

pub mod at24c256;
pub mod hc4067;
pub mod mcp4922;
