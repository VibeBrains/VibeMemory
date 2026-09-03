# Фикстуры

Записи реальных форматов и реального вывода инструментов, на которых тестируется ядро
(`vibememory-core`). Файлы хранятся байт-в-байт (`fixtures/** -text` в `.gitattributes`).

## `naming/` — кодировка `projects/<enc>` и имя стора

- `encFromTranscriptPath.json`, `encodeCwd.json`, `resolveStoreName.json`, `namingConfig.json` —
  кейсы; формат, метки и порядок добавления — [docs/manuals/namingFixtures.md](../docs/manuals/namingFixtures.md).
- `encodeCwdOracle.js` — точная JS-формула Claude Code (2.1.232 / 2.1.255) для `computed`-кейсов.

## `merge/` — слияние JSONL-транскриптов

- `mergeScenarios.json` — сценарии слияния; формат и порядок добавления кейса —
  [docs/manuals/mergeFixtures.md](../docs/manuals/mergeFixtures.md).
- `cli255Session.jsonl` (272 строки среза реальной сессии 2026-09-02, CLI 2.1.255),
  `cli255Isolated.jsonl` и `cli232Isolated.jsonl` (изолированные прогоны двух версий),
  `cli255Subagent.jsonl` (сайдчейн), `cli255Journal.jsonl` (журнал воркфлоу: ни `uuid`, ни
  `timestamp`) — все пропущены через `scrubTranscript.js`.
- `scrubTranscript.js` — скраббер: белый список ключей, перехеширование идентификаторов с
  сохранением формы, `<scrubbed>` для всего остального.

Что в этих фикстурах **остаётся** намеренно: `timestamp` (порядок — предмет слияния), версия CLI,
перечислимые значения (`type`, `subtype`, `operation`, `role`, `model`, `stop_reason`, `level`,
`mode`, `effort`, `permissionMode`, `userType`, `entrypoint`, `origin`, `promptSource`, `atis`,
`agentType`, `service_tier`, `status`, `reason`, `stop_sequence`), имена инструментов и
MCP-серверов (`name`), структура объектов и порядок ключей.
Что **не остаётся**: тексты сообщений, содержимое инструментов и вложений, заголовки сессий,
имена агентов, реальные идентификаторы (перехешированы), реальные пути и ветка.

## Происхождение и санитизация

- Метка `provenance` у каждого кейса: `observed` (снято с этой машины 2026-09-02: вывод
  `git rev-parse`, содержимое `.git`-файлов, каталоги, созданные CLI с изолированным
  `CLAUDE_CONFIG_DIR`), `computed` (оракул или правило), `unverified` (Windows — этап 4).
- Пути проектов владельца настоящие: они уже опубликованы в `docs/knowledge/`. Scratch-каталоги
  сокращены до `/private/tmp/scratch/<имя>` вместо полного пути scratchpad сессии — кроме
  `observed`-кейсов кодировки, где путь и есть вход (там путь полный, значение — реальное имя
  каталога, созданного CLI).
- Session id — синтетические UUID. Оговорка про изолированные прогоны CLI относится только к
  `naming/`: в `merge/` синтетические все `sessionId` без исключений (скраббер не имеет для них
  ветки). Ни одной строки транскрипта, ни реальных идентификаторов машин, ни имён хостов
  и remote в фикстурах нет и быть не должно.
