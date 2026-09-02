# Формат транскрипта сессии Claude Code (2026-09-02)

Проверено при проектировании слияния транскриптов между машинами: CLI 2.1.232 (терминал) и
2.1.255 (встроенный в Claude Desktop 1.40609.1), macOS, живые транскрипты владельца
(VibeSweep `e3bbbf16…` на 2.1.255, VibeShot `7bad7d59…` на 2.1.219), `strings` обоих бинарей,
`lsof` по живым процессам. Первоисточники — [research/researchReport.md](../research/researchReport.md),
[research/synthesisReport.md](../research/synthesisReport.md),
[research/judgeFindings.md](../research/judgeFindings.md). Официальная позиция Anthropic:
формат записей «internal to Claude Code and changes between versions, so scripts that parse
these files directly can break on any release»
([sessions](https://code.claude.com/docs/en/sessions#where-transcripts-are-stored)) — всё
ниже верно для перечисленных версий и обязано быть закрыто фикстурными тестами на каждую новую.
Перепроверено верификатором 2026-09-02 12:40–12:55 (read-only, те же файлы и strings): цифры по
живому VibeSweep-файлу пересняты при 563 строках, по VibeShot — при 2257.

## Каталог проекта — абсолютный cwd, каждый не-алфавитно-цифровой символ → «-»

Путь: `<CLAUDE_CONFIG_DIR>/projects/<enc(cwd)>/<sessionId>.jsonl`. В бинаре 2.1.255:
`function k(e){return e.replace(/[^a-zA-Z0-9]/g,"-")}`; длиннее 200 символов — обрезка до 200
плюс `-` и base36-хэш полного пути (2.1.224 чинил коллизию длинных путей). Примеры из стора:
`/Volumes/Storage/Projects/VibeCode/VibeSweep` → `-Volumes-Storage-Projects-VibeCode-VibeSweep`,
`D:\Projects\VibeCode\VibeIDE` → `D--Projects-VibeCode-VibeIDE`, `cwd=/` → `-`. Ключ — cwd как
запущен, не realpath: `claude project purge` проверяет оба варианта (`resolve` и `realpath`), а
2.1.50 чинил «resumed sessions could be invisible when the working directory involved symlinks».
Desktop нормализует cwd в NFC перед той же заменой (`cliSanitizeCwdSimple` в бандле; хелперы путей
Desktop делают `normalize('NFC')` только на darwin). Единственный штатный способ уйти от
привязки к пути — `CLAUDE_CODE_PROJECT_DIR_NAME` (2.1.234+): одно имя на запуск, требует
`CLAUDE_CONFIG_DIR`, читается только из окружения процесса.

Симлинк допустим на уровне каталога `projects/<enc>` (текущая сессия так и пишет), но не на
самом `.jsonl`: Desktop отказывается грузить транскрипт, если lstat/fstat расходятся
(«a symlink at the file itself, a FIFO, or an unverifiable reparse point»,
[#90903](https://github.com/anthropics/claude-code/issues/90903)). Результат слияния —
всегда обычный файл. Оговорка: слой private-file самого Desktop объявляет любую ссылку под корнем
конфига неподдерживаемой («components under the config root may not be symlinks»; запись →
`PlantDetectedError`, чтение → `warnAndRead`) — ссылки `projects/<enc>` сегодня терпятся, а не
поддерживаются, и это может ужесточиться любым релизом.

## Файл рождается после SessionStart, первая запись — `queue-operation`

При срабатывании SessionStart(startup) не существует ни файла `transcript_path`, ни каталога
`projects/<enc>/` (проверено t+0…t+3 с в `-p`-режиме 2.1.232 с изолированным
`CLAUDE_CONFIG_DIR`). Файл появляется в момент постановки первого промпта в очередь: первые
три строки — `queue-operation enqueue` 08:22:21.656Z, `dequeue` 08:22:21.657Z, `user`
08:22:21.675Z. CLI ждёт завершения SessionStart-хука: в racetest хук закончился 11:59:26.376,
первая запись легла 11:59:26.392 (+16 мс); в sstest хук сам работал 4 с (t+0…t+3), поэтому файл
появился через ≈4 с после его старта. В Desktop (stream-json) первая запись ≈340 мс после
`createdAt` дескриптора (07:24:46.761Z → 07:24:47.099Z). Файл создаётся даже без логина (stdout
«Not logged in»). Интерактивный TTY отдельно не прогонялся.

## Запись — open/append/close, дескриптор не удерживается

`lsof -p 6278` (CLI 2.1.255 под Desktop, аптайм 2:00 при перепроверке) и `lsof -p 11657` — 0 открытых
`.jsonl`, 0 файлов под `projects/`; `lsof -c claude | grep jsonl` — 0 (снято дважды: при написании и
при перепроверке). Следствия: замена inode
(`tmp+rename`, `git checkout`) сама по себе дописи не теряет — следующий append откроет файл
по пути заново; но загруженный в память разговор обновить нельзя ничем, и ввод в открытую
вкладку после работы на другой машине ветвит транскрипт молча. Прецедент 2026-08-08
(«открытый дескриптор продолжил писать в перенесённую папку») этим измерением не
подтверждается и остаётся без объяснения. Документация: «The transcript file is written
asynchronously and may lag the in-memory conversation». Снимок живого файла может захватить
оборванную последнюю строку: брать до последнего `\n`, неразбираемые строки отбрасывать —
CLI сам парсит их в try/catch и молча пропускает.

## Четверть–треть записей без `uuid` — состояние, а не сообщения

VibeSweep `e3bbbf16…` при 423 строках: 142 записи без `uuid` (33,6 %: `queue-operation` 24,
`custom-title` 20, `bridge-session` 19, `last-prompt` 18, `ai-title` 17, `atis-latch` 17,
`agent-name` 16, `mode` 11); повторный замер того же живого файла при 563 строках — 197 (35,0 %,
доля стабильна): `queue-operation` 30, `custom-title` 27, `bridge-session` 26, `last-prompt` 25,
`ai-title` 24, `atis-latch` 24, `agent-name` 23, `mode` 18. Поля: `queue-operation` —
`{operation, sessionId, timestamp, type}` плюс `content` у `enqueue` и иногда `reason`
(`operation` ∈ enqueue/dequeue/remove); `last-prompt` — `{lastPrompt, leafUuid, sessionId, type}`
(парсер понимает ещё необязательные `explicit`, `rewound`); `bridge-session` — `{bridgeSessionId,
lastSequenceNum, ownerAccountUuid, ownerOrganizationUuid, sessionId, type}`;
`custom-title`/`ai-title`/`atis-latch`/`agent-name`/`mode` — одно значение (`customTitle`,
`aiTitle`, `atis`, `agentName`, `mode`) плюс `sessionId`, `type`, без `timestamp`. VibeShot
`7bad7d59…` (2.1.219): 515 из 2257 (22,8 %) — `last-prompt` 154, `custom-title` 154, `ai-title`
152, `queue-operation` 44, `mode` 11; `bridge-session`, `atis-latch`, `agent-name` там нет вовсе —
набор типов растёт с версией. Парсер 2.1.255 собирает их в отдельные карты
(`summaries, customTitles, aiTitles, tags, relocatedCwds, atisLatches, bridgeSessionIds,
bridgeLastSeqs, leafUuids, …`), т.е. это состояние, сворачиваемое при чтении, а не сообщения;
правила слияния двух файлов у него нет. `bridge-session` — запись переподключения Remote Control:
по документации `--resume` переподключает мост, записанный в разговоре, а защита от тихого отбора
описана только для той же машины — на другой машине ожидаемо попытается отобрать (не прогонялось).
Скилл `sync-repo` такие строки явно не покрывал (сравнивал только множества `uuid`).

Строки `type:"relocated"` (`{type, sessionId, relocatedCwd}`, без `uuid`; пишутся при смене cwd
сессии) и `type:"summary"` (`{type, leafUuid, summary}`, без `uuid`; парсер 2.1.255 читает их в
карту `summaries` по `leafUuid`, писателя в strings 2.1.255 не найдено) видны только в strings
бинаря — в проверенных транскриптах не встретились. Строка `uuid: summary-<startUuid>` в бинаре —
элемент UI-списка (свёрнутая группа tool-вызовов с `searchCount/readCount/replCount`), не запись
транскрипта.

## Цепочка `parentUuid` и выбор листа при resume

Каждая запись с `uuid` несёт `parentUuid` (`null` у корня), `sessionId`, `timestamp`, `cwd`,
`version`, `gitBranch`, `isSidechain` (проверено на всех 366 + 1742 uuid-записях двух файлов —
пропусков 0; типы с `uuid`: `user`, `assistant`, `attachment`, `system`). Resume в 2.1.255
(`fAr`): `DX` возвращает `messages`, `leafUuids` и `endedSessions`; перебираются листья с
`isSidechain=false`, берётся запись с максимальным `new Date(timestamp).getTime()`, от неё `Pwe`
восстанавливает цепочку по `parentUuid` вверх (с детекцией цикла). Листьев в живом
VibeSweep-файле 10 (при 563 строках), в завершённом VibeShot — 1. Следствие для слияния: после union двух
ветвей (машина A и машина B продолжили один `sessionId`) файл содержит обе, а разговор — одну,
ту, чей лист моложе по часам своей машины. Вторая ветвь доступна только через `/rewind` или
fork; для пользователя это неотличимо от потери, и `additionalContext` обязан это называть.

Кросс-проектный `--resume <id>` срабатывает, только если ровно один другой каталог содержит
транскрипт с этим id («a hand-copied duplicate makes Claude Code report not-found»,
[sessions](https://code.claude.com/docs/en/sessions#resume-a-session)); сам кросс-проектный
поиск появился в 2.1.223 (до него поиск ограничивался текущим проектом и его worktree); до 2.1.251
перенос сессии на существующий одноимённый транскрипт молча его перезаписывал.

## Немонотонные `timestamp` и осиротевшие `parentUuid` есть в чистых одномашинных файлах

VibeSweep `e3bbbf16…`: 13 немонотонных пар из 304 (при 423 строках), 17 из 395 (при 563)
по всем записям с `timestamp`; 1 осиротевший `parentUuid` — строка 148, `user`-запись с
`tool_result`, родитель `9f348cb4…` не существует как `uuid` ни в основном файле, ни в
сайдкарах (совпадения только текстовые внутри содержимого). VibeShot `7bad7d59…`: 75 пар из
1785, сирот 0. Кто ломает порядок: `queue-operation(dequeue) → system(stop_hook_summary)` (11 пар
в VibeSweep, 14 в VibeShot: `dequeue` несёт время снятия с очереди, а следующая `system`-запись —
более раннее время события, разрыв от 10 с до 64 мин), `user → attachment` (4 и 40 пар —
attachment вроде `hook_success`/`total_tokens_reminder` обычно на 1 мс раньше своего `user`),
`user(tool_result) → user` (16 пар в VibeShot и 1 в VibeSweep, 1–18 мс),
`queue-operation(remove) → attachment` (5 в VibeShot), `queue-operation(dequeue) → user` (1 в
VibeSweep) — суммы сходятся: 11+4+1+1 = 17 и 14+40+16+5 = 75. Часы одной машины, один процесс,
никакого слияния.
Приёмочный гейт «timestamp монотонны, нет сирот» отклонит почти каждое слияние — проверять
можно только парсимость и покрытие всех `uuid` всех источников.

## Сайдкары `<sessionId>/`

Рядом с файлом — каталог с тем же именем: `tool-results/<shortid>.txt` (крупные выводы
инструментов; у текущей сессии 36 файлов при перепроверке — растёт по ходу сессии, имена вида
`b1f1vysjx.txt`),
`tool-results/pdf-<uuid>/page-N.jpg`, `subagents/workflows/wf_*/agent-<id>.jsonl` +
`agent-<id>.meta.json` (транскрипты сабагентов: `isSidechain: true`, `agentId`, `sessionId`
родителя — 35 файлов в каталоге одного workflow вместе с журналом),
`subagents/workflows/wf_*/journal.jsonl` (34 строки,
типы `started`/`result` по 17, ключи `agentId, key, type` плюс `result` у `result` — ни одной
записи с `uuid`, только байтовое сравнение), `workflows/wf_*.json` + `workflows/scripts/*.js`,
`custom-title.json` (`{"customTitle": …}`), `auto-mode-classifier-error.txt`. По стору `-ALL-`:
352 `<uuid>.jsonl` плюс одна конфликт-копия OneDrive `VibeIDE/78dcced4-…-MacMini.jsonl` (итого
353), 100 `journal.jsonl`, 9 `custom-title.json`, 12 `MEMORY.md` (при перепроверке 12:50 — 101 и
13: стор живой).

Размеры — величины разные: логический размер стора 1,64 ГБ (3783 файла; VibeIDE 637 МБ);
занято на диске 493 МБ на момент research (VibeIDE 223 МБ) и 957 МБ при перепроверке — разница от
гидрации OneDrive, 67 из 353 транскриптов по-прежнему `blocks=0`; самый большой транскрипт
26,7 МБ (`VibeIDE/2ec0007b-…`), т.е. транскрипты ≤27 МБ; один tool-result ровно 67 108 864 байта
= 64 MiB
(`VibeIDE/b34f247f-…/tool-results/bkeev8cqs.txt`) — производный, для resume не нужен, в
репозиторий не коммитится.

## Что из этого следует для слияния

- Ключ записи — `uuid`; строки без `uuid` — точная строка (byte-mode), дубли по ней;
  `journal.jsonl` целиком в byte-mode.
- Порядок: строки базы как есть, недостающие вставляются по `timestamp` со стабильным
  тай-брейком по порядку источника; `timestamp` не есть причинный порядок (часы двух машин),
  причинность — только `parentUuid`.
- Гейт приёмки: JSON парсится, все `uuid` всех источников присутствуют. Не требовать
  монотонности и отсутствия сирот — их даёт сам CLI.
- Снимок живого файла — до последнего `\n`; обрывок строки не должен попасть в union как
  «уникальная безuuid-строка».
- После слияния вилки честно сообщать: видима одна ветвь (лист с max `timestamp`), вторая — в
  файле.
- Результат — обычный файл на уровне `projects/<enc>/`; ссылки только на уровне каталога.
- Фикстуры реального формата на каждую версию CLI: `queue-operation`, `bridge-session`,
  `atis-latch`, безuuid-строки, обрывок строки, сирота `parentUuid`, немонотонная пара.
