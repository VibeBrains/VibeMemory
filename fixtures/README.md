# Фикстуры

Записи реальных форматов и реального вывода инструментов, на которых тестируется ядро
(`vibememory-core`). Файлы хранятся байт-в-байт (`fixtures/** -text` в `.gitattributes`).

## `naming/` — кодировка `projects/<enc>` и имя стора

- `encFromTranscriptPath.json`, `encodeCwd.json`, `resolveStoreName.json`, `namingConfig.json`,
  `canonicalCwd.json` (лексическая канонизация cwd: verbatim, NFC, `.`/`..`) —
  кейсы; формат, метки и порядок добавления — [docs/manuals/namingFixtures.md](../docs/manuals/namingFixtures.md).
- `encodeCwdOracle.js` — точная JS-формула Claude Code (2.1.232 / 2.1.255) для `computed`-кейсов.

## `merge/` — слияние JSONL-транскриптов

- `mergeScenarios.json` — сценарии слияния транскриптов; `keepBothScenarios.json` — сценарии
  выбора целых файлов (память и сайдкары); `surveyScenarios.json` — опись транскрипта без
  слияния (строки, хвост, `lastUuid`, лист возобновления, непрозрачные строки), ожидания
  реальных файлов посчитаны независимой реализацией. Формат и порядок добавления кейса —
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

## `export/` — что уходит с машины

- `exportScenarios.json` — пути `~/.claude` и вердикт гейта экспорта (белый список против
  чёрного, порог 45 МиБ). Формат кейса и порядок добавления —
  [docs/manuals/exportFixtures.md](../docs/manuals/exportFixtures.md).
- Путей владельца здесь нет сверх уже опубликованных в `docs/knowledge/`; все идентификаторы
  сессий синтетические.

## `desktop/` — карточки сессий Desktop

- `descriptorScenarios.json` — перевод `cwd` между машинами (`{ROOT}/rel`), храповик экспорта
  (не понижать `cliSessionId`, не выпускать `transcriptUnavailable`), гейт импорта и локальная
  починка. Формат и порядок добавления — [docs/manuals/desktopFixtures.md](../docs/manuals/desktopFixtures.md).
- Реального дескриптора здесь нет ни одного: стор лежит в облачной папке, чтение дегидрированных
  файлов тянет их из сети. Форма полей — из частот по 165 файлам в
  `docs/knowledge/claudeDesktop/sessionStore.md`, значения синтетические.

## `links/` — `machines/<id>/links.json`

- `linksScenarios.json` — что файл принимает, что отвергает и что переживает запись обратно;
  плюс сравнение имён стора. Формат и порядок добавления —
  [docs/manuals/linksFixtures.md](../docs/manuals/linksFixtures.md).
- Все кейсы `computed`: формат наш, первый настоящий файл появится с `install` на этапе 2.

## `config/` — `~/.vibememory/config.json`

- `configScenarios.json` — что конфиг принимает, что отвергает и во что превращаются корни.
  Все кейсы `computed`, кроме Windows-раскладки стора Desktop (`unverified`, этап 4).
  Описание ключей для модели — [docs/manuals/configSpec.md](../docs/manuals/configSpec.md).

## `access/` — снимок прав хоста

- `accessSnapshots.json` — снимки `access.json` и первый код, который отвечает на них
  `vibememory-mcp access check`. `base` — годный снимок; каждый кейс — его правка по JSON Merge
  Patch (RFC 7396: `null` удаляет поле, массив заменяется целиком) или сырой текст в `raw`. Коды и
  поля — [docs/manuals/accessSnapshotSpec.md](../docs/manuals/accessSnapshotSpec.md); гейт сверяет,
  что каждый код спеки покрыт кейсом и каждый кейс называет код спеки.
- Все кейсы `computed`: формат наш, первый настоящий снимок напишет кабинет. Участники, машины и
  отпечатки токенов синтетические; публичные ключи — синтетические ed25519, у которых все 32 байта
  ключа одинаковые, и гейт приватности проверяет это для любого `ssh-ed25519` в фикстурах.
- `preReceiveUpdates.json` — что решает `pre-receive` для push в стор команды: строки обновлений,
  изменённые пути `main` и размеры против снимка прав (база `accessSnapshots.json` с правкой кейса).

## `shell/` — принудительная команда ключа машины

- `originalCommands.json` — что ключу машины можно запустить на хосте: `SSH_ORIGINAL_COMMAND`
  против снимка прав, включая инъекции `;`, `..`, чужой путь и `sync` против `memory`.
- `statusAnswer.json` — ответ `status` ключу: снимок и `host.json`, спроецированные на команды
  ключа, без слагов и размеров чужих команд.

## `host/` — отчёт хоста и применение снимка

- `status.json` — `host.json`, каким его собирает функция `status` из применённого снимка,
  `applied.json` и того, что она нашла на хосте. Формат — [docs/manuals/hostStatusSpec.md](../docs/manuals/hostStatusSpec.md).
- `applyPlans.json` — что `access-apply` планирует для каталога `teams/` и какой
  `authorized_keys` пишет для `vmgit`.


## `claim/` — обмен claim-кода на токен

- `claimAnswers.json` — что отвечает `POST /api/agent/claim` кабинета и как это читает
  `vibememory connect`: код выхода `curl` и его stdout на входе, выдача или причина на выходе. Тест
  кабинета сверяет, что его ответы несут ровно поля кейсов `token` и `key`; тест ядра разбирает
  каждый кейс. Токены синтетические: секрет — один повторённый символ, как требует гейт приватности.
- `icaclsOutput.json` — вывод `icacls` для файла токена на Windows и вердикт `doctor`: сверка с
  текущим пользователем (`whoami`), а не с именами групп — Windows печатает их на языке системы.
  Метка `unverified`: первый настоящий вывод даст GPD (фаза W).

## Происхождение и санитизация

- Метка `provenance` у каждого кейса: `observed` (снято с этой машины 2026-09-02: вывод
  `git rev-parse`, содержимое `.git`-файлов, каталоги, созданные CLI с изолированным
  `CLAUDE_CONFIG_DIR`), `computed` (оракул или правило), `unverified` (путь заводит другая платформа или Desktop: Windows — этап 4, каталоги Cowork и реестр устройства — не на этой машине).
- Пути проектов владельца настоящие: они уже опубликованы в `docs/knowledge/`. Scratch-каталоги
  сокращены до `/private/tmp/scratch/<имя>` вместо полного пути scratchpad сессии — кроме
  `observed`-кейсов кодировки, где путь и есть вход (там путь полный, значение — реальное имя
  каталога, созданного CLI).
- Session id — синтетические UUID. Оговорка про изолированные прогоны CLI относится только к
  `naming/`: в `merge/` синтетические все `sessionId` без исключений (скраббер не имеет для них
  ветки). Ни одной строки транскрипта, ни реальных идентификаторов машин, ни имён хостов
  и remote в фикстурах нет и быть не должно.
