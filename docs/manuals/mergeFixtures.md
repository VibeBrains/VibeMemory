# Как работать с фикстурами слияния (`fixtures/merge/`)

Здесь два набора: слияние транскриптов (`mergeScenarios.json`, драйвер
`crates/vibememory-core/tests/merge_fixtures.rs`) и выбор целых файлов —
памяти и сайдкаров (`keepBothScenarios.json`, драйвер `tests/keep_both_fixtures.rs`,
[раздел ниже](#фикстуры-keep-both)).

Все ожидания о слиянии JSONL живут в `fixtures/merge/mergeScenarios.json`; драйвер —
`crates/vibememory-core/tests/merge_fixtures.rs`. Новое правило или новый край — это новый кейс,
а не unit-тест с литеральными строками. Правила слияния и их обоснование —
[knowledge/design/jsonlMergeInvariants.md](../knowledge/design/jsonlMergeInvariants.md).

## Шаг 1. Подготовить транскрипт (если нужен новый)

Фикстуры-транскрипты — реальные файлы, пропущенные через скраббер:

```bash
node fixtures/merge/scrubTranscript.js <исходный.jsonl> [--lines 0..272] > fixtures/merge/<имя>.jsonl
```

Скраббер — белый список: значения ключей `type`, `subtype`, `operation`, `role`, `model`,
`version`, `userType`, `entrypoint`, `permissionMode`, `origin`, `promptSource`, `level`, `mode`,
`stop_reason`, `stop_sequence`, `service_tier`, `status`, `reason`, `name`, `agentType`, `effort`,
`atis`, `timestamp` остаются как есть; числа,
булевы и `null` остаются всегда; идентификаторы (`uuid`, `parentUuid`, `leafUuid`, `promptId`,
`toolUseID`, `agentId`, `requestId`, `toolu_*`, `req_*`, `msg_*`, `cse_*`, ключи журнала
`v2:<sha>`) перехешируются `sha256("vibememory-fixture:" + значение)` с сохранением формы, так
что равенство и граф `parentUuid` целы; `sessionId` и `owner*Uuid` становятся синтетическими
константами; `cwd` и подобные — `/private/tmp/scratch/work`; **всё остальное** —
`"<scrubbed>"`. Порядок ключей сохраняется; прогон одного и того же исходника детерминирован, но
**повторно прогонять уже сскрабленную фикстуру нельзя**: идентификаторы перехешируются заново и
разойдутся с ожиданиями в `mergeScenarios.json`, где `uuid` выписаны литералами. Перед правкой
каждой строки проверяется `JSON.stringify(JSON.parse(line)) === line`: если писатель Claude Code
перестанет быть `JSON.stringify`, скраббер откажется работать и назовёт строку. Манифест
(строки, размеры, типы записей) печатается в stderr — его цифры идут в шапку сценариев.

Новый файл — одна строка в `fn transcript(...)` драйвера и одна запись в `transcripts` шапки.

## Шаг 2. Записать кейс

```json
{
  "id": "deletedByTheirsThreeWay",
  "provenance": "computed",
  "note": "Theirs dropped a line that base and ours still have: the deletion is honoured (lines 0..20 of the recorded session, CLI 2.1.255, 2026-09-02).",
  "input": {
    "base":   { "file": "cli255Session.jsonl", "lines": [0, 20] },
    "ours":   { "file": "cli255Session.jsonl", "lines": [0, 20] },
    "theirs": { "file": "cli255Session.jsonl", "lines": [0, 20], "ops": [{ "dropLines": [7] }] }
  },
  "expect": {
    "result": [{ "from": "theirs", "lines": [0, 19] }],
    "report": {
      "baseTruncated": 0,
      "ours":   { "truncated": 0, "added": 0, "addedUuid": 0, "dropped": 1, "resurrected": 0, "absent": false },
      "theirs": { "truncated": 0, "added": 0, "addedUuid": 0, "dropped": 0, "resurrected": 0, "absent": false },
      "conflicts": [], "opaque": 0, "beforeBoundary": 0, "resultEqualsOurs": false, "fork": null
    }
  }
}
```

### Вход (`base`, `ours`, `theirs`)

| Поле | Значение |
|---|---|
| `empty: true` | пустой вход; других полей быть не должно (`base` можно просто опустить) |
| `file` | имя из `transcripts` |
| `lines: [a, b]` | полуоткрытый диапазон 0-based строк файла; без него — весь файл |
| `ops` | правки, применяются **по порядку** к текущему состоянию |

Операции (индексы — по состоянию **до** данной операции):

| Операция | Что делает |
|---|---|
| `{"cutTailBytes": N}` | срезает N байт с конца; `1` убирает финальный `\n`, больше — рвёт последнюю запись |
| `{"appendLines": ["…", …]}` | дописывает строки целиком (только UTF-8) |
| `{"appendHex": "7b22…"}` | дописывает сырые байты — единственный способ выразить не-UTF-8 |
| `{"dropLines": [i, …]}` | удаляет строки с этими индексами |
| `{"duplicateLine": i}` | вставляет копию строки `i` сразу после неё |
| `{"setLine": [i, "…"]}` | заменяет строку `i` |

### Ожидание (`expect`)

- `result` — список выборок из **материализованных** входов: `{"from": "ours"|"theirs"|"base",
  "lines": [a, b]}` или `{"from": …, "pick": [i, …]}`. Индексы — по строкам входа после `ops` и
  после отсечения оборванного хвоста. Golden-файлов нет: ожидание должно читаться глазами.
- `report` — **полный** объект отчёта, сравнивается как JSON: `fork` либо `null`, либо
  `{"tailSide": "ours"|"theirs", "visible": "ours"|"theirs"|null}`.

### Метки `provenance`

- `observed` — вход собран только из записанных строк (`lines`, без `ops`); в `note` — версия CLI
  и дата записи.
- `computed` — есть `ops` или сконструированные строки; ожидание следует из правил.
- `unverified` — поведение версии или платформы, не прогнанной здесь (писатель 2.1.232 без
  «запечатывания» хвоста, редакторы Windows с CRLF).

## Шаг 3. Прогнать

```bash
cargo test -p vibememory-core --test merge_fixtures
```

Драйвер печатает все несовпадения разом (`mergeScenarios.json#<id> [<provenance>]: …`, без
содержимого строк — только тип, префикс `uuid` и длина) и падает на дубликате `id`, пустом
`note`, неизвестном поле, неизвестном имени транскрипта и на кейсе, у которого выборка не
описывает результат. Кроме записанного ожидания на каждом кейсе проверяются законы: `merge(x,x,x)
= x`, `merge(∅,x,∅) = merge(∅,∅,x) = x`, коммутативность мультимножества ключей и «закон второй
машины» `keys(merge(o,o,r)) = keys(r) = keys(merge(t,t,r))`; идентичность строк тест считает
собственным кодом, а не кодом крейта.

## Новая версия Claude Code

1. Прогнать изолированную сессию новой версией (`CLAUDE_CONFIG_DIR` во временный каталог, хук,
   пишущий stdin) и получить свежий транскрипт.
2. Пропустить его через скраббер: отказ по round-trip означает, что писатель сменил
   сериализатор — это факт для [transcriptFormat.md](../knowledge/claudeCode/transcriptFormat.md).
3. `strings` бинаря по `sealTornTailSync`, `setTailTorn`, `removeMessageByUuid`,
   `reAppendSessionMetadata`, `performCompactTranscript`, `last-prompt`, `compact_boundary` —
   обновить таблицу версий в `transcriptFormat.md`.
4. Добавить файл в `fixtures/merge/`, строку в `transcripts`, версию в `cliVersions` и кейс
   `identical` для нового файла; прогнать тесты.

## Фикстуры keep-both

`fixtures/merge/keepBothScenarios.json` описывает второй драйвер: он не сливает содержимое, а
выбирает целый файл — память, `custom-title.json`, `workflows/*.json`, бинарные `tool-results/**`.
Кейс:

```json
{
  "id": "keptBothMemoryIndex",
  "provenance": "computed",
  "note": "Both machines appended a different line to MEMORY.md: ours stays in the store and theirs is quarantined.",
  "input": {
    "path": "projects/VibeIDE/memory/MEMORY.md",
    "stamp": "mac-main-2026-09-03T10-15-00Z",
    "base":   { "text": "# Memory\n\n" },
    "ours":   { "text": "# Memory\n\n- [Store naming](storeNaming.md)\n" },
    "theirs": { "text": "# Memory\n\n- [Merge rules](mergeRules.md)\n" }
  },
  "expect": {
    "result": "ours",
    "quarantine": { "name": "projects-VibeIDE-memory-MEMORY-mac-main-2026-09-03T10-15-00Z.md" },
    "report": { "resolution": "keptBoth", "kind": "memory", "oursBytes": 46, "theirsBytes": 44 }
  }
}
```

| Поле | Значение |
|---|---|
| `input.path` | путь файла внутри стора (`%P` драйвера), разделитель `/` |
| `input.stamp` | машина и момент от вызывающего; попадает в имя карантина и санитизируется тем же правилом |
| `base` / `ours` / `theirs` | версия файла: `{"text": "…"}`, `{"hex": "ffd8…"}` для не-UTF-8 или `{"empty": true}` |
| `expect.result` | какому входу обязаны быть равны байты результата: `"ours"`, `"theirs"` или `"base"` |
| `expect.quarantine` | `null` либо `{"name": "…"}` — ожидаемое имя отложенной версии |
| `expect.report` | полный отчёт: `resolution` (`identical`/`tookOurs`/`tookTheirs`/`keptBoth`), `kind` (`memory`/`other`), размеры |

Имя карантина считается по правилу: путь без расширения, каждый символ вне `[A-Za-z0-9._-]` → `-`,
затем `-<штамп>` и расширение; длиннее 255 байт — обрезается голова пути. Ожидаемые имена в
фикстурах вычислены независимой реализацией этого правила
(`scratchpad/keepBothScenarios.py` сессии), а не самим модулем.

Имя детерминировано, но не уникально по построению: два разных пути могут санитизироваться в одно
имя (`a/b.md` и `a-b.md`), поэтому карантин пишется `create_new` с суффиксом при совпадении — это
обязанность CLI, кейс `quarantineNamesCanCollide` фиксирует само правило имени.

Кроме записанного ожидания драйвер проверяет на каждом кейсе: результат равен одному из входов;
карантин есть тогда и только тогда, когда `resolution` = `keptBoth`; имя — один компонент пути не
длиннее 255 байт; повторное слияние результата ничего не меняет, и вторая машина сходится на нём.

Метки `provenance` те же. Сейчас все кейсы `computed`: файлов памяти на этой машине нет
(локальный `memory/` пуст, сторы остальных проектов лежат в OneDrive и читать их нельзя), поэтому
содержимое сконструировано по документированному формату памяти и по константам `memory` /
`MEMORY.md`, снятым из бинаря.
