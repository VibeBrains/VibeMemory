# OneDrive как транспорт стора Claude Code: режимы отказа (2026-09-02)

Проверено на Mac владельца (CLI 2.1.232 в терминале, Desktop с встроенным CLI 2.1.255) на живом
сторе `~/OneDrive/.claude`, где с июля 2026 лежат транскрипты `projects/-ALL-/<repo>`, память,
дескрипторы Desktop, а до 2026-07-30 лежал весь `CLAUDE_CONFIG_DIR`. Фактура — четыре
исследовательских отчёта (native / community / layout / desktop), вердикты судей по трём
дизайнам синка, `README.md` самой облачной папки и повторный `stat`/`find` по стору сегодня.
Без пометки — проверено; «вероятно» — объяснение без прямого доказательства; «не проверено» —
так и названо. Вывод: облачная папка не транспорт для стора — она не сливает, не гарантирует
байты на диске и переносит то, что переносить нельзя.

## Files On-Demand: половина стора — плейсхолдеры, чтение — сетевая докачка

`find ~/OneDrive/.claude -name '*.jsonl' -exec stat -f '%b %z %N'` сегодня: **932 из 1766** jsonl
(первый замер сегодня — из 1759) имеют `blocks=0` при `size>0` — файл существует только в облаке.
Среди 353 верхнеуровневых транскриптов `-ALL-/<repo>/<uuid>.jsonl` дегидрированы 67: EventHub 4/4,
VibeReviewer 1/1, VibeReel 9/10, BuzzBang 9/11, Undercut 7/8, VSCodeSync 6/8, VibeIDE 27/80,
VibeSter 4/13. Дегидрированы и `tasks/` (17 папок), `telemetry/` (74 файла; в отчёте layout — 76),
`cache/changelog.md`.

Чтение плейсхолдера блокирует процесс на докачку: цикл `head` по jsonl VibeIDE (судья насчитал 133
файла; сегодня `find -maxdepth 1` даёт 80, рекурсивно 384 — число 133 не воспроизведено) не
уложился в 2 минуты при листинге каталога за 0.01 с; `grep` по VibeIDE (223 МБ по `du` на момент
замера, 403 МБ к 12:50) дважды упёрся в таймаут (exit 143). Для `--resume` это значит: первый
запуск сессии, пришедшей с другой машины, — ожидание сети, офлайн — отказ. Desktop при
подтверждённом отсутствии транскрипта ставит `transcriptUnavailable` и на `cli_resume_not_found`
навсегда стирает `cliSessionId` (`clearStaleResumeHandle`); что дегидрированный файл проходит по
этой же ветке — оценка судьи, не измерено.

Пин «Always keep on this device» на macOS ставится руками в Finder (скриптового способа судьи не
нашли — оценка, не проверено): File Provider (`~/Library/CloudStorage`) не даёт отключить Files
On-Demand и не даёт перенести sync root (MS Q&A 5269746). Скриптом пинятся только
Windows-плейсхолдеры (`attrib +P -U <path> /S /D`; `-P +U` — «Free up space»), и на поведение
`mkdir` пин не влияет (#50886).

## OneDrive не сливает — он плодит конфликт-копии

Файл, переписанный двумя машинами, остаётся в одной версии, вторая кладётся рядом как
`<имя>-<имя устройства>.<ext>`. В сторе прямо сейчас 12 таких файлов: транскрипт
`-ALL-/VibeIDE/78dcced4-…-MacMini.jsonl`, 4 дескриптора Desktop
`claude-code-sessions/…/local_{02ea2773,077ceed4,244ba07c,f7905ca2}-GPD-WIN-MAX2.json`,
`-ALL-/VibeIDE/memory/MEMORY-GPD-WIN-MAX2.md` (31.07), `.claude-GPD-WIN-MAX2-2.json`,
`.claude.json-GPD-WIN-MAX2.backup`, `mcp-needs-auth-cache-GPD-WIN-MAX2{,-2}.json`,
`-GPD-WIN-MAX2{,-2}.last-cleanup`. Ещё 8 копий (четыре `.claude.json`, кеш, `.last-cleanup`,
транскрипт VibeIDE от 06.07, `memory/release-log.md` от 19.07) разобраны 2026-07-30 — уникального
контента в них не было, но били они по контенту, а не только по конфигу. Копии порождают обе
машины: Mac даёт суффикс `-MacMini` (имя устройства пришлось переименовать — исходное содержало
U+00A0).

Копии не безобидны. Desktop грузит их наравне с оригиналами (фильтр `local_*.json`), дедуп по
внутреннему `sessionId`, побеждает прочитанный первым — недетерминированно. Три пары оригинал/копия
из четырёх отличаются флагом `transcriptUnavailable` (плюс `lastFocusedAt`/
`lastActivityAt`/`completedTurns`; у f7905ca2 ещё `remoteMcpServersConfig`), четвёртая (244ba07c,
сессия 78dcced4) — только полями активности: Windows не нашла Mac-транскрипты и пометила
дескрипторы; итог сегодня — 20 дескрипторов с флагом, 16 без `cliSessionId`. Причина копий
`.claude.json` — сам CLI: файл переписывается целиком (tmp + rename) при каждом старте и обновлении
кешей, idle Desktop-CLI сделал 5 перезаписей за 27 минут (бэкапы 10:22–10:49; к 12:45 ротация
прошла снова — 5 бэкапов за 12:40–12:45), так что каждое пробуждение второй машины — новая копия.
`machineID` при этом не аппаратный: одно значение `2799b878…` и `firstStartTime 2026-07-21` во всех
четырёх копиях — идентичность «приехала с файлом», а 21.07 конфиг был пересоздан с нуля (вероятно,
из-за `+R` и rename-stall на Windows, см. ниже; #29153 описывает тот же каскад 35 КБ → 77 Б).

## Ссылки внутри синк-дерева уезжают текстовыми файлами, «лечение» не срабатывает

macOS-клиент выгружает symlink как обычный файл размером с длину пути и с путём внутри: в
хранилище клиента ссылка `projects/-Users-…-Undercut` лежала как `Regular File, 58 байт` с
текстом `/Users/borodatych/OneDrive/.claude/projects/-ALL-/Undercut` (README облачной папки,
раздел 3.2; независимо — The Register 2022-01-14 о File Provider). Клиент периодически пытается
такие элементы «вылечить» и не может: `settings/Personal/healingItems.txt`,
`healingActionSuccessful=0`. На второй машине по тому же пути оказывался файл, и `sync-repo`
до 2026-07-30 рапортовал «истории нет» с кодом 0 (PowerShell-ветка падала на `Substring()`).

Windows-клиент symlink/junction внутри корня официально не поддерживает («This behavior is
intentional», MS Q&A 1295967, 2023) и по отчёту 2025-01 следует по ссылке и тянет содержимое цели
(MS Q&A 1327239). Следствие для обеих ОС: ссылки живут только снаружи синк-дерева и смотрят внутрь.
Отдельная ловушка Claude Code: фолбэк-сканы по всем проектам (CLI `jVt`, Desktop `QNr`) фильтруют
`Dirent.isDirectory()`, для symlink это `false` — `~/.claude/projects` на момент замера 28 ссылок,
0 каталогов (к 12:39 появился один реальный каталог `-Volumes-Storage-Projects-VibeCode-VibeMemory`
с транскриптом: cwd без ссылки — CLI создал каталог сам), и поиск транскрипта по id через ссылки не
находит ничего (#46342). Прецедент 2026-08-08: перелинковка `projects/<enc>` из-под живой сессии
дала 4 версии одного транскрипта.

## Исключить файл нельзя, синк ленивый

Персональный OneDrive не умеет исключать отдельные файлы — только целые папки через «Choose
folders» (MS Q&A 5288096); исключения по маске — только GPO OneDrive for Business. На macOS
с личным аккаунтом настройки нет вовсе (проверено 2026-07-30). Поэтому в облако уезжает всё,
что лежит под `CLAUDE_CONFIG_DIR`, включая то, что Anthropic добавит в следующем релизе.

Ленивость: при закрытой крышке выгрузка OneDrive останавливается, и машина B возобновляет
сессию без последних ходов A; после слияния хвост A остаётся боковой веткой `parentUuid`,
которую resume от последнего листа не показывает (оценка судей; в 2.1.255 `fAr` берёт лист с
`max(timestamp)` — проверено по бинарю). Триггера «по пробуждению» на Mac нет: launchd
`StartInterval` пропускает интервал во сне (`man launchd.plist`).

## Windows: `+R`, reparse points и `EEXIST` у Bun

С Files On-Demand каждый синкаемый файл и папка — reparse point (CldFlt), OneDrive ставит
атрибуты ReadOnly и Pinned (`attrib` → `A R P`, #50886). Бандлированный Bun бросает `EEXIST`
на `mkdirSync({recursive:true})` по read-only каталогу (oven-sh/bun#34413), и Claude Code
чинил это по одному месту: `session-env` (#50886/#51702/#37306 → 2.1.163), `agents` (2.1.181),
`plugins` (2.1.72), `installer`/`claude update` — #81004 открыт, обход `attrib -R <dir> /S /D`.
Осиротевшие плейсхолдеры дают противоречие «mkdir → EEXIST, stat → ENOENT» (#67692; tcaesvk
2026-07-24: единственный триггер — атрибут ReadOnly). Write/Edit в 2.1.69 принимал reparse point
за symlink-escape (#30928). Rename поверх read-only `~/.claude.json` «stall-ил» старт до 2.1.234.
Сайдкары `<sid>/tool-results` под junction в стор не покрыты ни одним фиксом — у Windows-сессии
78dcced4 (2.1.227/229, транскрипт 1.5 МБ) сайдкар-каталога в сторе нет (косвенный сигнал).

Desktop на Windows — MSIX (Store/WinGet и, по отчётам, с ~04.2026 установщик с claude.ai; данные в
`%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude`; Squirrel/.exe —
`%APPDATA%\Claude`), и любой junction под `%APPDATA%\Claude` ломает атомарную запись карточек
(`local_<id>.json.tmp` с флагом `wx` → ложный `EEXIST`, rename → `EXDEV`; #83584, #91409, #48362,
#32533, #57998). Junction `claude-code-sessions → OneDrive` из `init.ps1` на MSIX работать не
может. Работает обратное: junction `%USERPROFILE%\.claude → C:\ClaudeData\.claude` на локальный том
(#90903, #56370). Windows до сих пор на старой схеме: `init.ps1` и `init.cmd` целят
`CLAUDE_CONFIG_DIR` в облачную папку, оттуда же реестры `sessions/20712.json`, `21460.json`.

## Пустые каталоги не переживают синк

Прецедент 2026-07-25: пустые сторы не пережили синк, 11 ссылок `projects/<enc>` повисли (контекст
владельца; в README на эту дату — «сирот и битых ссылок нет» уже после ремонта). Локально
`session-env/` — 57 папок, все пустые; в OneDrive 64 таких же стейл-папки. Вероятное объяснение
прецедента (сторы лежат под `projects/`) — недокументированный свипер CLI: раз в процесс (не чаще
1/24 ч) `rmdir` по каждому каталогу под `projects/` независимо от `cleanupPeriodDays`, подтверждено
bcherny на 2.1.233 (#86952: auditd показал 461 rmdir за миллисекунды; защита — любой файл внутри).
Тот же свипер сносит маркер Syncthing `.stfolder`.

Второй свип — retention: `unlink` мимо корзины по mtime файла для `projects/**/*.jsonl`,
с 2.1.117 также `tasks/`, `shell-snapshots/`, `backups/`; дефолт 30 дней; при невалидном
`settings.json` в версии из #41458 действовал дефолт даже с заданным значением, а в бинарях
2.1.232 и 2.1.255 есть строки «Skipping cleanup: a settings file could not be read or parsed…»
и «Skipping cleanup: settings have validation errors but cleanupPeriodDays was explicitly set»
— свип отключается (fail-closed); для Windows 2.1.227/229 не проверено. OneDrive переносит старый
mtime, так что транскрипт старше 30 дней, пришедший с другой машины, удаляется при первом старте
(#59248/#62476). Сегодня 17 транскриптов старше 30 дней уцелели только потому, что свип не ходит
через symlink — недокументированно, может смениться любым релизом. На общем сторе удаление
уезжает на вторую машину за секунды, откат — корзина/версии OneDrive, вручную.

## Секреты и машинное состояние оказываются в облаке

`~/OneDrive/.claude/.credentials.json` — 2951 Б, mode 0700, mtime 2026-07-06 09:16, гидрирован
(blocks=8). Внутри только блок `mcpOAuth` с токенами восьми серверов плагина engineering
(linear, datadog, slack, notion, asana, github, atlassian, pagerduty), блока `claudeAiOauth`
нет. Причина — Windows/Linux хранят креды plaintext в `CLAUDE_CONFIG_DIR` (документация
authentication), а на Mac они в Keychain, поэтому локально файла нет. Единственная ручка для
Windows — недокументированная `CLAUDE_SECURESTORAGE_CONFIG_DIR` (есть в strings 2.1.232 и
2.1.255; на 2.1.227/229 не проверена). Удалять придётся вместе с корзиной и историей версий
OneDrive плюс отзыв грантов.

Рядом в облаке: Windows-реестры живых сессий `sessions/20712.json`, `21460.json` (pid, cwd,
procStart; `.key`-файлов с peerToken и сокетов в них нет — это 2.1.227/229), `.claude.json` с
`oauthAccount` чужой машины (`replBridgePlaceholders` в облачной копии нет — он только в локальном
Mac-файле), `ide/*.lock`, 64 пустых `session-env/<sid>/` (файлов `sessionstart-hook-*.sh` в них
нет). Общий `settings.json` читается с облачной папки на каждом старте обеих машин (сегодня в нём
только `permissions.allow`, тема и флаги; каталога `hooks/` нет), но блок `hooks` в нём исполнился
бы на обеих: компрометация аккаунта OneDrive — исполнение кода на обеих машинах (оценка судьи).
Anthropic сама зашила список машинно-локального в бинарь: `rOE = {.claude.json,
.claude.json.backup, .credentials.json, projects, sessions, todos, shell-snapshots, statsig,
file-history, history.jsonl, ide, logs, backups, .session_ingress_token}` (снапшот host-конфига
`$Ug` в 2.1.232).

## Что об этом знает Anthropic: changelog и issues

Документация про облачные папки молчит (troubleshooting и debug-your-config — 0 упоминаний
OneDrive/Dropbox/iCloud); auto memory объявлена «machine-local… not shared across machines or
cloud environments». Есть только записи об исправленных багах:

- 2.1.7 — «Fixed false "file modified" errors on Windows when cloud sync tools, antivirus
  scanners, or Git touch file timestamps without changing content»;
- 2.1.72 — «installation failing on Windows with EEXIST error in OneDrive folders»;
- 2.1.162 — «silent startup hang when the config directory is read-only or unwritable»;
- 2.1.163 — «Bash commands failing on Windows with "EEXIST: file already exists" on the
  session-env directory when it has the read-only attribute or is inside OneDrive»;
- 2.1.181 — «Write/Edit producing 0-byte or truncated files on network drives and cloud-synced
  folders» и «agent creation failing with EEXIST … (Windows/OneDrive)»;
- 2.1.234 — «startup no longer stalls on repeated rename retries when ~/.claude.json is read-only».

Запросы на синк без единого ответа Anthropic (0 комментариев MEMBER/COLLABORATOR):

- #22648 «Account-level settings sync across devices» — open с 02.2026, 47 реакций; дубли
  закрыты ботом цепочкой #64081 → #63242 → #38970 → #22648;
- #28791 «Sync conversation history between CLI and desktop» — open, 30 комментариев; дубль
  #56038;
- #73639 «Transfer session to another claude code instance» — open, stale; называет
  `CLAUDE_CONFIG_DIR` на shared storage «unsafe over NFS»;
- #36693, #57678, #45358; #81392 просит официальный split/.gitignore; #25739 — ключ по git remote.

Открытые ямы, бьющие по любой облачной схеме: #82553 (Dropbox Smart Sync — Write подмешивает
старое содержимое плейсхолдера), #78162 (атомарная запись `settings.json` разрешает symlink
на один хоп и подменяет промежуточную ссылку файлом), #47241 (iCloud `fileproviderd`
откатывает `mv`/`rm` через секунды), #32637 (0-байтные стабы iCloud: `cp -a` копирует пустоту,
`rm -rf` удаляет и в облаке; closed not planned).

## Практика сообщества

Консенсус (nickang, elizabethfuentes12, rymiwe, steeman, jtklinger) и список `rOE` совпадают:
делить `CLAUDE.md`, `settings.json`, `skills/`, `commands/`, `agents/`, `rules/`, `hooks/`,
манифесты `plugins/`, `projects/*/memory`; не делить `.claude.json`, `.credentials.json`,
`history.jsonl`, транскрипты, `sessions/`, `cache/`, `debug/`, `telemetry/`, `shell-snapshots/`,
`session-env/`. Транспорт — git-дотфайлы (nickang: `~/.claude → ~/dotfiles/claude` + whitelist
`.gitignore` + LaunchAgent `WatchPaths`; chezmoi у frxiaobei/rymiwe с SessionStart-хуком; stow),
NAS + Syncthing (steeman: NAS по SMB для конфигов, репо через Syncthing, `.sync-conflict-*` при
одновременной правке), iCloud (aiworkflowpro: весь `~/.claude` + один symlink, предупреждает о
параллельной работе двух Mac и Time Machine), Syncthing LAN-only между Mac и Windows (alebrije,
память внутри репо). Готовые инструменты: tawanorg/claude-sync (268★, R2/S3/WebDAV, токен `${HOME}`
переписывает имена `projects/` и содержимое транскриптов, `.conflict`-файлы, `history.jsonl` — LWW
с `rebuild-history`), claude-context-sync (бандлы `${PROJECTS}`, «No conflict resolution — use
sessions alternately»), renefichtmueller/claude-sync (24★, git/iCloud/Dropbox/OneDrive/Syncthing,
path-keying не решает), Dinesh3184/claude-session-sync (iCloud, beta, 1★). `.claude.json` и креды
не входят в таблицу синка tawanorg (остальные явно не проверялись); tawanorg и claude-context-sync
переписывают пути, claude-context-sync запрещает параллельную работу.

Syncthing для сценария «закрыл крышку — открыл другую» не подходит: обмен идёт узел-узел, и
данные доезжают только пока оба в сети — заснувший Mac ничего не отдаст (тезис владельца; в
отчётах исследователей не проверялся). Про Syncthing известно другое: свипер CLI удаляет пустой
`.stfolder` (#86952), каталог-лок `history.jsonl.lock` вероятно уедет на соседний узел (stale
10 с — шумно, не опасно), опубликованного `.stignore` для `~/.claude` нет. Reddit для проверки
недоступен (WebSearch → 400), HN-треды Dotclaude/Claude-Config пустые.

## Вынос

Облачная папка проваливает все три обязанности транспорта: слияние (копии вместо merge),
наличие данных (плейсхолдеры, ленивая выгрузка, `rmdir`/retention-свипы, реплицируемые за
секунды) и границу (нельзя исключить файл — секреты и pid едут вместе с транскриптами).
Нативного переноса у Anthropic нет: Remote Control — окно в живой процесс, teleport — сводка в
новую облачную сессию, `CLAUDE_CODE_PROJECT_DIR_NAME` (2.1.234+) — одно имя на запуск. Отсюда
решение VibeMemory — git-транспорт с union-слиянием append-only JSONL по uuid, ссылки только
снаружи репо и `.keep` в каждом сторе.
