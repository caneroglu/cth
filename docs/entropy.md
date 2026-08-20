# Entropi metodolojisi

Bu doküman şunu anlatıyor: cihazın ürettiği verinin rastgeleliği **nasıl
ölçüldü**, hangi iddialar **destekleniyor**, hangileri **desteklenmiyor**.

Bir RNG projesinde en çok sorulan soru "min-entropy'niz kaç" değil, "bunu
nereden biliyorsunuz" olmalı. Aşağıdakiler tekrar edilebilir ölçümlerdir.

## Birimler

Entropi rakamları paydası söylenmeden anlamsızdır ve bu belgede **üç ayrı
payda** geçiyor. Karıştırılmaması için:

| büyüklük | payda | değer | dayanak |
|---|---|---|---|
| gürültü kaynağı | **bit / örnek** (4-bit nibble) | 3.34 – 3.71 | SP 800-90B'nin değerlendirme birimi; NIST `ea_non_iid` da `H_original`'ı böyle basar |
| conditioner girdisi | **bit / bayt** | 3.4 | bayt = `(Z1<<4)\|Z2`; iki kanal 6.8 sayılmıyor, muhafazakâr olarak tek kanal alınıyor (§6) |
| conditioner çıktısı | **bit / bayt** | 8.00 (tam entropi) | SP 800-90C; tam entropi aslında çıkış **biti** başına tanımlıdır (1.00 bit/bit), 8.00 bit/bayt onun bayt cinsinden yazılışı |

Karşılaştırma için: BSI AIS-31 de çıkış tarafını **bit başına** konuşur
(PTG.2 sınıfı: iç rastgele bit başına Shannon entropisi > 0.997).

Cihaz ekranındaki `ent` satırı okun **iki ucunda da bit/bayt** kullanır
(`3.4 -> 8.00 b/B`) — paydalar aynı olmasa ok bir dönüşüm ima edip yanıltırdı.
Kaynağın SP 800-90B birimi (bit/örnek) ekranda değil, `s` durum raporunda.

---

## 1. Neden basit testler yetmez

Bir bayt akışı chi-square, monobit, runs, hatta dieharder'ın büyük kısmını
geçebilir ve yine de **tamamen öngörülebilir** olabilir. π'nin basamakları
geçer. Bir sayacı SHA-256'dan geçirirsen o da geçer.

Bu testler **düzgünlüğü** ölçer, **öngörülemezliği** değil. SP 800-90B'nin
varlık sebebi tam olarak budur: çıktının dağılımına bakmak yetmez, kaynağın
kendisi değerlendirilmelidir.

Bu proje bu tuzağa iki kez düştü ve ikisi de ölçümle yakalandı:

### 1a. Whitening çıkışı kalite göstergesi değil

Eski firmware ekranda bir "VN" satırı gösteriyordu: SHA-256'dan geçmiş verinin
Shannon entropisi. Değer **7.894 bit/bayt** okuyordu ve bu "kaynak iyi"
kanıtı sanılıyordu.

Ölçüm: aynı tahmincinin tavanı (mükemmel rastgele girdiyle) **7.898**. Fark
**0.004 bit**. SHA-256'ya ne verirsen tavanı verir — sayaç ver, sıfır ver,
deterministik bir tonun alt bitlerini ver. O satır kaynağı değil, hash
fonksiyonunun kendisini ölçüyordu.

### 1b. "8.00 bit/bayt" hedefi ulaşılamazdı

Ekranda `Theory.Max: 8.00` yazıyordu. Firmware'in tamsayı tahmincisi host'ta
birebir taklit edilip mükemmel rastgele veri verildiğinde:

| ölçüm | N | tahmincinin gerçek tavanı |
|---|---|---|
| Z1 / Z2 / XOR | 8128 | **7.928** |
| VN (whitened) | 4064 | **7.898** |

Kaybın kaynağı iki tane ve ikisi de hesaplanabilir:

- **~0.023 bit** histogram sapması (Miller-Madow: `(K−1)/(2N ln2)`, K=256)
- **~0.049 bit** `log2_fixed`'in ikinin kuvvetleri arasında **doğrusal**
  interpolasyon yapması (log2 içbükey olduğu için bu daima **düşük** tahmin
  verir)

Yani her sayı, asla vurulamayacak bir hedefle kıyaslanıyordu.

> **Yeni firmware ekranda `8.00 b/B` yazıyor — bu o hatanın tekrarı değil.**
> Ayrım sayının nereden geldiğinde: eskiden whitened akış **ölçülüyordu**
> (ve hash tavanı çıkıyordu); şimdi hiçbir şey ölçülmüyor, `8.00` §6'daki
> entropi bütçesinden **türetiliyor**. Test: kaynağı öldür. Eski satır
> 7.894'te kalırdı; yeni satır sağlık testleri düştüğü an ekrandan kaybolur.

---

## 2. Kaynağın gerçekten fiziksel olduğunun ölçümü

Sorun şu: sinyalin varyansının **%74'ü ~4.88 kHz'lik bir tondan** geliyor.
Deterministik bir tonun alt bitleri de düzgün görünür. O yüzden "ne kadarı
gerçek gürültü" sorusunu ayrıştırmak gerekti.

### Yöntem: AR(16) öngörü kalıntısı

Sinyale `p=16` mertebeli bir otoregresif model en küçük karelerle oturtulur:

```
x[n] ≈ Σ c_i · x[n−i]        i = 1..16
kalıntı r[n] = x[n] − tahmin
```

Kalıntı, **doğrusal olarak öngörülemeyen** kısımdır. Ton, drift, ramp,
periyodik girişim — hepsi modelin içine düşer ve kalıntıdan çıkar.

> **Neden cihazda değil host'ta:** cihazda tek parametreli AR(2) denendi ve
> **başarısız oldu**. Gürültü içine gömülü bir tonun tek parametreli LS
> kestirimi sıfıra doğru çekilir (attenuation bias): `a_LS ≈ 2ρ₁ = 0.54`
> çıktı, tonu iptal etmek için `2cos(ωT) = 0.95` gerekiyordu. Kalıntının
> `ρ₁`'i −0.50 kaldı, yani model oturmadı. Host'ta p parametre serbest
> bırakılınca sorun kalkıyor.

### Aracın kalibrasyonu

Yöntemi veriye uygulamadan önce bilinen sinyallerle doğrulandı:

| enjekte edilen sinyal | ham std | AR(16) kalıntı | beklenen |
|---|---|---|---|
| beyaz gürültü σ=400 | 404.7 | **399.8** | ≈400 ✓ |
| saf ton | 282.7 | **0.32** | ≈0 ✓ |
| ton + gürültü σ=20 | 283.5 | **20.7** | ≈20 ✓ |
| ton + gürültü σ=100 | 303.2 | **108.6** | ≈100 ✓ |
| ramp / sayaç | 1034.6 | **0.00** | ≈0 ✓ |

Araç, enjekte edilen gürültüyü her vakada geri veriyor ve deterministik
sinyallerde sıfır diyor.

### Sonuç

| kanal | ham std | AR(16) kalıntı | kalıntının alt-4-bit min-entropy |
|---|---|---|---|
| Z1 | ~445 | **197.5 LSB** | **3.34 – 3.71 bit** |
| Z2 | ~412 | **184.2 LSB** | **3.34 – 3.71 bit** |

Karşılaştırma için: saf ton referansında kalıntının alt-4-bit min-entropy'si
**0.157**'ye çöküyor. Burada 3.5 civarında kalıyor.

**Yorum:** yapı çıkarıldıktan sonra örnek başına ~3.4 bit öngörülemez içerik
kalıyor. Bu gerçek fiziksel gürültüdür.

---

## 3. ~4.88 kHz tonun gerçek olduğunun ölçümü

Firmware 8 farklı ADC örnekleme süresinde (9.12 – 35.46 µs/örnek) döküm
yapıyor. Üç senaryo ayırt ediliyor:

| gözlem | anlam |
|---|---|
| frekans Hz olarak sabit | gerçek harici sinyal |
| periyot **örnek** olarak sabit | örneklemeye kilitli artefakt |
| aliasing formülüne göre kayar | daha yüksek frekansın katlanması |

Ölçülen: tepe frekansı bağıl yayılım **0.04** (Z1) / **0.02** (Z2); örnek
cinsinden periyot **5.75 → 23.27** (yayılım 1.27 / 0.83).

Frekans sabit, periyot değişiyor → **gerçek harici sinyal**. En yavaş
örnekleme hızında bile (`fs = 28.2 kHz`, Nyquist 14.1 kHz) 4.88 kHz Nyquist'in
altında, yani aliasing doğrudan dışlanıyor.

Kaynağı bulunamadı. MCU'dan gelmiyor (bu firmware'de PWM, OLED ve DS18B20
başlatılmıyor). Analog beslemeyi üreten bir anahtarlamalı devre en güçlü aday.

---

## 4. Örnekleme süresi seçimi

Alt 4 bit alınıyor çünkü ADC'nin INL/DNL hatası ve kazanç kayması üst bitlerde
birikir. Ama örnekleme **hızı** da önemli çıktı:

| ADC örnekleme | µs/örnek | ham alt-4 min-entropy | MMC öngörücü |
|---|---|---|---|
| `CYCLES1_5` | 9.12 | **1.89 – 2.02** | 1.50 |
| `CYCLES239_5` | 35.46 | **3.33 – 3.57** | 4.00 |

Hızlı örneklerken ardışık örnekler zamanda çok yakın kalıyor ve alt bitler
korele oluyor. Firmware `CYCLES239_5` kullanıyor.

---

## 5. Sağlık testleri — eşikler nereden geliyor

Sürekli testler (SP 800-90B bölüm 4.4) ham nibble akışı üzerinde, **kanal
başına ayrı** koşuyor. Ortak tek test kursaydık bir kanalın ölümü diğerinin
arkasında gizlenirdi.

`H = 3.4 bit/örnek` (ölçülen aralığın muhafazakar ucu), `α = 2⁻²⁰`:

**RCT (Repetition Count Test)**

```
C = 1 + ceil(−log2(α) / H) = 1 + ceil(20 / 3.4) = 7
```

Aynı nibble 7 kez üst üste gelirse arıza.

**APT (Adaptive Proportion Test)**, `W = 512`

```
p = 2^−H = 0.09473     ortalama = 48.5     std = 6.63
P(X ≥ C) ≤ α veren en küçük C  =  84       (= ortalama + 5.36σ)
```

Farklı H varsayımları için: H=2.0 → RCT 11 / APT 177 · H=3.0 → 8 / 103 ·
H=3.7 → 7 / 72.

**Arıza politikası:** kilitlenir. Bir kez FAIL olunca çıkış kesilir ve
`h` komutu (veya reset) gelene kadar FAIL kalır. Otomatik toparlanma bilerek
yok — arızalı bir kaynak arada bir test geçip veri sızdırmasın.

### Donanımda ölçülen davranış

245.756 örnek/kanal, arızasız:

| test | ölçülen | eşik |
|---|---|---|
| RCT en uzun tekrar | **5** | 7 |
| APT tepe (Z1 / Z2) | **53 / 51** | 84 |

APT tepesinin teorik ortalamayla (48.5) örtüşmesi bağımsız bir doğrulama:
varsayılan `H = 3.4`, kaynağın gerçek davranışıyla sayısal olarak tutarlı.

---

## 6. Conditioner — entropi bütçesi

Zincirlenmiş HMAC-SHA256:

```
durum ← HMAC-SHA256(anahtar = durum, mesaj = girdi bloğu)
```

Anahtarın zincirlenmesi entropinin bloklar boyunca **birikmesini** sağlıyor.

### Eski tasarımın açığı

```
64 bayt girdi × 3.4 bit  =  217 bit entropi
                         →  32 bayt (256 bit) çıktı
                            217 < 256   ← AÇIK
```

Çıktı yine "rastgele görünüyordu" (hash'in kendisi öyle) ama iddia ettiği
entropiyi karşılamıyordu. Ayrıca bloklar birbirinden bağımsız hash'leniyordu,
yani durum taşınmıyordu.

### Şimdiki bütçe

Çıktı biti başına ≥1 bit girdi entropisi, üstüne 2× emniyet payı:

```
gerekli  = 8 × 32 × 2 = 512 bit
N        = ceil(512 / 3.4) = 151  →  160 bayta yuvarlandı
sağlanan = 160 × 3.4 = 544 bit    ≥  512 ✓   (pay 2.13×)
```

`BLOCK_IN = 160`, `BLOCK_OUT = 32`. Bu bütçe kodda bir `const` assert ile
korunuyor: oranlarla oynanırsa **derleme durur**, sessizce açık verilmez.

### Sonuç: çıktı tam entropi

SP 800-90C'nin tam-entropi koşulu, **onaylı (vetted)** bir conditioning
fonksiyonuna `h_in ≥ 2n` verilmesidir. Burada `h_in = 544 bit`, `n = 256 bit`,
oran **2.12×** → koşul sağlanıyor. Dolayısıyla `RAW` modunda conditioner
çıktısı **bayt başına 8.00 bit** taşır.

> **Bu bir ölçüm değil, bir türetmedir** — ve aradaki fark bu projede kritik.
> §1a'daki hata, whitened akışı bir tahminciye sokup çıkan sayıyı kalite
> göstergesi sanmaktı; o sayı kaynak ölüyken de aynı kalıyordu. Buradaki
> `8.00` ise hiç ölçülmüyor: ölçülen `3.4`'ten ve sıkıştırma oranından
> türetiliyor. Zinciri şu: **canlı sağlık testleri** `3.4` varsayımını
> ayakta tutar → bütçe `8.00` iddiasını üretir. Testler düşerse çıkış kesilir
> ve iddia da düşer. Cihaz ekranı (`src/ui.rs`) tam olarak bu mantığı
> uyguluyor: sağlık `OK` değilse `8.00` **yazılmaz**.

`DRBG` modunda bu iddia geçerli **değildir**: çıktı deterministik bir
genişletmedir, bilgi-kuramsal olarak tam entropi taşımaz. Ekran o modda
`256b CSPRNG` yazar — kriptografik güç iddiası, entropi yoğunluğu iddiası
değil.

Çıktı baytı iki kanalın nibble'ını taşıyor (`(Z1<<4)|Z2`) ama bütçede bayt
başına **3.4 bit** sayılıyor, 6.8 değil. Sebep: iki kanal ortak 4.88 kHz tonu
paylaşıyor ve alt nibble'lardaki ortak bileşen henüz ölçülmedi. Muhafazakar
taraf seçildi.

---

## 7. DRBG — ne olduğu ve ne olmadığı

HMAC_DRBG (SP 800-90A, HMAC-SHA256). Her yeni conditioner bloğuyla yeniden
tohumlanıyor; ayrıca 1024 üretimde bir tohumlama **zorunlu** (aralık dolunca
üretim reddediliyor, "biraz daha idare et" yok).

**DRBG entropi ÜRETMEZ.** Çıkışı deterministik bir genişletmedir. Bu yüzden
cihaz iki modu ayrı sunuyor:

| mod | ne verir | hız |
|---|---|---|
| `RAW` | doğrudan conditioner çıkışı; her bit ölçülmüş fiziksel entropiye dayanır | yavaş |
| `DRBG` | kriptografik olarak sağlam genişletme | hızlı |

Kriptografik anahtar üretimi gibi işler için `RAW` doğru seçimdir.

---

## 8. Desteklenmeyen iddialar

Dürüst olmak için açıkça yazıyoruz:

**Mekanizma saf tünelleme değil.** Kaynağın ne olduğu belirsiz değil — kaynak
zenerlerdir. Besleme hattı osiloskopla incelendi (ferrit dışında bileşen yok),
DAC bias kontrolünün çalıştığı doğrulandı, gürültüyü üretebilecek başka aday
bulunmuyor. AR kalıntısı da doğrusal öngörülemezliği ayrıca gösteriyor.

Ama 2.4 V sınıfı zenerlerde beklenen davranış **saf** tünelleme değil: alan
kristal kusurlarında yoğunlaştığı için tünelleme oralarda lokalize oluyor ve
üstüne bir boşalma döngüsü biniyor — yani **mikroplazma + tünelleme**
(OnSemi AN HBD854). Tetikleyici olay hâlâ band-to-band tünelleme, dolayısıyla
süreç kuantum kökenlidir; değişen şey mekanizmanın tek katmanlı değil, iki
katmanlı olması (tünelleme anı + hangi kusurda olacağı).

Ek doğrulama olarak firmware bias taramasını açılışta koşup gürültü genliğinin
akımla ölçeklenmesini `atif` satırında raporluyor (`report_scaling`,
`src/main.rs`): zener gürültüsü ~√I büyür, yükselteç/besleme/ADC tabanı ise
akımdan bağımsızdır.

**Entropi sınırı kuantum argümanından türetilmedi.** Bu, yukarıdakinden ayrı
bir kısıt ve asıl önemli olan bu. Buradaki min-entropy sayısı AR(16) kalıntısı
üzerinden geliyor; yani **doğrusal** öngörülemezliği sınırlıyor, kuantum
belirsizliğini değil. Ticari QRNG'lerin bir kısmı entropi sınırını doğrudan
kuantum modelinden (ölçüm istatistiği + yan bilgi sınırı) çıkarır; burada
öyle bir türetme yok. Kaynağın kuantum kökenli olması ayrı, sınırın kuantum
argümanıyla kanıtlanması ayrı şey.

**Formal SP 800-90B sertifikası yok.** `tools/analyze.py`'daki tahminciler o
setin sadeleştirilmiş bir alt kümesi. Resmî bir sayı için NIST'in kendi
`ea_non_iid` aracı uzun bir yakalama üzerinde koşturulmalı.

**Uzun vadeli kararlılık ölçülmedi.** Ölçümler saniyeler mertebesinde;
saatler boyunca ve sıcaklık aralığında davranış bilinmiyor. Sürekli sağlık
testleri bunun için var ama istatistiği toplanmadı.

**Ortak-mod bağımlılığı ölçülmedi.** İki kanalın alt nibble'ları arasındaki
ortak bileşen ölçülmedi; bütçede muhafazakar davranıldı.

---

## Tekrar üretmek için

```bash
pip3 install numpy                      # tek bağımlılık
python3 tools/analyze.py                # önce (dinler)
cargo embed --release --features dump   # sonra
```

Analizör aliasing kontrolünü, DAC etkisini, AR kalıntısını ve min-entropy
tahminlerini bir arada raporlar; kalibrasyon satırlarını da her koşuda basar
ki sayıların hangi referansa göre okunacağı belli olsun.
