# Как работать с фикстурами имён (`fixtures/naming/`)

Все ожидания о кодировке `projects/<enc>` и об имени стора живут в четырёх JSON-файлах;
драйвер — `crates/vibememory-core/tests/naming_fixtures.rs`. Новое правило или новый край —
это новый кейс, а не новый unit-тест с литеральным путём.

## Шаг 1. Выбрать файл

| Файл | Проверяет | Вход | `ok` |
|---|---|---|---|
| `encFromTranscriptPath.json` | `enc_from_transcript_path` | `{ "transcriptPath", "syntax": "posix" \| "windows" }` | строка enc |
| `encodeCwd.json` | `encode_cwd` | `{ "cwd" }` | строка enc |
| `resolveStoreName.json` | `resolve_store_name` | `{ "cwd", "syntax", "projectDirName"?, "git"?, "config"?, "existing"? }` | `{ "named": { "name", "source" } }` или `{ "ignored": { "pattern" \| "projectDirName" } }` |
| `namingConfig.json` | `NamingConfig::from_raw` | `{ "config": { "nameOverrides"?, "ignoreCwd"? } }` | `true` |

`git` в `resolveStoreName.json` — запись `GitProbe`:
`{ "ran": { "commonDir": "<stdout или null>", "dotGit": "notFound" | { "found": { "dir": { "path" } } } | { "found": { "file": { "path", "content" } } } | { "unknown": { "reason" } } } }`
или `{ "unavailable": { "reason" } }`. Кейс **без** поля `git` утверждает, что git не должен
вызываться (ignore и override решают раньше) — драйвер подставляет паникующее замыкание.

## Шаг 2. Записать кейс

```json
{
  "id": "camelCaseUnique",
  "provenance": "observed",
  "note": "Where the input came from and what the case proves.",
  "input": { "...": "..." },
  "expect": { "ok": "..." }          // или { "error": "<код NamingError>" }
}
```

- `id` уникален в файле; `note` непустой, по-английски (как комментарии в коде).
- `provenance`: `observed` — вход записан с реальной системы (вывод git, содержимое `.git`-файла,
  каталог, созданный CLI) с датой в `note`; `computed` — вход сконструирован, ожидание получено
  оракулом или следует из правила; `unverified` — намеренное поведение на платформе, которую в
  сессии не прогоняли (Windows), с пометкой «этап 4». `unverified`-кейсы **прогоняются**: они
  фиксируют намерение, после проверки на машине метка меняется.
- Ошибка сравнивается по коду (`NamingError::code()`), не по тексту. Коды:
  `malformedTranscriptPath`, `invalidEncSlug`, `relativeCwd`, `rootCwd`, `gitUnavailable`,
  `dotGitProbeFailed`, `gitRefused`, `gitLayoutMismatch`, `unrecognizedGitLayout`, `emptyRepoName`,
  `malformedDotGitFile`, `invalidStoreName`, `storeNameCollision`, `ambiguousOverride`, `configInvalid`.
- Не-ASCII — только `\u`-экранированием (`"caf\u00e9"`, `"cafe\u0301"`): NFC и NFD должны быть
  различимы глазами. Session id — синтетические UUID (кроме изолированных прогонов CLI, где
  реальный id ничего не идентифицирует); содержимого транскриптов в фикстурах нет.
- Scratch-пути сокращаются до `/private/tmp/scratch/...` (см. `fixtures/README.md`) — кроме
  `observed`-кейсов enc, где путь и есть вход: там путь настоящий и полный.

## Шаг 3. Получить `computed`-значение для `encodeCwd.json`

```bash
node fixtures/naming/encodeCwdOracle.js '/Volumes/x/' "$(printf '/tmp/caf\xc3\xa9')"
```

Оракул — точная JS-формула из бинарей CLI (с `normalize('NFC')`); значение из его вывода и есть
`ok`. Для путей с NFD/эмодзи удобнее генерировать строки через `printf`.

## Шаг 4. Прогнать

```bash
cargo test -p vibememory-core --test naming_fixtures
```

Драйвер показывает все несовпадения разом в формате `<файл>#<id> [<provenance>]: expected …, got …`;
он же падает на дубликате `id`, пустом `note`, неизвестном поле (в структурах кейса и в записях
`GitProbe`) и кейсе, у которого `expect` не содержит ровно одно из `ok` / `error`.

## Новая версия Claude Code

1. `strings <бинарь> | grep` по четырём функциям кодировки (тела в
   [knowledge/claudeCode/projectDirEncoding.md](../knowledge/claudeCode/projectDirEncoding.md)) и по
   регексу `CLAUDE_CODE_PROJECT_DIR_NAME`.
2. Изменилось — обновить оракул и `cliVersions` в `encodeCwd.json`, добавить кейс на различие,
   записать факт в `projectDirEncoding.md`.
3. Не изменилось — дописать версию в `cliVersions` и прогнать тесты.
