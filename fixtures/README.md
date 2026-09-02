# Фикстуры

Записи реальных форматов и реального вывода инструментов, на которых тестируется ядро
(`vibememory-core`). Файлы хранятся байт-в-байт (`fixtures/** -text` в `.gitattributes`).

## `naming/` — кодировка `projects/<enc>` и имя стора

- `encFromTranscriptPath.json`, `encodeCwd.json`, `resolveStoreName.json`, `namingConfig.json` —
  кейсы; формат, метки и порядок добавления — [docs/manuals/namingFixtures.md](../docs/manuals/namingFixtures.md).
- `encodeCwdOracle.js` — точная JS-формула Claude Code (2.1.232 / 2.1.255) для `computed`-кейсов.

## Происхождение и санитизация

- Метка `provenance` у каждого кейса: `observed` (снято с этой машины 2026-09-02: вывод
  `git rev-parse`, содержимое `.git`-файлов, каталоги, созданные CLI с изолированным
  `CLAUDE_CONFIG_DIR`), `computed` (оракул или правило), `unverified` (Windows — этап 4).
- Пути проектов владельца настоящие: они уже опубликованы в `docs/knowledge/`. Scratch-каталоги
  сокращены до `/private/tmp/scratch/<имя>` вместо полного пути scratchpad сессии — кроме
  `observed`-кейсов кодировки, где путь и есть вход (там путь полный, значение — реальное имя
  каталога, созданного CLI).
- Session id — синтетические UUID, кроме двух изолированных прогонов CLI, где id ничего не
  идентифицирует. Ни одной строки транскрипта, ни реальных идентификаторов машин, ни имён хостов
  и remote в фикстурах нет и быть не должно.
