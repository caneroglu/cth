#!/usr/bin/env python3
"""
CTH ham ornek analizoru.

KULLANIM (sira onemli):
    1) python3 tools/analyze.py       <- ONCE bunu baslat, dinlemeye gecer
    2) cargo embed --release --features dump   <- SONRA bunu

Neden bu sira: cargo-embed TCP ISTEMCI (probe-rs-tools rttui/tcp.rs:21
TcpStream::connect). Yani host DINLEYEN taraf olmak zorunda.

BAGIMLILIK: numpy.
    pip3 install numpy

Onceki surum bagimliliksizdi: FFT, en kucuk kareler cozucusu ve pencere
fonksiyonu elle yazilmisti (~240 satir sayisal kod). Bakim yuku faydasindan
buyuktu; FFT ve `lstsq` gibi seyleri elde tutmak, dogruladigimiz sonuclara
yeni bir hata kaynagi eklemekten baska bir sey getirmiyordu.

CEVAP ARADIGIMIZ SORULAR
------------------------
1) ~4.9 kHz'de gordugumuz bilesen GERCEK mi, ALIAS mi, yoksa orneklemeye
   kilitli bir ARTEFAKT mi?
   Firmware 8 farkli ADC ornekleme suresinde dokum yapiyor. Buna gore:
     - tepe frekansi Hz olarak sabit      -> gercek harici sinyal
     - tepe periyodu ORNEK olarak sabit   -> orneklemeye kilitli artefakt
                                             (S&H yuk enjeksiyonu, dongu yapisi)
     - aliasing formuluyle kayiyor        -> daha yuksek frekansin katlanmasi

2) Toplam varyansin ne kadari DAR BANTLI yapi, ne kadari GENIS BANTLI gurultu?
   Spektrumdan: tepe(ler)in tasidigi guc / toplam guc. Cihazda AR(2) ile
   yapmaya calistigim ve BASARISIZ olan ayrim bu; burada spektrumla yapiliyor.

3) DAC gercekten devreye bagli mi?
   Firmware kod 0 ve 2004'te dokum yapiyor. Iki grup istatistik olarak
   ayirt edilemiyorsa DAC'in analog yola etkisi yok.

4) Kabaca kac bit ONGORULEMEZ icerik var?
   AR(16) kalintisi + MultiMMC benzeri ongorucu. Bu bir UST SINIR degil
   dogrudan tahmin; asil degerlendirme icin NIST'in `ea_non_iid` araci gerekir.
"""

import math
import socket
import struct
import sys
from collections import defaultdict

try:
    import numpy as np
except ImportError:
    sys.exit("numpy gerekiyor:  pip3 install numpy")

PORT = 19021
MAGIC = b"CTH1"
HEADER = 16

# ADC ornekleme suresi kodlari -> cevrim sayisi (STM32F1 SMPR alanlari)
SMP_CYCLES = {0: 1.5, 1: 7.5, 2: 13.5, 3: 28.5, 4: 41.5, 5: 55.5, 6: 71.5, 7: 239.5}

# Kac cerceve toplayalim (matris 2 DAC x 8 smp x 2 kanal = 32 cerceve/tur)
TARGET_FRAMES = 96

# AR modelinin mertebesi. Cihazdaki AR(2) denemesi basarisiz oldu (asagida).
AR_ORDER = 16


# --------------------------------------------------------------------------
#  Metrikler
# --------------------------------------------------------------------------
def spectrum(samples, fs_hz):
    """Tek yanli guc spektrumu. (freqs, power) dondurur."""
    x = np.asarray(samples, dtype=float)
    # Pencereleme ONCESI DC'yi cikar, yoksa DC sizmasi tepeleri gomer.
    xw = (x - x.mean()) * np.hanning(len(x))
    power = np.abs(np.fft.rfft(xw)) ** 2
    freqs = np.fft.rfftfreq(len(x), d=1.0 / fs_hz)
    return freqs, power


def peak_info(freqs, power):
    """En guclu bin (DC haric) ve tepe civarindaki guc payi."""
    if len(power) < 4:
        return 0.0, 0.0
    # k=0..1 atla (DC ve pencere sizmasi)
    k = int(np.argmax(power[2:])) + 2
    total = power[2:].sum()
    if total <= 0:
        return 0.0, 0.0
    lo, hi = max(2, k - 2), min(len(power), k + 3)  # tepe +/- 2 bin
    return float(freqs[k]), float(power[lo:hi].sum() / total)


def basic_stats(samples):
    x = np.asarray(samples, dtype=float)
    return float(x.mean()), float(x.std())


def acf1(samples):
    """Lag-1 otokorelasyon."""
    d = np.asarray(samples, dtype=float)
    d = d - d.mean()
    var = float((d * d).mean())
    return float((d[1:] * d[:-1]).mean() / var) if var > 0 else 0.0


def min_entropy_lowbits(samples, bits):
    """Alt `bits` bitin min-entropy'si (IID varsayimi): -log2(max p).

    Negatif girdi (AR kalintisi) sorun degil: maskeleme ikiye tumleyen
    alt bitleri verir, yani 0..2^bits-1 araligina duser.
    """
    v = np.asarray(samples, dtype=np.int64) & ((1 << bits) - 1)
    counts = np.bincount(v, minlength=1 << bits)
    pmax = counts.max() / counts.sum()
    return -math.log2(pmax) if pmax > 0 else 0.0


def ar_fit_residual(samples, p=AR_ORDER):
    """
    AR(p) en kucuk kareler:  x[n] ~ SUM_{i=1..p} c_i * x[n-i]

    Dondurur: (residual_std, katsayilar, kalinti) — veri yetmezse (None,)*3.

    Kalinti, DOGRUSAL olarak ongorulemeyen kisimdir. Ton, drift, ramp,
    periyodik girisim — hepsi modelin icine duser ve kalintidan cikar.

    NEDEN BU, CIHAZDAKI AR(2) DEGIL:
    Cihazda `x[n] = a*x[n-1] - x[n-2]` formunu ZORLAYIP tek parametre
    kestirmistim. Gurultu icine gomulu bir tonun tek-parametreli LS kestirimi
    sifira dogru cekilir (attenuation bias) — olculdu, a=0.54 cikti, tonu
    iptal icin 0.95 gerekiyordu. Burada p parametre serbest; model ton +
    gurultu karisimina duzgun oturuyor ve KALINTI gercek ongoru hatasi oluyor.

    Kalibrasyon (dogrulandi):
      beyaz gurultu (sigma)  -> kalinti ~ sigma      (ongorulemez)
      saf ton                -> kalinti ~ 0          (tam ongorulebilir)
      ton + gurultu(sigma)   -> kalinti ~ sigma      (gurultuyu GERI VERIR)
      dogrusal ramp/sayac    -> kalinti ~ 0
    """
    x = np.asarray(samples, dtype=float)
    n = len(x)
    if n < 4 * p + 8:
        return None, None, None

    d = x - x.mean()
    # Tasarim matrisi: satir k -> [d[k-1], d[k-2], ..., d[k-p]]
    X = np.column_stack([d[p - 1 - i : n - 1 - i] for i in range(p)])
    y = d[p:]

    # lstsq SVD kullaniyor, yani tekillik korumasi icin ridge terimi
    # gerekmiyor (elle yazilmis Gauss eliminasyonunda gerekiyordu).
    c, *_ = np.linalg.lstsq(X, y, rcond=None)
    resid = y - X @ c
    return float(resid.std()), c, resid


def markov_predictor_min_entropy(samples, bits, depth):
    """
    MultiMMC benzeri: son `depth` degere bakip en sik geleni tahmin eder.
    Sayac/ramp gibi kurallari YAKALAR (lag-1 ve MCW yakalayamiyordu).
    """
    v = (np.asarray(samples, dtype=np.int64) & ((1 << bits) - 1)).tolist()
    n = len(v)
    if n < depth + 20:
        return 0.0

    table = {}
    hits = trials = 0
    for k in range(depth, n):
        ctx = tuple(v[k - depth : k])
        counts = table.get(ctx)
        if counts:
            trials += 1
            if max(counts, key=counts.get) == v[k]:
                hits += 1
        table.setdefault(ctx, defaultdict(int))[v[k]] += 1

    if trials == 0:
        return float(bits)
    return -math.log2(max(hits / trials, 1.0 / (1 << bits)))


# --------------------------------------------------------------------------
#  Cerceve toplama
# --------------------------------------------------------------------------
def collect(target):
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", PORT))
    srv.listen(1)
    srv.settimeout(120)
    print(f"dinleniyor 127.0.0.1:{PORT} — simdi 'cargo embed --release --features dump' calistir")

    try:
        conn, addr = srv.accept()
    except socket.timeout:
        sys.exit("HATA: cargo-embed baglanmadi (120 s)")
    print(f"baglandi {addr}\n")
    conn.settimeout(60)

    buf = bytearray()
    frames = []
    last_seq = None
    gaps = 0

    while len(frames) < target:
        try:
            chunk = conn.recv(65536)
        except socket.timeout:
            print("(zaman asimi — eldekiyle devam)")
            break
        if not chunk:
            break
        buf += chunk

        # magic ile hizala ve cerceveleri cikar
        while True:
            idx = buf.find(MAGIC)
            if idx < 0:
                del buf[:-3]  # magic'in yarisi kalabilir
                break
            del buf[:idx]
            if len(buf) < HEADER:
                break
            seq, ch, smp, dac, count, elapsed = struct.unpack_from("<HBBHHI", buf, 4)
            if count == 0 or count > 4096:
                del buf[:4]  # bozuk baslik, magic'i atla
                continue
            need = HEADER + count * 2
            if len(buf) < need:
                break
            samples = np.frombuffer(bytes(buf[HEADER:need]), dtype="<u2")
            del buf[:need]

            if last_seq is not None:
                gaps += ((seq - last_seq) & 0xFFFF) - 1
            last_seq = seq

            frames.append(dict(seq=seq, ch=ch, smp=smp, dac=dac, elapsed=elapsed, s=samples))
            if len(frames) % 16 == 0:
                print(f"  {len(frames)} cerceve...")

    conn.close()
    srv.close()
    print(f"\ntoplanan: {len(frames)} cerceve, atlanan seq: {gaps}")
    return frames


# --------------------------------------------------------------------------
def main():
    frames = collect(TARGET_FRAMES)
    if not frames:
        sys.exit("cerceve gelmedi")

    # (kanal, smp, dac) -> cerceveler
    groups = defaultdict(list)
    for f in frames:
        groups[(f["ch"], f["smp"], f["dac"])].append(f)

    print("\n" + "=" * 100)
    print("SORU 1+2: tepe frekansi ornekleme hizina gore nasil davraniyor?")
    print("=" * 100)
    print(
        f"{'ch':>3} {'smp':>4} {'cevrim':>7} {'dac':>5} {'us/orn':>7} {'fs kHz':>8} "
        f"{'tepe Hz':>9} {'periyot orn':>12} {'tepe guc %':>11} {'std':>7} {'rho1':>7}"
    )
    print("-" * 100)

    rows = []
    for key in sorted(groups):
        ch, smp, dac = key
        f = groups[key][0]
        s = f["s"]
        per_us = f["elapsed"] / len(s)
        fs = 1e6 / per_us

        fpk, share = peak_info(*spectrum(s, fs))
        mean, std = basic_stats(s)
        period_samples = (fs / fpk) if fpk > 0 else 0.0

        rows.append((ch, smp, dac, per_us, fs, fpk, period_samples, share, std, acf1(s)))
        print(
            f"{ch:>3} {smp:>4} {SMP_CYCLES[smp]:>7} {dac:>5} {per_us:>7.2f} "
            f"{fs/1000:>8.1f} {fpk:>9.0f} {period_samples:>12.2f} "
            f"{share*100:>10.1f}% {std:>7.1f} {rows[-1][9]:>7.3f}"
        )

    # --- Soru 1'in karari ---
    print("\n" + "-" * 100)
    for ch in (1, 2):
        sub = [r for r in rows if r[0] == ch]
        fpks = [r[5] for r in sub if r[5] > 0]
        pers = [r[6] for r in sub if r[6] > 0]
        if len(sub) < 3 or not fpks or not pers:
            continue
        f_spread = (max(fpks) - min(fpks)) / np.mean(fpks)
        p_spread = (max(pers) - min(pers)) / np.mean(pers)
        print(f"Z{ch}: tepe frekansi bagil yayilim={f_spread:.2f}  "
              f"periyot(ornek) bagil yayilim={p_spread:.2f}")
        if f_spread < 0.15 and p_spread > 0.5:
            print(f"  -> Z{ch}: frekans Hz olarak SABIT, periyot degisiyor "
                  f"=> GERCEK harici sinyal")
        elif p_spread < 0.15 and f_spread > 0.5:
            print(f"  -> Z{ch}: periyot ORNEK olarak sabit "
                  f"=> ORNEKLEMEYE KILITLI ARTEFAKT (devrede degil, olcumde)")
        else:
            print(f"  -> Z{ch}: ikisi de degisiyor => muhtemelen ALIASING; "
                  f"gercek frekans Nyquist ustunde")

    # --- Soru 3: DAC etkisi ---
    print("\n" + "=" * 100)
    print("SORU 3: DAC devreye bagli mi? (kod 0 ile 2004 ayirt edilebiliyor mu)")
    print("=" * 100)
    for ch in (1, 2):
        for smp in sorted(SMP_CYCLES):
            a = groups.get((ch, smp, 0))
            b = groups.get((ch, smp, 2004))
            if not a or not b:
                continue
            ma, sa = basic_stats(a[0]["s"])
            mb, sb = basic_stats(b[0]["s"])
            print(f"  Z{ch} smp={smp}: std {sa:7.1f} -> {sb:7.1f} "
                  f"({abs(sb-sa)/max(sa,1e-9)*100:5.1f}% fark) | "
                  f"ort {ma:7.1f} -> {mb:7.1f} ({abs(mb-ma):5.1f} LSB kayma)")

    # --- Soru 4: gercekten ongorulemez icerik ---
    print()
    print("=" * 104)
    print(f"SORU 4: yapiyi cikardiktan sonra ne kaliyor?  "
          f"(AR({AR_ORDER}) kalintisi = ongoru hatasi)")
    print("=" * 104)
    print("Referans sinyallerle DOGRULANDI:")
    print("   beyaz s=400 -> kalinti 400 | saf ton -> 0.3 | ton+s=20 -> 21 | ramp -> 0.0")
    print("-" * 104)
    hdr = ("ch", "smp", "dac", "ham std", f"AR{AR_ORDER} kal.", "kal/ham",
           "kal.alt4", "ham alt4", "MMC d2")
    print("%3s %4s %5s | %8s %10s %8s | %9s %9s %7s" % hdr)
    print("%3s %4s %5s | %8s %10s %8s | %9s %9s %7s" %
          ("", "", "", "LSB", "LSB", "%", "H_min", "H_min", "bit"))
    print("-" * 104)

    verdict = defaultdict(list)
    for key in sorted(groups):
        ch, smp, dac = key
        s = groups[key][0]["s"]
        _, std = basic_stats(s)
        rstd, _, resid = ar_fit_residual(s)
        if rstd is None:
            continue
        print("%3d %4d %5d | %8.1f %10.2f %7.1f%% | %9.3f %9.3f %7.3f" % (
            ch, smp, dac, std, rstd, (rstd / std * 100 if std > 0 else 0.0),
            min_entropy_lowbits(np.rint(resid), 4),
            min_entropy_lowbits(s, 4),
            markov_predictor_min_entropy(s, 4, 2),
        ))
        verdict[ch].append(rstd)

    print()
    print("=" * 104)
    print("KARAR")
    print("=" * 104)
    for ch in sorted(verdict):
        med = float(np.median(verdict[ch]))
        print()
        print("Z%d: AR(%d) kalinti std ortancasi = %.2f LSB" % (ch, AR_ORDER, med))
        if med < 3:
            print("  -> ADC kuantizasyon tabani seviyesi. Yapiyi cikardiginda pratikte")
            print("     HICBIR SEY kalmiyor. Bu kaynak fiziksel entropi uretmiyor;")
            print("     ustune SP 800-90B makinesi kurmak sadece cila olur.")
        elif med < 15:
            print("  -> ADC tabaninin uzerinde ama ZAYIF (~%d bit/ornek)." % max(0, int(math.log2(med))))
            print("     Kullanilabilir; hedef hiz icin kazanc/bant genisligi artmali.")
        else:
            print("  -> GERCEK genis bantli gurultu var (%.0f LSB)." % med)
            print("     Yapi bir sorun ama altinda olculebilir entropi bulunuyor:")
            print("     ton/drift filtrelenir, conditioner kalintidan beslenir.")
            print("     Kaba ust sinir: ~%.1f bit/ornek." % math.log2(med))

    print()
    print("NOT: 'ham alt4 H_min' bagimsizlik VARSAYAR ve yapi varsa oldugundan buyuk cikar.")
    print("'kal.alt4 H_min' ise ongorulebilir kisim cikarildiktan SONRAKI degerdir.")
    print("Ikisi arasindaki fark, ham sayilarin ne kadar aldatici oldugunun olcusudur.")


if __name__ == "__main__":
    main()
