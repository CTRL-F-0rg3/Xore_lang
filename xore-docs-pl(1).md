# Xore — dokumentacja języka i kompilatora (PL)

> Xore to mały, statycznie typowany język bez pętli i bez jawnych wskaźników,
> kompilowany bezpośrednio do kodu maszynowego x86_64 i RISC-V64 (bez LLVM).
> Ten dokument opisuje **stan faktyczny** kompilatora — łącznie z tym, co
> jeszcze nie działa. Jeżeli jakaś funkcja jest niedokończona, jest to jasno
> oznaczone (⚠️ / ❌), zamiast udawać, że działa.

## Spis treści

1. [Szybki start](#szybki-start)
2. [Pliki źródłowe](#pliki-źródłowe)
3. [Komentarze](#komentarze)
4. [Typy](#typy)
5. [Literały](#literały)
6. [Zmienne (`let`)](#zmienne-let)
7. [Funkcje](#funkcje)
8. [Wywołania funkcji](#wywołania-funkcji)
9. [Wyrażenie `if`](#wyrażenie-if)
10. [Operatory](#operatory)
11. [`include` i moduły](#include-i-moduły)
12. [Tablice](#tablice)
13. [Brak I/O (świadomie, na razie)](#brak-io-świadomie-na-razie)
14. [Buildy bare metal / freestanding](#buildy-bare-metal--freestanding)
15. [Znane ograniczenia i pułapki](#znane-ograniczenia-i-pułapki)
16. [Referencja CLI](#referencja-cli)
17. [Przykład](#przykład)

---

## Szybki start

```bash
cd xore
cargo build --release
cp target/release/xore_lang_new .

./xore_lang_new main.xre --target=x86_64
./main; echo $?
```

Domyślnie kompilator **od razu oddaje gotowy plik wykonywalny** — sam
wywołuje w tle systemowy asembler/linker (`cc`/`gcc`, a dla RISC-V
`riscv64-linux-gnu-gcc`). Nic nie trzeba linkować ręcznie.

Funkcja `main` w Xore odpowiada C-owemu `main` — jej `i32` staje się kodem
wyjścia procesu, dokładnie tak jak w Go czy Zig. To jedyny obecnie sposób,
żeby "zobaczyć" wynik programu (patrz [Znane ograniczenia](#znane-ograniczenia-i-pułapki) — brak I/O).

## Pliki źródłowe

| Rozszerzenie | Rola |
|---|---|
| `.xre` | plik źródłowy — zawiera implementacje funkcji (`FnDef`) |
| `.xrh` | plik nagłówkowy — zwykle same deklaracje (`FnDecl`, bez ciała) |

Rozróżnienie `.xre`/`.xrh` jest **czysto konwencjonalne** (jak `.c`/`.h`) —
kompilator ładuje oba tak samo. Realny "linker" tego kompilatora to
rekurencyjne rozwijanie `include "...";` zaczynając od pliku podanego w
wierszu poleceń — patrz [`include` i moduły](#include-i-moduły).

## Komentarze

Tylko komentarze liniowe:

```xore
// to jest komentarz do końca linii
let x: i32 = 5; // komentarz po instrukcji
```

Nie ma komentarzy blokowych (`/* ... */`).

## Typy

| Typ | Opis | Status |
|---|---|---|
| `i32`, `i64` | liczby całkowite ze znakiem | ✅ w pełni działa |
| `u32`, `u64` | liczby całkowite bez znaku | ⚠️ działają jak `i32`/`i64` — patrz niżej |
| `f32`, `f64` | liczby zmiennoprzecinkowe | ❌ nie działają poprawnie (patrz niżej) |
| `bool` | `True` / `False` | ✅ działa |
| `Foo` (dowolna nazwa) | typ nominalny (przyszłe struktury) | ❌ nie ma definicji struktur — nieużywalne |
| `[T; N]` (tablice) | tablice o stałym rozmiarze | ❌ parsuje się, ale nie działa w runtime |

**Dlaczego `u32`/`u64` i `f32`/`f64` nie działają w pełni:** wewnętrzna
reprezentacja pośrednia (IR) nie niesie informacji o typie operacji — `a + b`
wygląda tak samo dla inta jak dla floata. Dzielenie/modulo zawsze generują
kod **ze znakiem**, więc dla `u32`/`u64` przy wartościach powyżej połowy
zakresu wynik będzie zły. Arytmetyka zmiennoprzecinkowa w ogóle nie jest
generowana poprawnie — literały `f64`/`f32` są dziś zapisywane jako surowe
bity w rejestrze całkowitym (kompilator zostawia o tym komentarz w
wygenerowanym asemblerze), więc jakiekolwiek `+`/`*` na nich da bezsensowny
wynik. **Rekomendacja: używaj tylko `i32`/`i64`/`bool` do czasu naprawienia
tego w kompilatorze** (patrz sekcja "jak rozwijać" w rozmowie z asystentem).

## Literały

```xore
42          // IntLiteral (i32 domyślnie)
0x1A        // literał szesnastkowy
0b1010      // literał binarny
0o17        // literał ósemkowy
3.14        // FloatLiteral — patrz ograniczenia wyżej
"tekst"     // StringLiteral
r"C:\path"  // RawStringLiteral (bez interpretacji \)
'a'         // CharLiteral
True        // Bool
False       // Bool
None        // "brak wartości" — typ Unknown
```

✅ **Literały szesnastkowe/binarne/ósemkowe (`0x1A`, `0b101`, `0o17`)
działają poprawnie.** Wcześniejsza wersja lexera/lowering po cichu
zamieniała je na `0` — naprawione i zweryfikowane testem porównującym wynik
z obliczeniem w Pythonie.

## Zmienne (`let`)

```xore
let x: i32 = 10;      // z jawnym typem
let y = 20;            // typ wywnioskowany
```

Zmienne są zawsze modyfikowalne wewnętrznie (nie ma `const`/`mut` — checker
zawsze traktuje `let` jako `is_mut: true`).

## Funkcje

```xore
// deklaracja (bez ciała, kończy się `;`) — zwykle w pliku .xrh
public fn add(a: i32, b: i32) -> i32;

// definicja (z ciałem) — w pliku .xre
public fn add(a: i32, b: i32) -> i32 {
    a + b;      // ostatnie wyrażenie-instrukcja = niejawna wartość zwracana
}

fn helper(x: i32) -> i32 {   // bez `public` = prywatna (parsuje się,
    x * 2;                    // ale widoczność nie jest dziś nigdzie
}                              // egzekwowana — patrz ograniczenia)
```

**Ważne:** wartość zwracana to zawsze **ostatnia instrukcja-wyrażenie** ciała
funkcji (musi kończyć się `;`, tak jak wszystko w Xore). Nie ma słowa
kluczowego `return`.

Limit parametrów: **6 na x86_64, 8 na RISC-V64** (tyle rejestrów ma
standardowa konwencja wywołań na argumenty całkowite; przekazywanie
dodatkowych argumentów przez stos nie jest jeszcze zaimplementowane —
kompilator się wtedy jawnie wywali z czytelnym błędem, a nie cicho
wygeneruje zły kod).

## Wywołania funkcji

Xore **nie** używa klasycznej składni `nazwa(argumenty)`. Zamiast tego:

```xore
let wynik: i32 = add $ (10, 20);
//                ^^^^^^^^^^^^^^ operator wywołania: `$` + nawiasy
```

`add(10, 20)` (bez `$`) **nie sparsuje się** — spróbuje potraktować `add`
jako zmienną, a `(10, 20)` jako kolejną, niepowiązaną instrukcję, i wywali
błąd parsera. To najczęstsza pomyłka dla kogoś przyzwyczajonego do C.

## Wyrażenie `if`

`if` w Xore jest **wyrażeniem**, nie instrukcją — ma wartość:

```xore
let bigger: i32 = if a > b {
    a;
} else {
    b;
};   // <-- średnik na końcu, bo cały if jest jednym wyrażeniem-instrukcją
```

Wartością gałęzi jest jej ostatnia instrukcja-wyrażenie (tak samo jak w
funkcjach).

✅ Od tej wersji `if` użyty jako wartość z brakującym/niepełnym `else` nie
czyta już śmieci ze stosu — slot wyniku jest zerowany przed uruchomieniem
gałęzi, więc "niedokończone" wyrażenie `if` bezpiecznie ewaluuje się do `0`
zamiast do niezdefiniowanej wartości. Mimo to lepiej zawsze pisać jawny
`else`, gdy używasz wartości — jest to czytelniejsze i nie polega na tym
zabezpieczeniu.

## Operatory

| Operator | Znaczenie | Status |
|---|---|---|
| `+ - * / %` | arytmetyka | ✅ w pełni działa (int) |
| `== != < > <= >=` | porównania (dają `bool`) | ✅ w pełni działa |
| `$~` | `wartość $~ zmienna;` — zapisz `wartość` do `zmiennej` (zmienna musi być l-value) | ✅ działa |
| `$ (...)` | wywołanie funkcji | ✅ działa (patrz wyżej) |
| `->` | typ zwracany funkcji | ✅ działa |
| `:= ~= |= #== <=> => @ @/ @= -=` | zarezerwowane na przyszłość | ❌ część w ogóle się nie parsuje, część parsuje się i przechodzi type-checking, ale kompilator zgłasza błąd `Unsupported operator` przy generacji IR. **Nie używaj ich w kodzie produkcyjnym.** |

## `include` i moduły

```xore
include "sciezka/do/pliku.xre";
```

- Ścieżka jest rozwiązywana **względem katalogu pliku, który zawiera
  `include`** (nie względem katalogu roboczego).
- Kompilator rekurencyjnie odwiedza wszystkie `include` zaczynając od pliku
  podanego w wierszu poleceń, zbiera wszystkie `FnDef` (definicje z ciałem) z
  odwiedzonych plików w jeden program. `FnDecl` (same deklaracje, jak w
  `.xrh`) nie generują kodu — służą tylko do tego, żeby plik się
  parsował/typował, ale **implementacja musi znaleźć się w którymś z
  faktycznie dołączonych plików `.xre`**.
- Jeśli ta sama funkcja jest zdefiniowana w dwóch dołączonych plikach,
  wygrywa ta znaleziona później (kolejność zależy od kolejności `include`).
- Nie ma cyklicznej ochrony poza deduplikacją odwiedzonych plików (plik
  dołączony dwa razy jest wczytany tylko raz).

## Tablice

Tablice o stałym rozmiarze mają teraz realną pamięć — to był kompletny
placeholder (`[1,2,3]` zawsze lowerowało się do stałej `0`); zostało
zaimplementowane naprawdę:

```xore
let arr: [i32; 5] = [10, 20, 30, 40, 50];
let i: i32 = 3;

arr[i];          // odczyt pod indeksem liczonym w runtime -> 40
99 $~ arr[i];    // zapis pod indeksem liczonym w runtime (arr[3] = 99)
arr[2];          // odczyt pod stałym indeksem (znanym w czasie kompilacji) -> 30
```

- `[T; N]` to typ tablicowy: typ elementu `T`, stały rozmiar `N` znany w
  czasie kompilacji.
- Stały indeks (`arr[2]`) kompiluje się do zwykłego, bezpośredniego dostępu
  do pamięci pod stałym adresem — korzysta z tych samych optymalizacji co
  każda inna zmienna lokalna (store→load forwarding, eliminacja martwego kodu).
- Dynamiczny indeks (`arr[i]`) liczy adres w runtime (`baza - indeks * 8`,
  bo elementy rosną "w dół" w przyjętej konwencji układu stosu) —
  zaimplementowane natywnie w obu backendach (x86_64: `lea`+`sub`+`mov`;
  RISC-V64: `li`+`sub`+`sub`+`ld`/`sd`, bo RISC-V nie ma trybu adresowania
  skalowanego).
- Zapis do elementu tablicy używa tego samego operatora `$~` co wszystko
  inne: `wartosc $~ arr[indeks];`.
- **Kopiowanie całej tablicy działa:** `let b = a;` (gdzie `a` to znana
  tablica lokalna) robi głęboką kopię wszystkich N elementów do N nowych
  slotów — sprawdzone, że to realna kopia, nie alias (modyfikacja `b` po
  skopiowaniu nie wpływa na `a`).
- **Tablice można przekazywać do funkcji.** Tablica lokalna użyta jako
  zwykła wartość (np. argument wywołania) "rozpada się" do wskaźnika na jej
  pierwszy element, dokładnie jak w C:

  ```xore
  public fn suma3(arr: [i32; 3]) -> i32 {
      arr[0] + arr[1] + arr[2];   // indeksowanie przez wskaźnik
  }
  public fn main() -> i32 {
      let a: [i32; 3] = [10, 20, 30];
      suma3 $ (a);   // -> 60
  }
  ```

  Ponieważ to przekazanie przez referencję (wskaźnik), funkcja MOŻE
  zmodyfikować tablicę wywołującego przez `$~`, a wywołujący widzi tę
  zmianę po powrocie z wywołania — sprawdzone testem, w którym funkcja
  zeruje `arr[0]`, a późniejszy odczyt u wywołującego to potwierdza.

**Obecny zakres (świadomie ograniczony, opisany zamiast po cichu zepsuty):**
- **Zwracanie tablicy z funkcji jest wprost odrzucane** na etapie
  kompilacji z czytelnym błędem, zamiast po cichu generować zły kod —
  wymagałoby to konwencji ABI typu "sret" (wywołujący rezerwuje miejsce i
  przekazuje ukryty wskaźnik), której dziś nie mamy.
- Indeksowanie działa tylko bezpośrednio na nazwanej zmiennej tablicowej
  (`arr[i]`), nie na dowolnym wyrażeniu (`f()[i]` nie zadziała).
- Brak tablic wielowymiarowych, brak sprawdzania zakresu indeksu w runtime.
- Literały tablicowe z mieszanymi typami (`[1, True]`) są teraz poprawnie
  odrzucane przez checker typów — wcześniej po cichu przechodziły (realny
  błąd naprawiony przy okazji dodawania tej funkcji).

**Subtelny błąd aliasowania znaleziony i naprawiony przy dodawaniu
parametrów funkcji:** optymalizacja store→load forwarding nie wiedziała, że
`LoadAddr` (instrukcja stojąca za rozpadem tablicy do wskaźnika) pozwala
adresowi tablicy lokalnej "wyciec" do innej funkcji. Traktowała zapisy
inicjalizujące tablicę jako "martwe" (nigdy nie odczytane LOKALNIE) i je
usuwała — więc `suma3(a)` z przykładu wyżej zwracało `0` zamiast `60`, dopóki
tego nie naprawiono. Poprawka jest konserwatywna, ale zawsze poprawna: każda
funkcja, która choć raz bierze adres tablicy lokalnej, ma ten cały przebieg
optymalizacji całkowicie wyłączony (niewielki, dobrze zlokalizowany koszt,
tylko dla funkcji przekazujących tablice przez referencję).

## Brak I/O (świadomie, na razie)

Xore nie ma `print`, dostępu do plików, ani sposobu na odczyt wejścia.
Jedyny sposób na zaobserwowanie obliczonego wyniku to zwrócone przez `main`
`i32`, które staje się kodem wyjścia procesu.

Wcześniejsza wersja tego kompilatora miała `print`/`println`/`exit`
rozpoznawane po nazwie bezpośrednio wewnątrz kompilatora
(`checker.rs`/`lowering.rs`), wołające ręcznie napisane wrappery syscalli.
**To zostało celowo usunięte**: zaszywanie na sztywno konkretnych nazw
funkcji standardowej biblioteki w rdzeniu kompilatora to zła architektura —
to ukryta zależność bez żadnego ogólnego mechanizmu za nią stojącego (brak
deklaracji `extern`/FFI, tylko specjalne dopasowanie po nazwie funkcji).
Prawdziwe I/O wróci, gdy pojawi się porządny mechanizm deklarowania funkcji
zewnętrznych/wbudowanych, nie wcześniej.

## Buildy bare metal / freestanding

Każda binarka Xore jest budowana **bez libc, bez crt0/crt1 i bez
dynamicznego linkera**. Sam kompilator emituje punkt wejścia procesu
(`_start`), który woła Twoje `main` i przekazuje jej wynik bezpośrednio do
syscalla `exit`:

```bash
./xore_lang_new main.xre --target=x86_64
ldd main        # -> "not a dynamic executable"
file main        # -> statycznie linkowany plik wykonywalny ELF
```

To "bare metal" w praktycznym, dającym się zbudować sensie **freestanding
userspace**: zero zależności od libc, same surowe syscalle, minimalna,
samowystarczalna, statyczna binarka ELF — przydatne w scenariuszach
embedded/minimalnych kontenerów albo do nauki, jak buduje się runtime od
zera. To **nie jest** cel typu bootowalny obraz jądra systemu: binarki Xore
nadal działają jako zwykłe procesy Linuksa uruchamiane przez loader ELF
jądra (`execve`), korzystające z syscalli Linuksa (`write`, `exit`) do
I/O i sterowania procesem. Wygenerowanie prawdziwego obrazu
bootsektora/jądra (zupełny brak systemu operacyjnego pod spodem,
bezpośredni dostęp do sprzętu/MMIO, własny skrypt linkera, brak syscalli)
to fundamentalnie inny cel i to nie jest to, co ten kompilator dziś robi.

## Znane ograniczenia i pułapki

- **Brak pętli.** `while`/`for` są zarezerwowanymi słowami kluczowymi w
  lekserze, ale parser ich nie obsługuje — użycie da błąd parsowania. To
  świadoma decyzja projektowa Xore, nie przeoczenie.
- **Zupełny brak I/O** (patrz [Brak I/O](#brak-io-świadomie-na-razie) wyżej)
  — jedynym obserwowalnym wynikiem jest kod wyjścia `main`.
- **Tablice mają ograniczony zakres** (patrz [Tablice](#tablice) wyżej):
  brak wielu wymiarów, brak sprawdzania zakresu w runtime, brak zwracania
  tablicy z funkcji (wprost odrzucone, nie po cichu zepsute).
- **`enum` to wciąż zaślepka.** Parsuje się i przechodzi type-checking, ale
  nie generuje żadnego kodu ani nie tworzy sposobu na odwołanie się do wariantu.
- **`public`/`fn` (widoczność) nie jest egzekwowana.** Parsuje się, ale nic
  obecnie nie sprawdza, czy prywatna funkcja jest wywoływana spoza modułu.
- **Brak arytmetyki zmiennoprzecinkowej i poprawnego unsigned** — patrz
  sekcja [Typy](#typy).

## Referencja CLI

```
xore_lang_new <plik.xre> [opcje]
```

| Flaga | Opis |
|---|---|
| `--target=x86_64` \| `--target=riscv64` | architektura docelowa (domyślnie `x86_64`) |
| `-o <plik>` | ścieżka wyjściowa (domyślnie: nazwa pliku źródłowego bez rozszerzenia dla binarki, `<nazwa>.<arch>.s` dla `--emit-asm`) |
| `--emit-asm` | zatrzymaj się na tekstowym pliku `.s`, nie linkuj binarki |
| `--keep-asm` | przy budowaniu binarki zachowaj też wygenerowany `.s` obok niej |
| `--no-opt` | wyłącz optymalizacje IR (stałe składanie, DCE, forwarding pamięci...) |
| `--dump-ir` | wypisz wygenerowane IR (po optymalizacjach) na stderr — przydatne do debugowania |
| `-h`, `--help` | pomoc |

**Uwaga o RISC-V:** kompilacja do RISC-V64 wymaga cross-toolchaina
(`riscv64-linux-gnu-gcc`) zainstalowanego na maszynie, na której uruchamiasz
`xore_lang_new` (np. `apt install gcc-riscv64-linux-gnu
binutils-riscv64-linux-gnu`). Jeśli go nie ma, kompilator jasno o tym
poinformuje i zostawi wygenerowany `.s` na dysku zamiast udawać sukces.
Uruchamianie binarek RISC-V na maszynie x86_64 wymaga emulatora, np.
`qemu-riscv64`.

## Przykład

```xore
// mathutil.xre
public fn maxi(a: i32, b: i32) -> i32 {
    if a > b {
        a;
    } else {
        b;
    };
}

// main.xre
include "mathutil.xre";

public fn main() -> i32 {
    let wartosci: [i32; 3] = [7, 19, 12];
    let najwieksza: i32 = maxi $ (maxi $ (wartosci[0], wartosci[1]), wartosci[2]);
    najwieksza;   // -> kod wyjścia procesu: 19
}
```

```bash
./xore_lang_new main.xre --target=x86_64
./main; echo $?   # 19
```
