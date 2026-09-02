# Кодировка каталога проекта `projects/<enc>` (2026-09-02)

Точная формула, по которой Claude Code выбирает каталог транскрипта, и всё, что вокруг неё
проверено на бинарях 2.1.232 (терминал) и 2.1.255 (встроенный в Desktop) и живыми прогонами с
изолированным `CLAUDE_CONFIG_DIR`. Единственная запись про кодировку: [transcriptFormat.md](transcriptFormat.md),
[configDirLayout.md](configDirLayout.md) и [hooksLifecycle.md](hooksLifecycle.md) ссылаются сюда.
Реализация — `crates/vibememory-core/src/naming/enc.rs`, оракул — `fixtures/naming/encodeCwdOracle.js`,
эталоны — `fixtures/naming/encodeCwd.json`.

## Формула (strings обоих бинарей)

2.1.232 (`yTo` / `xT` / `xAy` / `Ynt`, `Pre=200`) и 2.1.255 (`k` / `PA` / `be` / `t4`, `rL=200`) —
одинаковые тела под разными минифицированными именами:

```js
yTo(e) = e.replace(/[^a-zA-Z0-9]/g, "-")
xT(e)  = { t = yTo(e); return t.length <= 200 ? t : `${t.slice(0,200)}-${xAy(e)}` }
xAy(e) = Math.abs(Ynt(e)).toString(36)
Ynt(e) = { t = 0; for (r = 0; r < e.length; r++) t = (t << 5) - t + e.charCodeAt(r) | 0; return t }
```

Следствия, важные для переноса на другой язык:

- **Единица — UTF-16 code unit** (`charCodeAt`, `length`): символ вне BMP (эмодзи) даёт два
  дефиса, NFD-«é» (e + U+0301) — два, NFC-«é» — один.
- **Хэш считается от полной исходной строки**, не от слага; арифметика — wrapping int32
  (`|0`), затем `Math.abs` (не переполняется: abs(−2³¹) = 2³¹ → `zik0zk`) и base36 строчными.
- Порог — ровно 200 символов слага: 200 сохраняются целиком, 201 → первые 200 + `-` + хэш
  (`/` + 200×`x` → хэш −786901713 → `d0i18x`, длина результата 207).

## Что CLI подаёт на вход: `NFC(realpath(cwd))`

Опровергнуто прежнее «ключ — cwd как запущен, не realpath; NFC делает только Desktop»:

- 2.1.232: `ZI(e) = Vu(await realpath(e))` с фолбэком `Vu(e)`, `Vu(e) = e.normalize("NFC")`;
  `r1c` нормализует `originalCwd` и `cwd` в NFC (strings, воспроизведено). 2.1.255 — та же
  связка по отчёту верификатора (resolve → realpath с фолбэком → NFC); имя обёртки локально не
  воспроизведено, измерение NFC в stdin на 2.1.255 — прогон верификатора в той же сессии.
- Прогон 2026-09-02 (2.1.232, изолированный `CLAUDE_CONFIG_DIR`, каталог `cafe` + U+0301 на диске,
  `getcwd` отдаёт NFD-байты `63 61 66 65 cc 81`): в stdin SessionStart-хука `cwd` уже в NFC
  (`63 61 66 c3 a9`), `transcript_path` — `…/projects/-…-work-caf-/<sid>.jsonl`. Верификатор в
  той же сессии повторил прогон на 2.1.255 — тот же результат.
- Следствие: слаг = формула(NFC(физический путь)). Для движка это значит: ядро применяет NFC само
  (`unicode-normalization`), realpath делает CLI до вызова, а stdin хука уже нормализован.
  Остаточное расхождение — Windows-CLI (`realpathSync` без разворота junction/subst, не проверено).

## `CLAUDE_CODE_PROJECT_DIR_NAME` — контракт (2.1.255)

```js
D = /^[A-Za-z0-9_-]{1,64}$/, C = /^(?:con|prn|aux|nul|com[0-9]|lpt[0-9])$/i
uIn(e) { if (!e || !D.test(e) || C.test(e)) return; return e }   // иначе — молча вычисленный слаг
```

Прогоны 2026-09-02 (2.1.255, изолированный конфиг): `Weird Name.x` → проигнорировано (каталог по
формуле), `my_name-1` → `projects/my_name-1/`, `aux` → проигнорировано. В 2.1.232 переменной нет
(документация: с 2.1.234). Итог: имя каталога проекта всегда ∈ `[A-Za-z0-9_-]+` — формула даёт
`[A-Za-z0-9-]`, переменная добавляет `_`. Desktop ставит переменную в `session` для сессий типа
`3p` — один каталог на все cwd такого окружения ([hooksLifecycle.md](hooksLifecycle.md)).

## Эталоны

| cwd | enc | откуда |
|---|---|---|
| `/Volumes/Storage/Projects/VibeCode/VibeMemory` | `-Volumes-Storage-Projects-VibeCode-VibeMemory` | реальный каталог |
| `D:\Projects\VibeCode\VibeIDE` и `D:/Projects/VibeCode/VibeIDE` | `D--Projects-VibeCode-VibeIDE` | реальный каталог; форма с `/` — оракул |
| `/` | `-` | реальный каталог (раннер VibeDub) |
| `/Users/borodatych/Projects/café` (NFC или NFD) | `-Users-borodatych-Projects-caf-` | прогон |
| `/tmp/🚀` | `-tmp---` | оракул |
| `/` + 200×`x` | 200 символов + `-d0i18x` | оракул |

## Что из этого следует для движка

- `enc` живой сессии — только `basename(dirname(transcript_path))`; валидируется форма
  `<…>/projects/<enc>/<sid>.jsonl` и алфавит `[A-Za-z0-9_-]+`.
- Формула нужна ровно в одном месте — реконсилеру тика, чтобы завести ссылку для cwd с другой
  машины до первой сессии. Это предсказание: сверяется с `transcript_path` на SessionStart, при
  расхождении создаётся вторая ссылка, предсказанная никогда не удаляется и не считается
  подтверждённой для гейта импорта дескрипторов Desktop.
- Новая версия CLI = `strings` бинаря по этим четырём функциям и прогон
  `fixtures/naming/encodeCwd.json`; порядок — в [manuals/namingFixtures.md](../../manuals/namingFixtures.md).
