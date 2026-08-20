//! Build script.
//!
//! Iki isi var:
//!   1. `memory.x`'i linker'in bulabilecegi bir yere koymak (bu olmadan link
//!      "cannot find memory.x" ile patlar).
//!   2. Firmware'e kimlik basmak: derleme zamani + git commit'i. Bir QRNG'de
//!      uretilen raporun hangi firmware'den ciktigi kanitlanabilir olmali.
//!
//! Bu dosya HOST'ta kosar (x86 Windows), hedefte degil. `std` kullanabilir.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    place_memory_x();
    stamp_provenance();

    println!("cargo:rerun-if-changed=build.rs");
}

/// `memory.x`'i OUT_DIR'e kopyalar ve OUT_DIR'i linker'in arama yoluna ekler.
///
/// Neden kopyalamak gerekiyor: cortex-m-rt'nin `link.x` betigi icinde duz bir
/// `INCLUDE memory.x` var. Linker bunu ararken sadece `-L` ile verilen arama
/// yollarina bakar; projenin kok dizini o listede DEGIL. Dolayisiyla dosyayi
/// zaten arama yolunda olan bir dizine (OUT_DIR) tasiyip o dizini kaydediyoruz.
fn place_memory_x() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR tanimli degil"));
    let dest = out_dir.join("memory.x");

    fs::copy("memory.x", &dest).unwrap_or_else(|e| {
        panic!(
            "memory.x -> {} kopyalanamadi: {e}\n\
             Proje kokunde memory.x var mi?",
            dest.display()
        )
    });

    println!("cargo:rustc-link-search={}", out_dir.display());
    println!("cargo:rerun-if-changed=memory.x");
}

/// Firmware kimligi: `BUILD_EPOCH` ve `GIT_DESCRIBE` derleme-zamani env
/// degiskenleri olarak gomulur, kodda `env!("BUILD_EPOCH")` ile okunur.
///
/// BAYAT DAMGA UYARISI — bu bilinen bir sinirlama, surpriz degil:
/// Cargo build script'ini yalnizca `rerun-if-*` tetiklendiginde yeniden kosar.
/// Yani `BUILD_EPOCH`, "en son build.rs / memory.x / .git degisiminde" ne ise
/// o donar; her `cargo build`de tazelenmez.
///
/// Damgayi zorla tazelemek icin:
///     SOURCE_DATE_EPOCH=$(date +%s) cargo embed --release
///
/// KALICI COZUM (bu projede yapilacak): zamani derleme aninda hic gommemek,
/// host'tan runtime'da almak. RTT down-channel'i tam da bunun icin var —
/// kalibrasyon zaman damgasi artik derleyicinin degil, host'un saatinden
/// gelecek. O is bitince buradaki BUILD_EPOCH tamamen kaldirilabilir.
fn stamp_provenance() {
    // SOURCE_DATE_EPOCH: reproducible-builds standardi. Verilirse ona uyuyoruz;
    // hem tekrarlanabilir build hem de "damgayi zorla tazele" kolu ayni yer.
    let epoch = env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        });
    println!("cargo:rustc-env=BUILD_EPOCH={epoch}");
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");

    let git = git_describe().unwrap_or_else(|| "nogit".to_string());
    println!("cargo:rustc-env=GIT_DESCRIBE={git}");

    // Commit / checkout / staging degisince damga tazelensin.
    // Not: git worktree veya submodule'de ".git" dizin degil DOSYA olabilir;
    // rerun-if-changed ikisiyle de calisir, o yuzden sadece varligina bakiyoruz.
    for path in [".git/HEAD", ".git/index"] {
        if fs::metadata(path).is_ok() {
            println!("cargo:rerun-if-changed={path}");
        }
    }
}

/// `git describe --always --dirty` ciktisi; git yoksa veya repo degilse `None`.
///
/// `--dirty=+` : commit edilmemis degisiklik varsa sonuna `+` ekler. Yani
/// `a1b2c3d4+` gorursen o firmware temiz bir commit'ten URETILMEMISTIR —
/// imzali rapor uretirken bu bilgi kritik.
fn git_describe() -> Option<String> {
    let out = Command::new("git")
        .args(["describe", "--always", "--dirty=+", "--abbrev=8"])
        .output()
        .ok()?;

    if !out.status.success() {
        return None;
    }

    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}
