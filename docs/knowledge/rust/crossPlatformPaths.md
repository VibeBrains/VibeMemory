# Кросс-платформенные пути и гейты в Rust-ядре (2026-09-02)

Грабли, собранные при написании `vibememory-core::naming` на typed-path 0.12.3, globset 0.4.20,
serde 1.0.229 и clippy 1.97.1. Проверено scratch-сборками (частью — верификатором в той же сессии,
помечено).

## typed-path

- `std::path::Path` разбирает по синтаксису платформы компиляции: на Mac `D:\a\b` — один
  компонент. Ядро получает пути с другой машины строками, поэтому синтаксис задаётся явно
  (`Utf8TypedPath::unix` / `::windows`), а `derive` — эвристика только для чужих строк
  (`gitdir:`, шаблоны): `//server/share/x` она относит к Windows.
- `\\server\share` без хвостового разделителя: `is_absolute() == false`, `file_name() == None`,
  компоненты — только `Prefix(UNC)`. Якорность нужно считать по префиксу (любой, кроме голого
  `Disk`), а корень — по отсутствию обычных компонентов; `D:foo` — drive-relative, не якорный.
- `normalize()` у Windows-путей оставляет префикс как есть (`//server/share\x` — смешанные
  разделители), а у относительных путей съедает ведущие `..`. Поэтому относительный `gitdir:`
  сперва join'ится к каталогу файла, и только потом разрешаются `..`; сравнения — по компонентам,
  не по строке (верификатор + собственный прогон фикстур).
- Компоненты путей нормализуются в NFC при разборе: git печатает имена в форме на диске (NFD
  на APFS), CLI — в NFC ([git/repoLayoutEdges.md](../git/repoLayoutEdges.md)); имена сторов,
  выведенные из путей, поэтому всегда NFC.
- Свёртка регистра — посимвольно (`chars().flat_map(char::to_lowercase)`): `str::to_lowercase`
  применяет контекстное правило финальной сигмы (`ΔΟΣ` → `δος`, `δοσ` → `δοσ`), файловые
  системы — нет.
- Константы Windows: `typed_path::constants::windows::{RESERVED_DEVICE_NAMES_STR, DISALLOWED_FILENAME_CHARS}`
  (модуль `typed_path::windows` приватный). Список включает `COM0`/`LPT0`, но не надстрочные
  `COM¹`…`LPT³` из документации Microsoft — они добавлены у нас (на Windows не проверены).

## globset

- Шаблоны канонизируются тем же разбором, что и cwd (NFC, `.`/`..`, хвостовой `/`), и glob
  компилируется из канонической формы — иначе `/x/proj/` из tab-completion и `//server/share`
  без хвоста молча никогда не совпадут.
- `backslash_escape` по умолчанию зависит от платформы хоста («only enabled on Unix platforms by
  default») — задавать явно; при `false` `\` — литерал, который никогда не совпадёт с путём,
  отрендеренным через `/`, поэтому шаблоны с `\` отвергаются при загрузке.
- `literal_separator(true)`: `*` не пересекает `/`. `foo/**` не матчит `foo`, но матчит `foo/` —
  cwd рендерится без хвостового разделителя. `case_insensitive` — на каждый glob отдельно, в
  одном `GlobSet`. Пустой шаблон компилируется в `^$` (верификатор) — отвергаем сами.
- `GlobSet::matches` возвращает индексы по возрастанию — «первый совпавший в порядке файла»
  получается даром.

## serde_json на строках JSONL

- `Cow<str>` при `from_slice` не заимствует (всегда `Owned`), поэтому голову строки читаем в
  `Option<serde_json::Value>` и клонируем оттуда короткие поля: поле не того JSON-типа не валит
  строку, а неизвестные поля пропускаются без аллокации.
- Struct десериализуется из JSON-**массива** позиционно: `["a","b"]` дал бы «запись с uuid `a`».
  Поэтому голова читается только когда первый непробельный байт строки — `{`.
- Ошибкой разбора (строка становится непрозрачной и сохраняется байт-в-байт) оказываются: BOM,
  дубль **любого читаемого** поля, число вне диапазона `f64`/`i64`/`u64`, невалидный UTF-8 в
  верхнеуровневом ключе или в читаемом значении. Дубль ключа, который не читается, безвреден —
  побеждает последнее значение, как в самом читателе. Хвостовой `\r` серде терпит.
- Замеры release на этой машине (2026-09-03): слияние 28,4 МБ / 43 000 строк — 162–221 мс,
  одна строка 64 МиБ — 117 мс (в цену входит постусловие: результат разбирается заново). Бюджет
  тика (2 мин) и Stop-хука это не задевает.

## serde

- `#[serde(rename_all = "camelCase")]` на enum переименовывает варианты, но не поля
  struct-вариантов: для `Ran { common_dir }` → `commonDir` нужен `rename_all_fields`.
- `#[serde(try_from = "String", into = "String")]` на newtype даёт валидацию при десериализации
  фикстур и конфига одним местом (`EncSlug`, `StoreName`).

## cargo / clippy (1.97.1, edition 2024)

- `[package] workspace = true` — невалидно (ключ — путь к корню); наследование только пофилдно:
  `version.workspace = true`, `edition.workspace = true`, `publish.workspace = true`, плюс
  отдельная секция `[lints] workspace = true`.
- `include_str!` относителен файлу с макросом: из `crates/<crate>/tests/` до корня workspace —
  `../../../`.
- `allow-unwrap-in-tests` и родственные ключи clippy.toml действуют в `#[test]`-функциях и
  `#[cfg(test)]`, но не в хелперах интеграционного теста — в `tests/*.rs` нужен файловый
  `#![allow(clippy::panic, clippy::unwrap_used, …)]`. `indexing_slicing` имеет свой ключ
  `allow-indexing-slicing-in-tests`.
- `missing_docs = warn` под `-D warnings` доходит до `pub`-элементов в `tests/*.rs` — там нет `pub`.
- В `clippy::pedantic` 1.97 нет `module_name_repetitions` (restriction) — строка `allow` была бы
  мёртвой; `missing_errors_doc` есть. `doc_markdown` требует бэктики вокруг идентификаторов и
  CamelCase-слов в doc-комментариях; `unicode_not_nfc` ловит NFD-литералы в исходниках —
  не-ASCII в Rust-литералах только через `\u{…}`, а лучше — в JSON-фикстурах.
- Гейт «ядро без I/O» — `disallowed-methods` / `disallowed-types` / `disallowed-macros` в
  корневом `clippy.toml` (`std::fs::*`, `std::env::*`, `std::process::Command`, предикаты
  `Path::exists/metadata/read_dir/canonicalize…`, `io::stdin/stdout/stderr`, `print!`/`dbg!`,
  `thread::sleep`, `Instant::now`/`SystemTime::now`, сокеты): в чистом крейте срабатывает под
  `-D warnings`, I/O-крейт отключает одной строкой
  `#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::disallowed_macros)]`.
  Grep по `std::fs` обходится `use std::{fs, env};`, а голый список `std::fs::*` — методами
  `Path` (ревью 2026-09-02).
  Платформенные пути в списке (`std::os::unix::*`, `std::os::windows::*`) под чужой целью не существуют, и clippy
  печатает на каждый «does not refer to a reachable function» — не ошибку, но шум, в котором тонет настоящее.
  Такой записи — `allow-invalid = true`; запрет при этом действует под своей целью. Без записей `std::os::windows`
  ядро под Windows могло делать симлинки мимо гейта (2026-10-08).
- `.gitattributes`: `* text=auto eol=lf` даёт одинаковый `cargo fmt --check` на Mac и Windows;
  `fixtures/** -text` хранит фикстуры байт-в-байт (оборванные строки, CRLF внутри).
- `rust-toolchain.toml` с точным `channel` — единственный источник версии компилятора;
  `rust-version` в манифесте его дублировал бы.
