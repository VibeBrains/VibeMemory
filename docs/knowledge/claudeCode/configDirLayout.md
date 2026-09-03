# Раскладка `~/.claude`: что горячее, что общее, что секрет (2026-09-02)

Проверено на этом Mac read-only (`ls`/`stat`/`python`/`security`/`strings` бинаря) поверх отчётов
четырёх исследователей (native/community/layout/desktop) и трёх судей схемы ClaudeSync.
Конфигурация: `CLAUDE_CONFIG_DIR=/Users/borodatych/.claude` (`~/.zshrc:8`, `~/.profile:1`,
`launchctl getenv`); в каталог пишут два клиента — терминальный CLI 2.1.232 (`~/.local/bin/claude`,
последний запуск 14.08) и Claude Desktop 1.40609.1 со встроенным CLI 2.1.255 (`sessions/6278.json`,
`entrypoint=claude-desktop`). Четыре класса: **горячее per-machine** (переписывается постоянно,
привязано к машине или секрет), **долговечное общее** (append-only или редкий пользовательский
контент), **ключуемое по абсолютному пути**, **lock/runtime**. «Непроверено» стоит там, где у
исследователей было [unverified]/[likely].

## Инвентарь

| Запись | Класс | Кто и когда пишет | Per-session ключ | Делить между машинами |
|---|---|---|---|---|
| `.claude.json` (57 048 B на 12:38, 57 528 B на 12:45; 51 ключ, 26 проектов) | горячее | оба клиента, файл целиком tmp+rename; бэкапы 10:22, 10:24, 10:41, 10:48, 10:49 (research), к 12:45 ротация уже 12:40:40…12:45:17 — 5 за 5 минут при трёх живых Desktop-сессиях, т.е. не реже раза в минуту при активности (в idle не измерено) | нет | вредно: конфликт-копии, чужой pid в `replBridgePlaceholders` |
| `.claude.json.backup`, `.claude.json.backup.<ms>`×5 в корне (14.08, 49 340 B, за 15 с при апдейте 2.1.217→2.1.232), `backups/`×5 (новое место) | горячее, производное | CLI: ≤5 штук, не чаще 1/60 с (`Med=60000,Led=5`); `backups/` подметает cleanupPeriodDays | нет | шум |
| `.credentials.json` | секрет | macOS — файла нет (Keychain); Windows/Linux — plaintext | нет | никогда |
| `mcp-needs-auth-cache.json` (518 B) | горячее | каждый старт/подключение MCP (mtime 12:39 сегодня = старт pid 17546) | нет | вредно: в OneDrive три копии по 610 B |
| `policy-limits.json` (214 B, 0600), `remote-settings.json` (2 B `{}`) | горячее, кеш серверных политик | старт; по докам удаляются при logout | нет | бессмысленно |
| `stats-cache.json` (1 231 B, 14.08, `totalSessions=14`) | горячее, производное | терминальный CLI для `/stats`; Desktop не обновляет | нет | задвоение при слиянии |
| `.last-cleanup` (24 B) | горячее | маркер retention-свипа: сегодня 10:59:48 и 11:33:20 | нет | вредно: две GPD-копии в OneDrive |
| `.last-update-result.json` (161 B) | горячее | автообновление 14.08 (`path=native`, `success`) | нет | нет |
| `cache/` (504 KB: `changelog.md` 510 641 B, `my-closed-issues.json`) | горячее, сетевой кеш | фоновая докачка (док: «Refreshed in the background») | нет | нет |
| `debug/` (108 KB, 9 `<sid>.txt` + symlink `latest` на абсолютный путь) | горячее | только при `/debug`; внутри локальные пути и применённые permission-правила | да | вредно: symlink в OneDrive станет текстом; путь лога переопределяет документированная `CLAUDE_CODE_DEBUG_LOGS_DIR` (путь файла, не каталога) |
| `telemetry/` (16 MB, 40 файлов `1p_failed_events.<sid>.<batch>.json`, 5 649 событий) | горячее спул | когда аналитический эндпоинт отверг батч; досылается позже; `platform darwin`, `node v26.3.0` запечены; путей 0 | да | вредно: churn, досылка с чужой машины |
| `shell-snapshots/` (`snapshot-zsh-<ms>-<rand>.sh`, 5 001 B, 3 шт.) | горячее | старт сессии; подключается к каждому Bash; по докам снимается при чистом выходе («Removed on clean exit»), но снимок 16.08 без живой сессии лежит до сих пор — выход Desktop-сессий чистым, видимо, не считается (причина не проверена) | нет | бессмысленно: `/Users/borodatych/.local/bin/claude` внутри, zsh |
| `plugins/` (6,4 MB, GCS-снимок official marketplace без `.git`; `installed_plugins.json` нет) | горячее + path-keyed (`installLocation`) | автопереустановка (`officialMarketplaceAutoInstalled=true`); активные плагины — `@inline` через claude.ai | нет | вредно: гонка записи `known_marketplaces.json` (changelog); корень `plugins/` переопределяет документированная `CLAUDE_CODE_PLUGIN_CACHE_DIR` |
| `downloads/` (пусто), `todos/` (4×`[]`, legacy) | горячее | не пишутся | да | нет |
| `projects/<enc(cwd)>/` | path-keyed имя | CLI при первом промпте (не при SessionStart) | — | имя — нет, содержимое — да |
| `projects/…/<sid>.jsonl`, `<sid>/{tool-results,subagents,workflows,custom-title.json}` | общее, дозапись + редкие удаления записей самим CLI | оба клиента, open/append/close (`lsof` живых pid: 0 открытых `.jsonl`) | да, uuid | максимальная: это и есть `--resume`; вред только живому файлу (прецедент 08.08) |
| `projects/…/memory/` (`MEMORY.md`, темы, `sessions/*.md`) | общее | модель в каждой сессии | нет | ценно; официально «machine-local»; в VibeIDE уже `MEMORY-GPD-WIN-MAX2.md` |
| `tasks/<sid>/N.json` (584 KB, 11 сессий, 1…22 файла) | общее | CLI при TaskList; подметает cleanupPeriodDays | да | умеренно: нужен `--resume` той сессии |
| `tasks/<sid>/.lock` (0 B), `history.jsonl.lock` (mkdir-лок, stale 10 с) | lock | advisory | да / нет | нет |
| `history.jsonl` (174 B, 1 строка) | общее, path-keyed построчно | только терминальный CLI при отправке промпта; Desktop не пишет; `CLAUDE_CODE_SKIP_PROMPT_HISTORY=1` глушит вместе с транскриптами | нет | низкая ценность (стрелка вверх), гонки записи |
| `sessions/<pid>.json` + `<pid>.<sha256>.key` (108 B, 0600) | runtime + секрет | CLI на старте, снимает на выходе; чужой pid на другой машине чистится как «crash leftover» | pid | никогда |
| `session-env/<sid>/` (57 пустых, 12 KB) | runtime | старт сессии; `CLAUDE_ENV_FILE=…/session-env/<sid>/sessionstart-hook-0.sh` | да | вредно: пустые папки не переживают синк, EEXIST в OneDrive (2.1.163) |
| `ide/*.lock` | runtime | только в OneDrive (5, 30.06–01.07), мёртвые | pid | нет |
| `CLAUDE.md` (26 031 B), `settings.json` (254 B), `skills/` (100 KB) | общее пользовательское | владелец; CLI сам дописывает allow-правила («Adding 4 allow rule(s) to destination 'userSettings'», debug-лог 14.08) | нет | да — уже симлинки в OneDrive |

## `.claude.json`: 57 KB кешей и ноль переносимого

По весу (снимок 12:45): `cachedGrowthBookFeatures` 31 220 B (577 флагов), `projects` 9 266 B (8 986 B
в research с 25 проектами), `clientDataCacheSlots` 3 037 B, `cachedExperimentData` 971 B плюс десяток `*Cache`-ключей;
`oauthAccount` 758 B — профиль (email, accountUuid, orgUuid, billingType, displayName), не токен;
`replBridgePlaceholders` — по записи `cse_… → {createdAt, pid, procStart}` на каждую живую Remote-Control-сессию (350 B, 4 записи на 12:48); onboarding-флаги,
`skillUsage`/`pluginUsage`, `migrationVersion=14`, `installMethod=native`, `autoUpdates=false`.

Пользовательских полей восемь: семь в `projects[*]` — `{allowedTools, mcpServers, enabledMcpjsonServers,
disabledMcpjsonServers, mcpContextUris, hasTrustDialogAccepted, hasClaudeMdExternalIncludesApproved}` —
и top-level `mcpServers`. Во всех трёх копиях (`~/.claude/.claude.json`, `~/.claude.json`,
`~/OneDrive/.claude/.claude.json`) они пусты: `allowedTools=[]`, `mcpServers`/`*McpjsonServers`/`mcpContextUris` пусты во всех записях `projects` (26, 4 и 13), top-level
`mcpServers` нигде не задан. Разрешения — 4 правила `permissions.allow` в `settings.json`, он общий.
Остальные ключи проекта — статистика последней сессии (`lastCost`, `lastDuration`, `lastSessionId`,
`lastSessionMetrics`, `exampleFiles`, `lastGracefulShutdown`, `lastVersionBase`). Ключи `projects` —
абсолютные пути: на Mac и Windows разные, а в одном файле три написания одного Windows-пути
(`D:\Projects\…`, `D:/Projects/…`, `C:\Users\borodatych\Projects\…`). Док claude-directory: «App
state, OAuth, UI toggles, personal MCP servers»; «Don't delete ~/.claude.json, ~/.claude/settings.json,
or ~/.claude/plugins/». Путь файла — `ney()`: `$CLAUDE_CONFIG_DIR/.claude<suffix>.json`, а при
наличии `.config.json` в конфиг-дире приоритет у него (недокументировано).

**machineID приезжает с файлом.** `machineID 2799b878…`, `userID 9fb047c1…`, `firstStartTime
2026-07-21T06:09:11.828Z` одинаковы в четырёх копиях: локальной, OneDrive (Windows-живой, 16.08),
`.claude-GPD-WIN-MAX2-2.json`, `.claude.json-GPD-WIN-MAX2.backup`. «Смена machineID 21.07» — не
привязка к железу, а пересоздание файла с нуля. Телеметрия использует `device_id` с префиксом userID,
не machineID. Второй конфиг на этой же машине — `~/.claude.json` (42 078 B, 26 ключей, 4 проекта,
machineID `33695401…`, userID `d80f9b90…`, firstStartTime 2026-07-01T21:56, mtime 30.08 14:08):
его ведёт какой-то запуск без переменной; кто — не установлено (кандидат — VS Code extension host,
который `CLAUDE_CONFIG_DIR` игнорирует, #30538; непроверено).

## Почему seedTrust в `.claude.json` писать нельзя

- История порчи: [#28847](https://github.com/anthropics/claude-code/issues/28847) (февраль 2026,
  мейнтейнер stevenpetryk: «escalated to a hotfix», 2.1.59, причина — фича, генерившая много записей
  конфига); [#29153](https://github.com/anthropics/claude-code/issues/29153) (домашний каталог в
  OneDrive: каскад 35 KB → 9 KB → 1,5 KB → 77 B, 106 файлов в `backups/`); changelog 2.1.234
  «Windows: startup no longer stalls on repeated rename retries when ~/.claude.json is read-only» —
  OneDrive ставит +R ([#50886](https://github.com/anthropics/claude-code/issues/50886): `attrib` →
  `A R P`). Пересоздание Windows-конфига 21.07 этим объясняется правдоподобно, прямого лога нет
  (непроверено).
- Механика записи — не инкремент, а замена целиком: lock → tmp `.tmp.<pid>.<hex>` с O_EXCL →
  fsync → rename. Строки бинаря 2.1.232: «Lock acquisition took longer than expected - another Claude
  instance may be running», «saveConfigWithLock: re-read hit a parse error; auto-repairing from cached
  config under lock. See GH #3117», `tengu_config_stale_write`. Лок замечает другой Claude, а не
  внешний писатель, и при ошибке парсинга CLI перезапишет файл своей кешированной копией. Desktop-CLI
  переписывает файл не реже раза в несколько минут (≥5 за 27 минут в research, 5 за 5 минут в
  12:40–12:45 — бэкапы выше).
- Ценность нулевая. Desktop-сессии работают при `hasTrustDialogAccepted=false`: живой pid 11657 в
  `/Volumes/Storage/Projects/VibeCode/Promed`, у которого в `projects` стоит `false` (false у 8 из 26
  записей, включая 7 из 9 `/Volumes/Storage/*`; true — у VibeSweep, VibeMemory, всех старых `/Users/…` и двух из трёх написаний Windows-пути). Флаг
  ставит нативный диалог доверия — терминального CLI или Desktop (workspace trust → `saveWorkspaceTrust`;
  VibeMemory получил `true` от Desktop-сессии pid 17546 в 12:39) — по одному на путь; ключ — абсолютный путь,
  посев с Mac для Windows-путей ничего не даёт. Судьи сошлись: единственный компонент, способный
  сломать вход в Claude Code на машине, ради поля, которое пусто — убирать, не гейтить.

## Keychain ключуется sha256 пути `CLAUDE_CONFIG_DIR`

`DZ()` в 2.1.232: имя записи = `Claude Code<OAUTH_FILE_SUFFIX>-credentials` + суффикс
`-<sha256(dir)[:8]>`, где dir = `CLAUDE_SECURESTORAGE_CONFIG_DIR` (NFC) либо `xn()` (=
`CLAUDE_CONFIG_DIR || ~/.claude`); суффикс опускается только когда переменная не задана
(`r = t!==void 0 ? !t : !process.env.CLAUDE_CONFIG_DIR`). Задать `CLAUDE_CONFIG_DIR` даже равным
дефолту — уже другая запись.

Проверено: `sha256("/Users/borodatych/.claude")[:8] = e2b18f98`;
`security find-generic-password -s 'Claude Code-credentials-e2b18f98'` → acct borodatych, создана
2026-08-14 08:53Z, изменена 2026-08-30 08:30Z — это живая запись. Запись без суффикса «Claude
Code-credentials» (2026-07-04) принадлежит запускам без переменной — тем, что ведут `~/.claude.json`.
Отчёт layout приписал живой OAuth именно ей — ошибка. В `login.keychain-db` 63 записи с
хеш-суффиксом — по одной на каждое уникальное значение `CLAUDE_CONFIG_DIR`; что их плодит, не
установлено; серия по одной в сутки около 04:02Z с 24.08 по 02.09 (10 записей; 23.08 — две, 03:53Z и
04:23Z) — источник не установлен. Снять или поменять
переменную = relogin; док authentication: «keys the macOS Keychain entry to that directory too, so a
session with a different CLAUDE_CONFIG_DIR reads a different entry». Удаление записи у CLI —
`security delete-generic-password -a <acct> -s <name>` (строка бинаря).

## `.credentials.json` на Windows/Linux и недокументированная `CLAUDE_SECURESTORAGE_CONFIG_DIR`

Путь plaintext-файла — `b_n()`: `join(ine(), ".credentials.json")`; `ine()` = значение
`CLAUDE_SECURESTORAGE_CONFIG_DIR` (пустая строка → `~/.claude`), иначе `xn()`. Док: «On Linux and
Windows, the credentials go into a plaintext JSON file… If you've set CLAUDE_CONFIG_DIR…
.credentials.json lives under that directory». Так 6 июля токены Windows легли в облако:
`init.ps1:8`/`init.cmd:10` ставили `CLAUDE_CONFIG_DIR` в папку OneDrive. Файл 2 951 B, mode 0700,
гидрирован; содержит только блок `mcpOAuth` для 8 серверов `plugin:engineering` (linear, datadog,
slack, notion, asana, github, atlassian, pagerduty), блока `claudeAiOauth` нет; те же серверы во
всех `mcp-needs-auth-cache` помечены «needs auth» — токены протухли, но лежат.

`CLAUDE_SECURESTORAGE_CONFIG_DIR`: 11 вхождений в strings 2.1.232, 10 в 2.1.255, явно
пробрасывается в env подпроцессов; 0 упоминаний в env-vars.md, authentication.md и CHANGELOG.md —
недокументирована; на Windows-сборках 2.1.227/229 не проверена. CLI следит за mtime
`ine()/.credentials.json` и перечитывает при изменении. Вывод судьи: секреты, зависящие от
недокументированной переменной, «fail open» — исчезнет переменная, файл снова ляжет в конфиг-дир.

Другие секреты: `sessions/<pid>.<sha256>.key` (peerToken межсессионного сокета
`/tmp/cc-socks/<pid>.sock`), `.session_ingress_token` (в списке исключений бинаря), в Desktop —
`ant-device-registry.json` (`pk1:…`, ключ устройства) и `config.json → oauth:tokenCache*`.

## Список «машинно-локального» зашит в бинарь

`$Ug` (снапшот host-конфига для self-hosted runner, 2.1.232): `rOE = {.claude.json,
.claude.json.backup, .credentials.json, projects, sessions, todos, shell-snapshots, statsig,
file-history, history.jsonl, ide, logs, backups, .session_ingress_token}` плюс regex
`^\.claude(-[a-z-]+)?\.json(\.backup)?$` и `^\.config\.json(\.|$)`. Совпадает с консенсусом
дотфайл-гайдов (nickang, steeman, chezmoi-гисты); официальный split просят в
[#81392](https://github.com/anthropics/claude-code/issues/81392) (open). Ни в одном доке нет
строки про синк каталога между машинами; про облачные папки — только bug-fix записи changelog
(2.1.7, 2.1.72, 2.1.163, 2.1.181).

## `projects/<enc>`: основной путь видит ссылки, фолбэк-сканы — нет

Кодирование: `yTo(e)=e.replace(/[^a-zA-Z0-9]/g,"-")`, при >200 символов — обрез до 200 + `-` +
base36-хеш (fix 2.1.224); CLI и Desktop перед этим делают `NFC(realpath)` — формула и эталоны в
[projectDirEncoding.md](projectDirEncoding.md). Локально 28 симлинков →
`~/OneDrive/.claude/projects/-ALL-/<repo>` (353 транскрипта; du 493 MB в research, 913 MB на 12:50 —
зависит от гидрации OneDrive, 67 файлов всё ещё blocks=0), реальных каталогов 0 на момент research — в
12:39 появился один: `-Volumes-Storage-Projects-VibeCode-VibeMemory` (jsonl + `<sid>/` + `memory/`) от
Desktop-сессии pid 17546 в новом cwd, для которого ссылки не было; `-` для
cwd=`/` (109 jsonl), `D--Projects-VibeCode-VibeIDE` для Windows-пути; несколько ссылок ведут в один
стор. Основной путь (readdir вычисленного каталога) сквозь ссылку работает — текущая сессия так и
пишет; фолбэк-сканы «по всем проектам» (CLI `jVt`, Desktop `QNr`) фильтруют `Dirent.isDirectory()`,
для симлинка это false (`node`: 28 записей, 28 symlink, 0 directory) → пикер `claude -r` и поиск
по id через ссылки слепы ([#46342](https://github.com/anthropics/claude-code/issues/46342), closed
stale; на 2.1.232 не перепроверено). Единственный override имени — `CLAUDE_CODE_PROJECT_DIR_NAME`
(2.1.234+, только вместе с `CLAUDE_CONFIG_DIR`, одно имя на окружение запуска, не из settings).

Где ещё зашит абсолютный путь: `cwd` в каждой записи транскрипта и сабагентов, `project` в
`history.jsonl`, ключ `projects["<abs>"]` и `githubRepoPaths` (протух:
`/Users/borodatych/Projects/VibeCode/VibeIDE`) в `.claude.json`, `installLocation` в
`known_marketplaces.json`, `cwd/originCwd` дескрипторов Desktop (23 с `D:\…`, трансляции нет —
resume отказывает «Working directory no longer exists»). `sessions-index.json` в 2.1.232 не
упоминается вовсе (0 строк) — транскрипты единственный источник истины.

## Два свипа-удалителя и симлинки

- `cleanupPeriodDays`: дефолт 30, минимум 1, unlink по mtime мимо корзины — `projects/**/*.jsonl`,
  с 2.1.117 также `tasks/`, `shell-snapshots/`, `backups/`; `memory/` исключён; Desktop/Cowork-
  транскрипты бессрочно с 2.1.248 (`desktopSessionCleanupPeriodDays`). Схемно-невалидный
  settings.json → дефолт 30 ([#41458](https://github.com/anthropics/claude-code/issues/41458));
  нечитаемый → «Skipping cleanup: a settings file could not be read or parsed…» (строка 2.1.232);
  какой путь срабатывает на валидном JSON с невалидной схемой в 2.1.232 — не проверено.
  Наблюдение: маркер писался сегодня дважды, `cleanupPeriodDays` не задан, а 17 транскриптов
  старше 30 дней (старейший EventHub 2026-06-30) и 60 sidecar-папок в `-ALL-` целы — сквозь
  симлинки `projects/<enc>` свип не ходит; недокументировано, может смениться релизом.
- rmdir-свип ([#86952](https://github.com/anthropics/claude-code/issues/86952), bcherny на 2.1.233):
  раз в процесс (≤1/24 ч) rmdir по каждому каталогу под `projects/`, пустые удаляются независимо
  от возраста и настройки; «placing any file inside the affected directory will protect it».
  Правдоподобная причина прецедента 25.07 «пустые сторы не пережили синк» (непроверено).

## Стор Desktop — отдельный каталог с той же болезнью

`~/Library/Application Support/Claude/claude-code-sessions` → симлинк на OneDrive (04.07);
`<accountUuid>/<orgUuid>/local_<id>.json` — 166 файлов (в т.ч. 4 конфликт-копии
`-GPD-WIN-MAX2`) + 2 tombstone `deleted_<id>`; поля `cliSessionId` (у 149), `cwd/originCwd`, `lastFocusedAt`
(перезапись при каждом фокусе — горячий набор); секретов нет. Desktop не ищет транскрипт сам —
спавнит CLI с cwd дескриптора и `--resume=<cliSessionId>`. При «транскрипт не найден» приложение
ставит `transcriptUnavailable` (20 дескрипторов) и через `clearStaleResumeHandle` навсегда стирает
`cliSessionId` (16 без него); конфликт-копии с Windows отличаются от оригиналов этим флагом (у одной из
трёх сверенных пар — ещё `remoteMcpServersConfig`).
Слой private-file: «components under the config root may not be symlinks» — запись →
`PlantDetectedError`, чтение → warnAndRead; ссылки `projects/<enc>` сегодня терпит.
`local-agent-mode-sessions/` — Cowork/scheduled-задачи (systemPrompt 57 KB, email), не Code-сессии,
не делить.
