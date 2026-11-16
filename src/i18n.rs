//! UI translations. English strings are the keys; anything without an entry
//! (technical data, driver output) is shown as is.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Lang {
    En = 0,
    Uk = 1,
    Ru = 2,
    Pl = 3,
    De = 4,
}

pub const ALL: [(Lang, &str, &str); 5] = [
    (Lang::En, "en", "English"),
    (Lang::Uk, "uk", "Українська"),
    (Lang::Ru, "ru", "Русский"),
    (Lang::Pl, "pl", "Polski"),
    (Lang::De, "de", "Deutsch"),
];

static CUR: AtomicU8 = AtomicU8::new(0);

pub fn set(l: Lang) {
    CUR.store(l as u8, Ordering::Relaxed);
}

pub fn get() -> Lang {
    ALL[CUR.load(Ordering::Relaxed) as usize % ALL.len()].0
}

pub fn code(l: Lang) -> &'static str {
    ALL.iter().find(|x| x.0 == l).map(|x| x.1).unwrap_or("en")
}

pub fn from_code(c: &str) -> Option<Lang> {
    let c = c.trim().to_lowercase();
    let c = c.get(..2)?;
    ALL.iter().find(|x| x.1 == c).map(|x| x.0)
}

/// System UI language, falling back to English.
pub fn detect() -> Lang {
    for k in ["GPUR_LANG", "LANGUAGE", "LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(v) = std::env::var(k) {
            if let Some(l) = v.split(':').find_map(from_code) {
                return l;
            }
            if !v.is_empty() && k != "GPUR_LANG" {
                break;
            }
        }
    }
    #[cfg(windows)]
    {
        unsafe extern "system" {
            fn GetUserDefaultUILanguage() -> u16;
        }
        let id = unsafe { GetUserDefaultUILanguage() } & 0x3ff;
        return match id {
            0x22 => Lang::Uk,
            0x19 => Lang::Ru,
            0x15 => Lang::Pl,
            0x07 => Lang::De,
            _ => Lang::En,
        };
    }
    #[allow(unreachable_code)]
    Lang::En
}

/// Translate a UI string.
pub fn tr(s: &str) -> &str {
    let l = get();
    if l == Lang::En {
        return s;
    }
    static MAP: OnceLock<HashMap<&'static str, &'static [&'static str; 4]>> = OnceLock::new();
    let m = MAP.get_or_init(|| T.iter().map(|(k, v)| (*k, v)).collect());
    match m.get(s) {
        Some(v) => v[l as usize - 1],
        None => s,
    }
}

/// Translate a template and fill `{}` placeholders in order.
pub fn trf(s: &str, args: &[&dyn std::fmt::Display]) -> String {
    let mut out = String::new();
    let mut it = args.iter();
    let mut parts = tr(s).split("{}").peekable();
    while let Some(p) = parts.next() {
        out.push_str(p);
        if parts.peek().is_some() {
            if let Some(a) = it.next() {
                out.push_str(&a.to_string());
            }
        }
    }
    out
}

// key: [uk, ru, pl, de]
static T: &[(&str, [&str; 4])] = &[
    // tabs and chrome
    ("Graphics Card", ["Відеокарта", "Видеокарта", "Karta graficzna", "Grafikkarte"]),
    ("Sensors", ["Датчики", "Датчики", "Czujniki", "Sensoren"]),
    ("Advanced", ["Додатково", "Дополнительно", "Zaawansowane", "Erweitert"]),
    ("Validation", ["Перевірка", "Проверка", "Weryfikacja", "Prüfung"]),
    ("Memory Test", ["Тест пам'яті", "Тест памяти", "Test pamięci", "Speichertest"]),
    ("Switch to light theme", ["Світла тема", "Светлая тема", "Jasny motyw", "Helles Design"]),
    ("Switch to dark theme", ["Темна тема", "Тёмная тема", "Ciemny motyw", "Dunkles Design"]),
    ("Rescan hardware", ["Пересканувати обладнання", "Пересканировать оборудование", "Wykryj sprzęt ponownie", "Hardware neu einlesen"]),
    ("Save a JSON report to your home folder", ["Зберегти JSON-звіт у домашню теку", "Сохранить JSON-отчёт в домашнюю папку", "Zapisz raport JSON w folderze domowym", "JSON-Bericht im Home-Ordner speichern"]),
    ("Language", ["Мова", "Язык", "Język", "Sprache"]),
    ("Close", ["Закрити", "Закрыть", "Zamknij", "Schließen"]),
    ("No graphics adapter found on the PCI bus.", ["На шині PCI не знайдено відеоадаптера.", "На шине PCI не найден видеоадаптер.", "Nie znaleziono karty graficznej na magistrali PCI.", "Keine Grafikkarte am PCI-Bus gefunden."]),
    // field labels
    ("Name", ["Назва", "Название", "Nazwa", "Name"]),
    ("Revision", ["Ревізія", "Ревизия", "Rewizja", "Revision"]),
    ("Technology", ["Техпроцес", "Техпроцесс", "Technologia", "Fertigung"]),
    ("Die Size", ["Кристал", "Кристалл", "Rdzeń", "Die-Größe"]),
    ("Release Date", ["Дата виходу", "Дата выхода", "Premiera", "Erschienen"]),
    ("Transistors", ["Транзистори", "Транзисторы", "Tranzystory", "Transistoren"]),
    ("BIOS Version", ["Версія BIOS", "Версия BIOS", "Wersja BIOS", "BIOS-Version"]),
    ("Subvendor", ["Виробник", "Производитель", "Producent", "Hersteller"]),
    ("Device ID", ["ID пристрою", "ID устройства", "ID urządz.", "Geräte-ID"]),
    ("Bus Interface", ["Шина", "Шина", "Interfejs", "Bus"]),
    ("Shaders", ["Шейдери", "Шейдеры", "Shadery", "Shader"]),
    ("Pixel Fillrate", ["Заповн. пікс.", "Заполн. пикс.", "Fillrate pikseli", "Pixelfüllrate"]),
    ("Texture Fillrate", ["Заповн. текст.", "Заполн. текст.", "Fillrate tekstur", "Texelfüllrate"]),
    ("Memory Type", ["Тип пам'яті", "Тип памяти", "Typ pamięci", "Speichertyp"]),
    ("Bus Width", ["Ширина шини", "Ширина шины", "Szyna", "Busbreite"]),
    ("Memory Size", ["Обсяг пам'яті", "Объём памяти", "Pamięć", "Speichergröße"]),
    ("Bandwidth", ["Пропускна зд.", "Пропускная сп.", "Przepustowość", "Bandbreite"]),
    ("Driver Version", ["Драйвер", "Драйвер", "Sterownik", "Treiber"]),
    ("Driver Files", ["Цілісність", "Целостность", "Integralność", "Integrität"]),
    ("GPU Clock", ["Частота GPU", "Частота GPU", "Zegar GPU", "GPU-Takt"]),
    ("Memory", ["Пам'ять", "Память", "Pamięć", "Speicher"]),
    ("Reference", ["Еталон", "Эталон", "Wzorzec", "Referenz"]),
    ("Max Memory", ["Макс. пам'ять", "Макс. память", "Maks. pamięć", "Max. Speicher"]),
    ("Computing", ["Обчислення", "Вычисления", "Obliczenia", "Berechnung"]),
    ("Technologies", ["Технології", "Технологии", "Technologie", "Technologien"]),
    ("Ray Tracing", ["Трасування", "Трассировка", "Ray tracing", "Raytracing"]),
    ("Root access", ["Права root", "Права root", "Dostęp root", "Root-Rechte"]),
    ("Admin rights", ["Права адміна", "Права админа", "Prawa admina", "Admin-Rechte"]),
    // values
    ("N/A", ["Н/Д", "Н/Д", "B/D", "k. A."]),
    ("Unknown", ["Невідомо", "Неизвестно", "Nieznane", "Unbekannt"]),
    ("Distribution package, unmodified", ["Пакет дистрибутива, без змін", "Пакет дистрибутива, без изменений", "Pakiet dystrybucji, niezmieniony", "Distributionspaket, unverändert"]),
    ("MODIFIED / injected", ["ЗМІНЕНО / впроваджено", "ИЗМЕНЕНО / внедрено", "ZMODYFIKOWANY / wstrzyknięty", "VERÄNDERT / injiziert"]),
    ("loaded module differs from disk", ["завантажений модуль відрізняється від диска", "загруженный модуль отличается от диска", "załadowany moduł różni się od dysku", "geladenes Modul weicht vom Datenträger ab"]),
    ("unverified", ["не перевірено", "не проверено", "niezweryfikowany", "ungeprüft"]),
    ("Enabled (full VRAM aperture)", ["Увімкнено (уся VRAM)", "Включено (вся VRAM)", "Włączony (cała VRAM)", "Aktiv (voller VRAM)"]),
    // legend + evidence tooltip
    ("cross-checked", ["перевірено", "перепроверено", "zweryfikowane", "geprüft"]),
    ("differs from reference", ["відрізняється від еталону", "отличается от эталона", "różni się od wzorca", "weicht von Referenz ab"]),
    ("sources contradict", ["джерела суперечать", "источники противоречат", "źródła sprzeczne", "Quellen widersprechen"]),
    ("hover a value for its sources", ["наведіть на значення, щоб побачити джерела", "наведите на значение, чтобы увидеть источники", "najedź na wartość, by zobaczyć źródła", "Maus über einem Wert zeigt die Quellen"]),
    ("Confirmed by independent sources:", ["Підтверджено незалежними джерелами:", "Подтверждено независимыми источниками:", "Potwierdzone przez niezależne źródła:", "Von unabhängigen Quellen bestätigt:"]),
    ("Single source, nothing to compare against:", ["Одне джерело, немає з чим порівняти:", "Один источник, не с чем сравнить:", "Jedno źródło, brak porównania:", "Einzelne Quelle, kein Vergleich möglich:"]),
    ("Differs from the factory reference:", ["Відрізняється від заводського еталону:", "Отличается от заводского эталона:", "Różni się od fabrycznego wzorca:", "Weicht von der Werksreferenz ab:"]),
    ("Sources contradict each other:", ["Джерела суперечать одне одному:", "Источники противоречат друг другу:", "Źródła są ze sobą sprzeczne:", "Quellen widersprechen sich:"]),
    ("Not available.", ["Недоступно.", "Недоступно.", "Niedostępne.", "Nicht verfügbar."]),
    ("{} of {} agree", ["{} з {} збігаються", "{} из {} совпадают", "{} z {} zgodnych", "{} von {} stimmen überein"]),
    // verdicts
    ("Contradictions", ["Суперечності", "Противоречия", "Sprzeczności", "Widersprüche"]),
    ("Modified", ["Модифікована", "Модифицирована", "Zmodyfikowana", "Modifiziert"]),
    ("Consistent", ["Узгоджено", "Согласовано", "Spójna", "Konsistent"]),
    ("Genuine", ["Оригінал", "Оригинал", "Oryginalna", "Original"]),
    ("Two or more sources report different values for the same thing. Treat this card as fake or faulty until explained.",
     ["Два або більше джерел повідомляють різні значення одного параметра. Вважайте карту підробленою або несправною, доки це не пояснено.",
      "Два или более источника сообщают разные значения одного параметра. Считайте карту поддельной или неисправной, пока это не объяснено.",
      "Dwa lub więcej źródeł podaje różne wartości tego samego parametru. Traktuj kartę jako podróbkę lub uszkodzoną, dopóki nie zostanie to wyjaśnione.",
      "Zwei oder mehr Quellen melden unterschiedliche Werte für dieselbe Größe. Betrachten Sie die Karte als gefälscht oder defekt, bis das geklärt ist."]),
    ("Everything is self-consistent, but some values differ from the factory spec: modded BIOS, unlocked units, memory mod or a non-reference model.",
     ["Дані узгоджені між собою, але деякі значення відрізняються від заводських: модифікований BIOS, розблоковані блоки, мод пам'яті або нереференсна модель.",
      "Данные согласованы между собой, но некоторые значения отличаются от заводских: модифицированный BIOS, разблокированные блоки, мод памяти или нереференсная модель.",
      "Dane są spójne, ale niektóre wartości różnią się od fabrycznych: zmodyfikowany BIOS, odblokowane jednostki, mod pamięci lub model niereferencyjny.",
      "Alles ist in sich stimmig, aber einige Werte weichen von der Werksspezifikation ab: modifiziertes BIOS, freigeschaltete Einheiten, Speichermod oder ein Nicht-Referenzmodell."]),
    ("Every source agrees with the others and with the factory spec. Run the memory test and FP32 benchmark to verify the hardware itself.",
     ["Усі джерела узгоджуються між собою та із заводськими характеристиками. Запустіть тест пам'яті й FP32-бенчмарк, щоб перевірити саме залізо.",
      "Все источники согласуются между собой и с заводскими характеристиками. Запустите тест памяти и FP32-бенчмарк, чтобы проверить само железо.",
      "Wszystkie źródła są zgodne ze sobą i ze specyfikacją fabryczną. Uruchom test pamięci i benchmark FP32, aby sprawdzić sam sprzęt.",
      "Alle Quellen stimmen untereinander und mit der Werksspezifikation überein. Starten Sie Speichertest und FP32-Benchmark, um die Hardware selbst zu prüfen."]),
    ("passed", ["пройдено", "пройдено", "zaliczone", "bestanden"]),
    ("differ", ["відрізняються", "отличаются", "różnice", "abweichend"]),
    ("contradict", ["суперечать", "противоречат", "sprzeczne", "widersprüchlich"]),
    ("not run", ["не запущено", "не запущено", "nie uruchomiono", "nicht ausgeführt"]),
    ("Run as root to also read the physical BIOS chip and the full PCI config space.",
     ["Запустіть від root, щоб також прочитати мікросхему BIOS і весь конфігураційний простір PCI.",
      "Запустите от root, чтобы также прочитать микросхему BIOS и всё конфигурационное пространство PCI.",
      "Uruchom jako root, aby odczytać też układ BIOS i pełną przestrzeń konfiguracyjną PCI.",
      "Als root starten, um auch den BIOS-Chip und den gesamten PCI-Konfigurationsraum zu lesen."]),
    ("On Windows the BIOS chip and driver files cannot be read; identity, VRAM and the GPU tests still work.",
     ["У Windows мікросхему BIOS і файли драйвера прочитати не можна; ідентифікація, VRAM і тести GPU працюють.",
      "В Windows микросхему BIOS и файлы драйвера прочитать нельзя; идентификация, VRAM и тесты GPU работают.",
      "W systemie Windows nie da się odczytać układu BIOS ani plików sterownika; identyfikacja, VRAM i testy GPU działają.",
      "Unter Windows sind BIOS-Chip und Treiberdateien nicht lesbar; Identität, VRAM und GPU-Tests funktionieren."]),
    // check groups
    ("Identity", ["Ідентифікація", "Идентификация", "Identyfikacja", "Identität"]),
    ("Video BIOS", ["Відео-BIOS", "Видео-BIOS", "BIOS karty", "Video-BIOS"]),
    ("Shader array", ["Шейдерний масив", "Шейдерный массив", "Jednostki cieniujące", "Shader-Array"]),
    ("Driver", ["Драйвер", "Драйвер", "Sterownik", "Treiber"]),
    ("Measured", ["Виміряно", "Измерено", "Pomiary", "Gemessen"]),
    // check titles
    ("PCIe link width", ["Ширина лінка PCIe", "Ширина линка PCIe", "Szerokość łącza PCIe", "PCIe-Linkbreite"]),
    ("Active compute units vs reference", ["Активні обчислювальні блоки та еталон", "Активные вычислительные блоки и эталон", "Aktywne jednostki obliczeniowe a wzorzec", "Aktive Recheneinheiten vs. Referenz"]),
    ("CU count self-consistency", ["Узгодженість кількості CU", "Согласованность количества CU", "Spójność liczby CU", "Konsistenz der CU-Anzahl"]),
    ("CUDA cores vs reference", ["Ядра CUDA та еталон", "Ядра CUDA и эталон", "Rdzenie CUDA a wzorzec", "CUDA-Kerne vs. Referenz"]),
    ("Memory bus width vs reference", ["Ширина шини пам'яті та еталон", "Ширина шины памяти и эталон", "Szerokość szyny pamięci a wzorzec", "Speicherbusbreite vs. Referenz"]),
    ("Memory type vs reference", ["Тип пам'яті та еталон", "Тип памяти и эталон", "Typ pamięci a wzorzec", "Speichertyp vs. Referenz"]),
    ("ROPs vs reference", ["ROP та еталон", "ROP и эталон", "ROP a wzorzec", "ROPs vs. Referenz"]),
    ("GPU driver kernel parameters", ["Параметри ядра для драйвера GPU", "Параметры ядра для драйвера GPU", "Parametry jądra sterownika GPU", "Kernel-Parameter des GPU-Treibers"]),
    ("Kernel taint", ["Позначка taint ядра", "Метка taint ядра", "Flaga taint jądra", "Kernel-Taint"]),
    ("Library injection (LD_PRELOAD / ld.so.preload)", ["Впровадження бібліотек (LD_PRELOAD / ld.so.preload)", "Внедрение библиотек (LD_PRELOAD / ld.so.preload)", "Wstrzykiwanie bibliotek (LD_PRELOAD / ld.so.preload)", "Bibliotheks-Injektion (LD_PRELOAD / ld.so.preload)"]),
    ("Loaded module == module on disk", ["Завантажений модуль == модуль на диску", "Загруженный модуль == модуль на диске", "Załadowany moduł == moduł na dysku", "Geladenes Modul == Modul auf Datenträger"]),
    ("Module signature", ["Підпис модуля", "Подпись модуля", "Podpis modułu", "Modulsignatur"]),
    ("Module taint", ["Позначка taint модуля", "Метка taint модуля", "Flaga taint modułu", "Modul-Taint"]),
    ("CUDA compute capability (architecture)", ["CUDA compute capability (архітектура)", "CUDA compute capability (архитектура)", "CUDA compute capability (architektura)", "CUDA Compute Capability (Architektur)"]),
    ("Device ID consistent across sources", ["ID пристрою однаковий в усіх джерелах", "ID устройства одинаков во всех источниках", "ID urządzenia zgodne we wszystkich źródłach", "Geräte-ID in allen Quellen gleich"]),
    ("Graphics core IP version (silicon)", ["Версія IP графічного ядра (кристал)", "Версия IP графического ядра (кристалл)", "Wersja IP rdzenia graficznego (krzem)", "IP-Version des Grafikkerns (Silizium)"]),
    ("Reference data consistency", ["Узгодженість еталонних даних", "Согласованность эталонных данных", "Spójność danych wzorcowych", "Konsistenz der Referenzdaten"]),
    ("Reference SKU", ["Еталонна модель", "Эталонная модель", "Model wzorcowy", "Referenzmodell"]),
    ("FP32 throughput vs claimed shaders", ["Продуктивність FP32 та заявлені шейдери", "Производительность FP32 и заявленные шейдеры", "Wydajność FP32 a deklarowane shadery", "FP32-Durchsatz vs. angegebene Shader"]),
    ("Physical VRAM test", ["Фізичний тест VRAM", "Физический тест VRAM", "Fizyczny test VRAM", "Physischer VRAM-Test"]),
    ("Usable VRAM vs total (kernel reservation)", ["Доступна VRAM та загальна (резерв ядра)", "Доступная VRAM и общая (резерв ядра)", "Dostępna VRAM a całkowita (rezerwa jądra)", "Nutzbarer VRAM vs. gesamt (Kernel-Reservierung)"]),
    ("VRAM size agreement between driver interfaces", ["Обсяг VRAM однаковий в інтерфейсах драйвера", "Объём VRAM одинаков в интерфейсах драйвера", "Zgodność rozmiaru VRAM w interfejsach sterownika", "VRAM-Größe über Treiberschnittstellen gleich"]),
    ("VRAM size plausible for bus width", ["Обсяг VRAM відповідає ширині шини", "Объём VRAM соответствует ширине шины", "Rozmiar VRAM pasuje do szerokości szyny", "VRAM-Größe passt zur Busbreite"]),
    ("VRAM size vs factory configurations", ["Обсяг VRAM та заводські конфігурації", "Объём VRAM и заводские конфигурации", "Rozmiar VRAM a konfiguracje fabryczne", "VRAM-Größe vs. Werkskonfigurationen"]),
    ("VRAM size vs PCI BAR aperture", ["Обсяг VRAM та апертура PCI BAR", "Объём VRAM и апертура PCI BAR", "Rozmiar VRAM a okno PCI BAR", "VRAM-Größe vs. PCI-BAR-Fenster"]),
    ("Board (subsystem) ID in ATOM header", ["ID плати (subsystem) у заголовку ATOM", "ID платы (subsystem) в заголовке ATOM", "ID karty (subsystem) w nagłówku ATOM", "Platinen-ID (Subsystem) im ATOM-Header"]),
    ("Device ID embedded in VBIOS (PCIR)", ["ID пристрою, записаний у VBIOS (PCIR)", "ID устройства, записанный в VBIOS (PCIR)", "ID urządzenia zapisane w VBIOS (PCIR)", "Im VBIOS eingetragene Geräte-ID (PCIR)"]),
    ("Driver VBIOS == physical ROM", ["VBIOS драйвера == фізичний ROM", "VBIOS драйвера == физический ROM", "VBIOS sterownika == fizyczny ROM", "Treiber-VBIOS == physisches ROM"]),
    ("Legacy image checksum", ["Контрольна сума legacy-образу", "Контрольная сумма legacy-образа", "Suma kontrolna obrazu legacy", "Prüfsumme des Legacy-Images"]),
    ("Part number (image vs driver)", ["Номер деталі (образ і драйвер)", "Номер детали (образ и драйвер)", "Numer części (obraz a sterownik)", "Teilenummer (Image vs. Treiber)"]),
    ("PowerPlay table in use == VBIOS table", ["Активна таблиця PowerPlay == таблиця VBIOS", "Активная таблица PowerPlay == таблица VBIOS", "Używana tabela PowerPlay == tabela VBIOS", "Aktive PowerPlay-Tabelle == VBIOS-Tabelle"]),
    ("VBIOS image", ["Образ VBIOS", "Образ VBIOS", "Obraz VBIOS", "VBIOS-Image"]),
    ("VBIOS version (ROM vs NVML)", ["Версія VBIOS (ROM і NVML)", "Версия VBIOS (ROM и NVML)", "Wersja VBIOS (ROM a NVML)", "VBIOS-Version (ROM vs. NVML)"]),
    // sensors
    ("Memory Clock", ["Частота пам'яті", "Частота памяти", "Zegar pamięci", "Speichertakt"]),
    ("GPU Temperature", ["Температура GPU", "Температура GPU", "Temperatura GPU", "GPU-Temperatur"]),
    ("Hot Spot", ["Гаряча точка", "Горячая точка", "Hot spot", "Hotspot"]),
    ("Memory Temperature", ["Температура пам'яті", "Температура памяти", "Temperatura pamięci", "Speichertemperatur"]),
    ("Fan Speed", ["Оберти вентилятора", "Обороты вентилятора", "Obroty wentylatora", "Lüfterdrehzahl"]),
    ("Fan Speed (%)", ["Вентилятор (%)", "Вентилятор (%)", "Wentylator (%)", "Lüfter (%)"]),
    ("Board Power", ["Споживання плати", "Потребление платы", "Pobór mocy karty", "Kartenleistung"]),
    ("Power Limit", ["Ліміт потужності", "Лимит мощности", "Limit mocy", "Leistungslimit"]),
    ("GPU Voltage", ["Напруга GPU", "Напряжение GPU", "Napięcie GPU", "GPU-Spannung"]),
    ("GPU Load", ["Завантаження GPU", "Загрузка GPU", "Obciążenie GPU", "GPU-Last"]),
    ("Memory Controller Load", ["Завантаж. контролера пам'яті", "Загрузка контроллера памяти", "Obciążenie kontrolera pamięci", "Speichercontroller-Last"]),
    ("Memory Used", ["Зайнято пам'яті", "Занято памяти", "Użyta pamięć", "Belegter Speicher"]),
    ("GTT Used", ["Зайнято GTT", "Занято GTT", "Użyte GTT", "Belegter GTT"]),
    ("Temperature", ["Температура", "Температура", "Temperatura", "Temperatur"]),
    ("Clock", ["Частота", "Частота", "Zegar", "Takt"]),
    ("Power", ["Потужність", "Мощность", "Moc", "Leistung"]),
    ("Load", ["Завантаження", "Загрузка", "Obciążenie", "Last"]),
    ("Fan", ["Вентилятор", "Вентилятор", "Wentylator", "Lüfter"]),
    ("Voltage", ["Напруга", "Напряжение", "Napięcie", "Spannung"]),
    ("Refresh", ["Оновлення", "Обновление", "Odświeżanie", "Intervall"]),
    ("Log to file", ["Запис у файл", "Запись в файл", "Zapis do pliku", "In Datei schreiben"]),
    ("Reset", ["Скинути", "Сбросить", "Resetuj", "Zurücksetzen"]),
    ("Reset min / max / average", ["Скинути мін / макс / середнє", "Сбросить мин / макс / среднее", "Resetuj min / maks / średnią", "Min / Max / Mittel zurücksetzen"]),
    ("Click a sensor to plot its group above. Hover for min, max and average.",
     ["Клацніть датчик, щоб показати його групу на графіку. Наведіть, щоб побачити мін, макс і середнє.",
      "Щёлкните датчик, чтобы показать его группу на графике. Наведите, чтобы увидеть мин, макс и среднее.",
      "Kliknij czujnik, aby pokazać jego grupę na wykresie. Najedź, aby zobaczyć min, maks i średnią.",
      "Sensor anklicken, um seine Gruppe oben zu zeichnen. Maus darüber zeigt Min, Max und Mittel."]),
    ("now {} {}\nmin {}, max {}, avg {}", ["зараз {} {}\nмін {}, макс {}, сер {}", "сейчас {} {}\nмин {}, макс {}, ср {}", "teraz {} {}\nmin {}, maks {}, śr {}", "jetzt {} {}\nMin {}, Max {}, Mittel {}"]),
    ("Click to plot this group", ["Клацніть, щоб показати цю групу", "Щёлкните, чтобы показать эту группу", "Kliknij, aby pokazać tę grupę", "Klicken, um diese Gruppe zu zeichnen"]),
    // memory test
    ("Fills free VRAM with patterns unique to each block, then reads it back. Missing chips show up as blocks that overwrite each other, failing chips as flipped bits. Close games and 3D apps first.",
     ["Заповнює вільну VRAM унікальними для кожного блоку шаблонами й читає їх назад. Відсутні чипи проявляються як блоки, що перезаписують один одного, несправні — як змінені біти. Спершу закрийте ігри та 3D-програми.",
      "Заполняет свободную VRAM уникальными для каждого блока шаблонами и читает их обратно. Отсутствующие чипы проявляются как блоки, перезаписывающие друг друга, неисправные — как изменённые биты. Сначала закройте игры и 3D-приложения.",
      "Wypełnia wolną VRAM wzorcami unikalnymi dla każdego bloku i odczytuje je z powrotem. Brakujące kości objawiają się jako bloki nadpisujące się nawzajem, uszkodzone jako przekłamane bity. Najpierw zamknij gry i aplikacje 3D.",
      "Füllt freien VRAM mit blockweise eindeutigen Mustern und liest sie zurück. Fehlende Chips zeigen sich als Blöcke, die sich gegenseitig überschreiben, defekte als gekippte Bits. Vorher Spiele und 3D-Programme schließen."]),
    ("Mode", ["Режим", "Режим", "Tryb", "Modus"]),
    ("Single run", ["Одноразовий", "Однократный", "Pojedynczy", "Einmalig"]),
    ("Stress test", ["Стрес-тест", "Стресс-тест", "Test obciążeniowy", "Stresstest"]),
    ("Repeats all patterns until you press Stop, an error appears or the GPU hangs. Each pass is written to a log file in your home folder, so the result survives a system freeze.",
     ["Повторює всі шаблони, доки ви не натиснете Стоп, не з'явиться помилка або GPU не зависне. Кожен прохід записується в журнал у домашній теці, тож результат збережеться навіть після зависання системи.",
      "Повторяет все шаблоны, пока вы не нажмёте Стоп, не появится ошибка или GPU не зависнет. Каждый проход записывается в журнал в домашней папке, поэтому результат сохранится даже после зависания системы.",
      "Powtarza wszystkie wzorce, aż naciśniesz Zatrzymaj, pojawi się błąd lub GPU się zawiesi. Każdy przebieg trafia do dziennika w folderze domowym, więc wynik przetrwa zawieszenie systemu.",
      "Wiederholt alle Muster, bis Sie Stopp drücken, ein Fehler auftritt oder die GPU hängt. Jeder Durchlauf wird in eine Logdatei im Home-Ordner geschrieben, das Ergebnis übersteht also auch einen Systemhänger."]),
    ("Load shaders too (heats the core)", ["Також навантажити шейдери (гріє ядро)", "Также нагрузить шейдеры (греет ядро)", "Obciąż też shadery (grzeje rdzeń)", "Auch Shader belasten (heizt den Kern)"]),
    ("Keep going after errors", ["Не зупинятися на помилках", "Не останавливаться на ошибках", "Kontynuuj po błędach", "Bei Fehlern weitermachen"]),
    ("Start stress test", ["Почати стрес-тест", "Начать стресс-тест", "Uruchom test obciążeniowy", "Stresstest starten"]),
    ("Time", ["Час", "Время", "Czas", "Zeit"]),
    ("Pass", ["Прохід", "Проход", "Przebieg", "Durchlauf"]),
    ("GPU lost by the driver (hang or reset): {}", ["Драйвер втратив GPU (зависання або скидання): {}", "Драйвер потерял GPU (зависание или сброс): {}", "Sterownik utracił GPU (zawieszenie lub reset): {}", "Treiber hat die GPU verloren (Hänger oder Reset): {}"]),
    ("GPU has not responded for {} s: it may have hung", ["GPU не відповідає вже {} с, можливо, завис", "GPU не отвечает уже {} с, возможно, завис", "GPU nie odpowiada od {} s, mogło się zawiesić", "GPU antwortet seit {} s nicht, möglicherweise hängt sie"]),
    ("Log: {}", ["Журнал: {}", "Журнал: {}", "Dziennik: {}", "Protokoll: {}"]),
    ("Free VRAM", ["Вільна VRAM", "Свободная VRAM", "Wolna VRAM", "Freier VRAM"]),
    ("Passes", ["Проходи", "Проходы", "Przebiegi", "Durchläufe"]),
    ("Patterns", ["Шаблони", "Шаблоны", "Wzorce", "Muster"]),
    ("address", ["адреса", "адрес", "adres", "Adresse"]),
    ("inverted", ["інверсія", "инверсия", "odwrócony", "invertiert"]),
    ("random", ["випадковий", "случайный", "losowy", "zufällig"]),
    ("walking 1s", ["біжуча 1", "бегущая 1", "krocząca 1", "Lauf-1"]),
    ("checker", ["шахівниця", "шахматка", "szachownica", "Schachbrett"]),
    ("solid", ["суцільний", "сплошной", "stały", "konstant"]),
    ("Stop", ["Стоп", "Стоп", "Zatrzymaj", "Stopp"]),
    ("Start memory test", ["Почати тест пам'яті", "Начать тест памяти", "Uruchom test pamięci", "Speichertest starten"]),
    ("FP32 benchmark", ["FP32-бенчмарк", "FP32-бенчмарк", "Benchmark FP32", "FP32-Benchmark"]),
    ("Measures real shader throughput to confirm the shader count", ["Вимірює реальну продуктивність шейдерів, щоб підтвердити їх кількість", "Измеряет реальную производительность шейдеров, чтобы подтвердить их количество", "Mierzy rzeczywistą wydajność shaderów, aby potwierdzić ich liczbę", "Misst den realen Shader-Durchsatz, um die Shader-Anzahl zu bestätigen"]),
    ("Last run: {} errors", ["Останній запуск: {} помилок", "Последний запуск: {} ошибок", "Ostatni test: {} błędów", "Letzter Lauf: {} Fehler"]),
    ("Last run: no errors", ["Останній запуск: без помилок", "Последний запуск: без ошибок", "Ostatni test: bez błędów", "Letzter Lauf: keine Fehler"]),
    ("Not run yet", ["Ще не запускався", "Ещё не запускался", "Jeszcze nie uruchomiono", "Noch nicht ausgeführt"]),
    ("VRAM map, {} MB per block of {}", ["Карта VRAM, {} МБ на блок, усього {}", "Карта VRAM, {} МБ на блок, всего {}", "Mapa VRAM, {} MB na blok, razem {}", "VRAM-Karte, {} MB pro Block von {}"]),
    ("untested", ["не перевірено", "не проверено", "nietestowane", "ungetestet"]),
    ("allocated", ["виділено", "выделено", "przydzielone", "belegt"]),
    ("written", ["записано", "записано", "zapisane", "geschrieben"]),
    ("verified", ["перевірено", "проверено", "zweryfikowane", "verifiziert"]),
    ("errors", ["помилки", "ошибки", "błędy", "Fehler"]),
    ("Memory test", ["Тест пам'яті", "Тест памяти", "Test pamięci", "Speichertest"]),
    ("Failing addresses", ["Збійні адреси", "Сбойные адреса", "Błędne adresy", "Fehlerhafte Adressen"]),
    ("Test log", ["Журнал тесту", "Журнал теста", "Dziennik testu", "Testprotokoll"]),
    // advanced
    ("Compute units", ["Обчислювальні блоки", "Вычислительные блоки", "Jednostki obliczeniowe", "Recheneinheiten"]),
    ("{} shader engines, {} arrays each, {} CU slots per array. {} of {} CUs active, read from the hardware harvest registers.",
     ["{} шейдерних рушіїв, по {} масиви, {} слотів CU на масив. Активно {} з {} CU (з апаратних регістрів harvest).",
      "{} шейдерных движков, по {} массива, {} слотов CU на массив. Активно {} из {} CU (из аппаратных регистров harvest).",
      "{} silników shaderów, po {} macierze, {} gniazd CU na macierz. Aktywne {} z {} CU (z rejestrów sprzętowych harvest).",
      "{} Shader-Engines mit je {} Arrays, {} CU-Plätze pro Array. {} von {} CUs aktiv (aus den Hardware-Harvest-Registern)."]),
    ("active", ["активний", "активен", "aktywna", "aktiv"]),
    ("disabled (harvested or fused off)", ["вимкнений (відбраковано або перепалено)", "отключён (отбракован или пережжён)", "wyłączona (odrzucona lub przepalona)", "deaktiviert (teildeaktiviert oder durchgebrannt)"]),
    ("amdgpu driver", ["Драйвер amdgpu", "Драйвер amdgpu", "Sterownik amdgpu", "amdgpu-Treiber"]),
    ("Firmware", ["Мікропрограми", "Микропрограммы", "Firmware", "Firmware"]),
    ("VBIOS (driver copy)", ["VBIOS (копія драйвера)", "VBIOS (копия драйвера)", "VBIOS (kopia sterownika)", "VBIOS (Treiberkopie)"]),
    ("VBIOS (physical ROM)", ["VBIOS (фізичний ROM)", "VBIOS (физический ROM)", "VBIOS (fizyczny ROM)", "VBIOS (physisches ROM)"]),
    ("Driver files", ["Файли драйвера", "Файлы драйвера", "Pliki sterownika", "Treiberdateien"]),
    ("Reference spec", ["Еталонні характеристики", "Эталонные характеристики", "Specyfikacja wzorcowa", "Referenzspezifikation"]),
    ("Size", ["Розмір", "Размер", "Rozmiar", "Größe"]),
    ("Source", ["Джерело", "Источник", "Źródło", "Quelle"]),
    ("Part number", ["Номер деталі", "Номер детали", "Numer części", "Teilenummer"]),
    ("Version", ["Версія", "Версия", "Wersja", "Version"]),
    ("Address", ["Адреса", "Адрес", "Adres", "Adresse"]),
    ("Vendor", ["Виробник", "Производитель", "Producent", "Hersteller"]),
    ("Device", ["Пристрій", "Устройство", "Urządzenie", "Gerät"]),
    ("Subsystem", ["Підсистема", "Подсистема", "Podsystem", "Subsystem"]),
    ("Model", ["Модель", "Модель", "Model", "Modell"]),
    ("Matched by", ["Визначено за", "Определено по", "Dopasowano wg", "Erkannt über"]),
    ("Units", ["Блоки", "Блоки", "Jednostki", "Einheiten"]),
    ("Boost clock", ["Boost-частота", "Boost-частота", "Zegar boost", "Boost-Takt"]),
    ("Signed by", ["Підписано", "Подписано", "Podpisane przez", "Signiert von"]),
    ("Module", ["Модуль", "Модуль", "Moduł", "Modul"]),
    ("Bus width", ["Ширина шини", "Ширина шины", "Szerokość szyny", "Busbreite"]),
    // status bar
    ("Hardware rescanned", ["Обладнання перескановано", "Оборудование пересканировано", "Sprzęt wykryty ponownie", "Hardware neu eingelesen"]),
    ("Memory test finished", ["Тест пам'яті завершено", "Тест памяти завершён", "Test pamięci zakończony", "Speichertest beendet"]),
    ("Memory test found errors", ["Тест пам'яті знайшов помилки", "Тест памяти нашёл ошибки", "Test pamięci wykrył błędy", "Speichertest hat Fehler gefunden"]),
    ("FP32 benchmark finished", ["FP32-бенчмарк завершено", "FP32-бенчмарк завершён", "Benchmark FP32 zakończony", "FP32-Benchmark beendet"]),
    ("Test could not start: {}", ["Тест не запустився: {}", "Тест не запустился: {}", "Nie udało się uruchomić testu: {}", "Test konnte nicht starten: {}"]),
    ("Report saved to {}", ["Звіт збережено: {}", "Отчёт сохранён: {}", "Raport zapisany: {}", "Bericht gespeichert: {}"]),
    ("Report not saved: {}", ["Звіт не збережено: {}", "Отчёт не сохранён: {}", "Raport nie został zapisany: {}", "Bericht nicht gespeichert: {}"]),
    ("Logging sensors to {}", ["Запис датчиків у {}", "Запись датчиков в {}", "Zapis czujników do {}", "Sensoren werden protokolliert in {}"]),
    ("Sensor logging stopped", ["Запис датчиків зупинено", "Запись датчиков остановлена", "Zapis czujników zatrzymany", "Sensorprotokoll beendet"]),
];
