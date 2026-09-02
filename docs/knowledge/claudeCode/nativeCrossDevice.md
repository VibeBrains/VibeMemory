# Нативные кросс-девайс возможности Claude Code и Claude Desktop (2026-09-02)

Проверено при проектировании ClaudeSync — переноса сессий Mac ↔ Windows — чтобы не строить
своё там, где Anthropic уже дала готовое. Версии: терминальный CLI 2.1.232 (`claude --version`),
Claude Desktop 1.40609.1 со встроенным CLI 2.1.255 (`~/.claude/sessions/6278.json`, entrypoint
`claude-desktop`), upstream CHANGELOG прочитан до 2.1.258. Источники: официальные доки
code.claude.com (remote-control, sessions, desktop, claude-code-on-the-web, memory, env-vars,
cross-session-messaging, claude-directory), CHANGELOG, GitHub API по issues anthropics/claude-code,
строки бинарей CLI 2.1.232/2.1.255 и app.asar Desktop, файлы этого Mac под `~/.claude` (только
чтение). Что не подтверждено документом или прогоном — помечено «не проверено».

Итог, ради которого всё читалось: **нативного переноса локальной сессии на другую машину нет** —
ни в CLI, ни в Desktop. Официальных механизмов три, и каждый переносит не то.

## Remote Control — окно в живой процесс, а не перенос

Док: «Remote Control connects claude.ai/code or the Claude app … to a Claude Code session running
on your machine» и прямо — «The web and mobile interfaces are a window into that local session»
(https://code.claude.com/docs/en/remote-control). Процесс остаётся на машине A; с машины B сессия
видна только через браузер claude.ai/code или мобильное приложение.

- **«Local process must keep running»** — раздел Limitations: закрыли терминал, вышли из VS Code
  или убили `claude` — сессия offline: «Unless Claude is in the middle of a task, claude.ai and the
  Claude app show the session as offline within seconds after the process exits». CHANGELOG
  2.1.236: «Remote Control now marks a session offline within seconds when the CLI exits or its
  terminal closes».
- **Где живёт транскрипт.** «While Remote Control is connected, the session transcript … is stored
  on Anthropic servers. The stored transcript keeps the conversation in sync across your devices
  and lets the session reconnect after a network drop. Execution and filesystem access stay on
  your machine». Синхронизируются сообщения, прогресс сабагентов и workflow, permission-промпты,
  модель/effort, переименования. Auto memory не передаётся (док memory: «not shared across
  machines or cloud environments»); `tasks/` и `todos/` док RC не упоминает вовсе — в перечне
  синхронизируемого их нет, вживую не проверялось.
- **Возобновление после остановки** описано только «в той же директории»: `claude remote-control`
  / `--continue` / `--session-id <id>`, окно «about four hours after the server stopped»;
  интерактивная сессия переподключается через «reconnection record», записанный в разговоре
  (CHANGELOG 2.1.229 «Documented claude remote-control --continue»). Про другой хост — ни слова.
- **Серверная копия наполняется из локальной истории**, а не наоборот: CHANGELOG 2.1.224 «Fixed a
  Remote Control session recreated after its server session expired uploading prior local
  conversation history into the new session»; 2.1.232 «Fixed Remote Control bridge sessions not
  restoring conversation history when the session worker restarts».
- **Между своими машинами ходят только сообщения**, и только когда обе стороны подключены к
  Remote Control: таблица доставки cross-session-messaging — «On another of your machines |
  Through Anthropic servers, arriving over that machine's Remote Control connection»
  (https://code.claude.com/docs/en/cross-session-messaging#message-sessions-on-other-machines);
  с 2.1.225 `SendMessage` умеет начинать разговор с RC-сессией на другой машине по имени;
  2.1.243 добавил notice «sessions on other machines can't be seen from, or reach, this one».
- **Desktop включает мост своим переключателем** («Desktop app: Settings > Claude Code > Enable
  remote control by default»): текущая Desktop-сессия подключена к bridge, хотя
  `remoteControlAtStartup` отсутствует и в `~/.claude/settings.json` (симлинк на копию в
  OneDrive), и в `.claude.json` (перепроверено при верификации); состояние переключателя в GUI не
  проверялось.

## Reconnection record лежит в транскрипте и в `.claude.json`

Запись о привязке к Remote Control — часть самих файлов сессии, поэтому она уезжает вместе с
любым синком транскриптов.

- В текущем транскрипте `~/.claude/projects/-Volumes-Storage-Projects-VibeCode-VibeSweep/
  e3bbbf16-….jsonl` — 26 записей `"type":"bridge-session"` с одним `bridgeSessionId`
  `cse_01Xk…` на момент верификации (утром при исследовании 19, при написании 25 — сессия
  продолжалась). Поля записи:
  `bridgeSessionId, lastSequenceNum, ownerAccountUuid, ownerOrganizationUuid, sessionId, type`.
- `~/.claude/.claude.json` → `replBridgePlaceholders`: словарь по тому же `cse_…` (на момент
  верификации три ключа — по одному на каждую живую Desktop-сессию из `sessions/*.json`:
  VibeSweep, Promed, VibeMemory; при написании было два), значение `{createdAt, pid, procStart}` —
  pid процесса этой машины.
- `~/.claude/sessions/6278.json` → `bridgeSessionId: session_01X…` — тот же id в форме
  `session_`; док web: «pass the bare ID, such as session_... or cse_...».
- Док RC: «When you resume a conversation with claude --resume or claude --continue, Claude Code
  reconnects to the Remote Control session recorded in that conversation»; troubleshooting:
  «Another device or Claude Code session took the session over: run /remote-control only if you
  want to take it back». Защита от тихого отбора задокументирована только для одной машины —
  CHANGELOG 2.1.232: «resuming a conversation no longer silently takes Remote Control away from
  another Claude Code on the same machine that still has it».
- Следствие (вывод из доков, на второй машине не прогонялось): транскрипт, доехавший на машину B
  синком, при `--resume` там попытается переподключить/отобрать RC-сессию машины A. При
  закрытой сессии на A это желаемое поведение; при живой вкладке Desktop на A судьи workflow
  предсказывают два живых CLI на один sessionId — это их вывод, не измерение.

## Облако: «Continue in Web» и `--teleport` — односторонне и из сводки

Единственный официальный путь A → B с историей идёт через облако, и он lossy.

- Desktop, меню **Continue in** → «Claude Code on the Web: sends your local session to continue
  running remotely. Desktop pushes your branch, generates a summary of the conversation, and
  creates a new cloud session with the full context … This requires a clean working tree, and is
  not available for SSH sessions» (https://code.claude.com/docs/en/desktop#continue-in-another-surface).
  Док в одном предложении говорит и «summary», и «full context»; по описанному механизму это
  сводка и новая сессия, а не транскрипт.
- CLI пушить не умеет вообще: «From the CLI, session handoff is one-way: you can pull cloud
  sessions into your terminal with --teleport, but you can't push an existing terminal session to
  the web» (https://code.claude.com/docs/en/claude-code-on-the-web#move-tasks-between-web-and-terminal).
- На машине B `claude --teleport <id>`: «Claude verifies you're in the correct repository, fetches
  and checks out the branch from the cloud session, and loads the full conversation history into
  your terminal. The terminal gets its own copy of the session: new work there stays local».
  Требования: чистый git, тот же репозиторий, ветка запушена, тот же аккаунт. «--teleport is
  distinct from --resume. --resume … doesn't list cloud sessions». CHANGELOG 2.1.243: teleport
  предлагает stash вместо выхода на uncommitted changes.
- В существующую облачную сессию можно писать с любой машины: `claude -p "msg" --cloud <id>` —
  «sends no local session state, so the command doesn't need to run from the machine that
  started the session».

## `/desktop` — та же машина; Desktop и CLI ведут раздельные истории

- Док desktop: «Each maintains separate session history, but they share configuration and project
  memory via CLAUDE.md files. To move a CLI session into Desktop, run /desktop in the terminal.
  Claude saves your session and opens it in the desktop app, then exits the CLI» — доступно на
  macOS и x64 Windows с подпиской, не с API-ключом/Bedrock/Agent Platform/Foundry
  (https://code.claude.com/docs/en/desktop#coming-from-the-cli). Sessions-док: «The desktop app,
  Claude Code on the web, and the VS Code extension each maintain their own session history».
- Desktop-инструменты «работа между сессиями» видят только сессии самого Desktop: «Claude doesn't
  see cloud sessions, or sessions you started from the terminal CLI or the VS Code extension»
  (https://code.claude.com/docs/en/desktop#work-across-sessions).
- Как Desktop находит транскрипт (app.asar + строки CLI): дескриптор
  `~/Library/Application Support/Claude/claude-code-sessions/<accountUuid>/<orgUuid>/local_<uuid>.json`
  хранит `cliSessionId` и **абсолютные** `cwd`/`originCwd`; приложение спавнит встроенный CLI с
  процессным cwd = `descriptor.cwd` и `--resume=<cliSessionId>`, CLI ищет
  `<CLAUDE_CONFIG_DIR>/projects/<enc(cwd)>/<cliSessionId>.jsonl`. Трансляции путей нет: 23
  Windows-дескриптора с `D:\…` на Mac грузятся, а resume по коду бандла упирается в
  `prepareSpawnCwd` (`access(cwd)` → ENOENT → «Working directory no longer exists: D:\…»; вживую
  resume Windows-дескриптора не запускался). Единственный штатный способ переселить сессию —
  `change_directory` (CLI пишет запись `{type:"relocated", relocatedCwd}` и переносит транскрипт).
- Побочный эффект чужих дескрипторов без транскрипта: Desktop сам ставит `transcriptUnavailable`
  и при `cli_resume_not_found_*` навсегда стирает `cliSessionId` (`clearStaleResumeHandle`);
  в сторе уже 20 помеченных и 16 без `cliSessionId`.
- `/desktop` первой командой в сессии даёт «CLI session transcript not found»
  (https://github.com/anthropics/claude-code/issues/62017; данные issue — по API при
  исследовании, локально не кэширован): транскрипт рождается только при
  постановке первого промпта в очередь — проверено на 2.1.232 в `-p`-режиме, SessionStart →
  первая запись `queue-operation enqueue` через ≈4 с; интерактивный режим отдельно не прогонялся.

## Экспорт/импорт сессий в Desktop — есть в коде, за фиче-флагом

В бандле Desktop (index.chunk-jfL6G7d3.js) — подсистема: экспорт в zip с манифестом
`claude-3p-export.json` (kind `desktop3p`), источники импорта `localZip | local1P | local3PBundle
| terminalCli | remotePull`, staging-папка `imported-staging`, классификатор папки (`userData` при
наличии `claude-code-sessions`/`local-agent-mode-sessions`, `projects` при наличии `projects/`);
источник `terminalCli` адоптирует транскрипты из `projects/`, перемаппивая home-относительные
пути на текущий homedir. Тексты флагов: «Lets users export this computer's chats, Cowork tasks,
and Code sessions as a zip another install can import. No effect unless `enabled` is true» и
«Lets users import Claude.ai chats and projects, plus earlier Claude sessions on this computer,
when `enabled` is true». Это единственный найденный в коде официальный механизм «сессии на другой
машине». Включён ли флаг для аккаунта — **не проверено**: флаги лежат в бинарном fcache, а
UI списка сессий — веб-код claude.ai вне app.asar, по строкам его не проверить.

## Auto memory — machine-local; что вообще восстанавливает resume

- Док memory: «Auto memory is machine-local. … Files are not shared across machines or cloud
  environments» (https://code.claude.com/docs/en/memory#storage-location). Лежит в
  `projects/<name>/memory/`, из retention-свипа исключена (док memory), через Remote Control не
  передаётся (док RC её не упоминает — вывод из memory-дока).
- Локальный `--resume` восстанавливает историю, модель, агента, permission mode, активный
  `/goal`, неистёкшие scheduled tasks; **не** восстанавливает фоновые Bash/monitor-задачи и флаги
  `--mcp-config/--settings/--plugin-dir/--fallback-model/--add-dir`
  (https://code.claude.com/docs/en/sessions#what-a-resumed-session-restores). Значит даже полный
  перенос транскрипта переносит не всё состояние сессии.

## `CLAUDE_CODE_PROJECT_DIR_NAME` — одно имя на запуск, не per-repo

CHANGELOG 2.1.234: «Added the optional CLAUDE_CODE_PROJECT_DIR_NAME environment variable: hosts
that give each session its own config directory can choose a short name for the per-project
transcript directory». Док sessions «Name the project directory yourself»
(https://code.claude.com/docs/en/sessions#name-the-project-directory-yourself):

- Задаёт имя `projects/<name>/` для транскриптов **и** auto memory независимо от cwd; требует
  v2.1.234+ — терминальный CLI владельца 2.1.232 ниже порога.
- Работает только вместе с `CLAUDE_CONFIG_DIR`: «under the default ~/.claude it would merge every
  project's transcripts and auto memory into one directory».
- Читается один раз из окружения, запускающего `claude`: «an env block in a settings file can't
  set it» — на каждый репозиторий нужна своя обёртка запуска.
- Значение: «Use 1-64 letters, digits, hyphens, or underscores»; иное игнорируется.
- `claude --resume <session-id>` находит сессию под любым из имён; `Ctrl+A` показывает все.
- К Desktop применимость **не проверена**: Desktop «reads your shell profile … to extract PATH and
  a fixed set of Claude Code variables» и не меняет переменную per-project
  (https://code.claude.com/docs/en/desktop#local-sessions).

## `CLAUDE_CONFIG_DIR` и синк — что сказано официально

- env-vars: «Override the configuration directory (default: ~/.claude). All settings, session
  history, and plugins are stored under this path … Useful for running multiple accounts side by
  side … Set it in your shell, user settings, or managed settings. Ignored in project and local
  settings» (https://code.claude.com/docs/en/env-vars; запрет из project settings — CHANGELOG
  2.1.251). Учётные данные привязаны к каталогу: macOS Keychain-запись ключуется по нему,
  на Linux/Windows `.credentials.json` лежит под ним
  (https://code.claude.com/docs/en/authentication#credential-management).
- **Ни одной строки о синхронизации каталога между машинами** ни в одном доке; наоборот —
  «Show sessions from all projects on this machine», «every other project on this machine»,
  «machine-local». Troubleshooting и debug-your-config: 0 упоминаний OneDrive/Dropbox/iCloud.
- Облачные папки в CHANGELOG — только как баги: 2.1.181 «0-byte or truncated files on network
  drives and cloud-synced folders», 2.1.163 EEXIST на `session-env` внутри OneDrive, 2.1.72
  EEXIST плагинов в OneDrive, 2.1.7 ложное «file modified» от cloud sync tools.
- Anthropic сама зашила список машинно-локального в бинарь 2.1.232 (исключения при снапшоте
  host-конфига для self-hosted runner, `rOE`): `.claude.json`, `.claude.json.backup`,
  `.credentials.json`, `projects`, `sessions`, `todos`, `shell-snapshots`, `statsig`,
  `file-history`, `history.jsonl`, `ide`, `logs`, `backups`, `.session_ingress_token`.
  Официального документа «portable vs machine-local» нет — его просит #81392 (open, 2026-07-26).
- Не все компоненты уважают переменную: VS Code extension host игнорирует `CLAUDE_CONFIG_DIR`
  (https://github.com/anthropics/claude-code/issues/30538, open с 2026-03-03).
- Дубли транскриптов официально признаны проблемой: кросс-проектный `--resume <id>` «resolves the
  ID only when exactly one other project holds a transcript … a hand-copied duplicate makes Claude
  Code report not-found»; CHANGELOG 2.1.251 «Fixed session transcripts being silently overwritten
  when a directory change relocated a session onto an existing same-ID transcript». Формат JSONL —
  «internal to Claude Code and changes between versions, so scripts that parse these files
  directly can break on any release» (https://code.claude.com/docs/en/sessions).

## GitHub issues без ответа Anthropic

- https://github.com/anthropics/claude-code/issues/22648 «[Feature Request] Account-level settings
  sync across devices» — open с февраля 2026, 47 реакций, 25 комментариев, 0 от
  MEMBER/COLLABORATOR; #64081 закрыт ботом как дубль #63242, #63242 — как дубль #38970; #38970
  открыт (бот лишь указал на #22648).
- https://github.com/anthropics/claude-code/issues/28791 «Sync conversation history between CLI
  and Claude Code desktop app» — open, 30 комментариев, 0 официальных; #56038 закрыт как дубль.
- https://github.com/anthropics/claude-code/issues/73639 «Transfer session to another claude code
  instance» — open, labels enhancement/area:cli/stale, 0 официальных; в треде только сторонние
  инструменты (ctxhop, claude-sync). Данные по API на момент исследования; локально JSON не
  кэширован.
- Через аккаунт claude.ai синкаются только skills/plugins/connectors для Cowork-таба Desktop:
  «syncs through your claude.ai account, not from the CLI's ~/.claude directory»
  (https://code.claude.com/docs/en/desktop).
- CHANGELOG 2.1.233 → 2.1.258 (670 строк) — ни одной записи о «resume on another machine/device»
  кроме notice 2.1.243.

## Сводка

| Механизм | Что переносится | Где живёт состояние | Другая машина |
|---|---|---|---|
| Remote Control | окно в живой процесс | процесс на A; транскрипт на серверах Anthropic только пока соединение живо | только просмотр/управление через claude.ai или мобильное приложение |
| Continue in Web → `--teleport` | сводка → новая облачная сессия → копия в терминале B | облако | да, но не транскрипт A |
| `/desktop` | CLI-сессия в Desktop | та же машина | нет |
| Экспорт/импорт Desktop | zip с сессиями | код за фиче-флагом | не проверено — состояние флага для аккаунта неизвестно |
| `CLAUDE_CODE_PROJECT_DIR_NAME` | имя каталога `projects/` | `CLAUDE_CONFIG_DIR` | одно имя на запуск; Desktop — не проверено |
| Auto memory | — | `projects/<name>/memory/`, machine-local | нет |

Перенос сессии между машинами остаётся файловой задачей поверх внутренних форматов — с двумя
предупреждениями из этого документа: транскрипт несёт reconnection record Remote Control, а
дубль id в двух каталогах `projects/` ломает `--resume` и портит дескрипторы Desktop.
