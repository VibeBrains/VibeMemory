# Жизненный цикл хуков Claude Code глазами синка (2026-09-02)

Собрано в workflow ClaudeSync (четыре исследователя, три дизайна, судьи, синтез) и проверено на
этом Mac: терминальный CLI 2.1.232 (`~/.local/bin/claude`), Claude Desktop 1.40609.1 со встроенным
CLI 2.1.255, оба пишут в `CLAUDE_CONFIG_DIR=/Users/borodatych/.claude`. Эмпирика — два изолированных
прогона с собственным `CLAUDE_CONFIG_DIR` (`scratchpad/sstest` в режиме `-p`, `scratchpad/racetest`
в режиме stream-json), строки бинарей 2.1.232/2.1.255, живые процессы Desktop (pid 6278, 11657) и
стор `~/OneDrive/.claude/projects/-ALL-`. Windows-машина (GPD, CLI 2.1.227/229) вживую не
прогонялась — всё про неё ниже помечено как непроверенное. Официальная документация:
https://code.claude.com/docs/en/hooks, https://code.claude.com/docs/en/env-vars,
https://code.claude.com/docs/en/claude-directory.

## SessionStart: что приходит на вход

- Событие есть с 1.0.62; matcher — `startup | resume | clear | compact | fork` (`fork` с 2.1.214).
  Выходные поля: `additionalContext`, `initialUserMessage`, `sessionTitle`, `watchPaths`,
  `reloadSkills` (`sessionTitle` и `reloadSkills` — с 2.1.152); обычный stdout тоже попадает в
  контекст модели.
  Поддерживаются только `type: command` и `mcp_tool`.
- stdin на 2.1.232 (`-p`, `source=startup`) содержал ровно пять полей:
  `session_id, transcript_path, cwd, hook_event_name, source` — без `permission_mode`, `model`,
  `prompt_id` (sstest/out.txt). Документация обещает опциональные `model/agent_type/session_title`,
  а на `resume`/`fork` с 2.1.251 ещё четыре поля свежести кэша
  (`seconds_since_last_response, context_tokens, prompt_cache_likely_expired, estimated_cache_write_usd`).
- Окружение хука: `CLAUDE_CODE_SESSION_ID`, `CLAUDE_PROJECT_DIR`,
  `CLAUDE_ENV_FILE=<cfg>/session-env/<sid>/sessionstart-hook-0.sh`; `CLAUDE_CODE_BRIDGE_SESSION_ID`
  задаётся только при активном Remote Control (в тесте пусто). На SessionEnd `CLAUDE_ENV_FILE` уже пуст.
- Кодировку каталога проекта переизобретать не нужно: `enc = basename(dirname(transcript_path))`.
  Сама кодировка — NFC-нормализация (в коде Desktop `cliSanitizeCwdSimple`; в CLI 2.1.232 сам
  кодировщик `yTo` — только regex, NFC делает обёртка realpath `ZI`) + `[^a-zA-Z0-9]`→`-`, при длине
  >200 усечение до 200 + `-` + хэш (base36 — по коду Desktop; в CLI хэш `xAy` не разбирался;
  фикс 2.1.224).
- Грабли пути в истории релизов: 2.1.72 — `transcript_path` не в тот каталог для resumed/forked;
  2.1.141 — несуществующий `transcript_path` после EnterWorktree; 2.1.73 — SessionStart стрелял
  дважды на resume. При `--continue`/`--resume` без явного id `CLAUDE_CODE_SESSION_ID` в
  MCP-подпроцессах может содержать стартовый id (env-vars).

## SessionStart завершается до первой записи транскрипта

- racetest (stream-json, хук с четырьмя `sleep 1`, `timeout: 20`): старт CLI 11:59:21.66 → хук
  11:59:22.31 → все четыре секунды `transcript MISSING (dir missing)` → конец хука 11:59:26.376 →
  первая запись `queue-operation enqueue` 08:59:26.392Z (+16 мс), рождение файла 11:59:26.409
  (+34 мс). В `out.jsonl` порядок системных событий: `hook_started` → `hook_response` → `init`.
  CLI ждёт хук; будь иначе, файл появился бы во время четырёхсекундного сна.
- sstest (`-p`, не залогинен): SessionStart 08:22:17Z, четыре секунды файл и каталог отсутствуют;
  первая запись 08:22:21.656Z, рождение файла 08:22:21.677Z; SessionEnd в 08:22:21Z уже видит
  `size=10739` и рядом каталог `memory/`. Файл создаётся даже когда запрос к модели не ушёл
  («Not logged in»). Первая запись — `queue-operation enqueue`, затем `dequeue`, затем `user`.
- Живая Desktop-сессия подтверждает порядок: `createdAt` 07:24:46.761Z → первая запись транскрипта
  07:24:47.099Z (~340 мс) → первое сообщение пользователя 07:24:49.041Z. Тот же механизм в issue
  #62017: `/desktop` первой командой → «CLI session transcript not found».
  Док: «The transcript file is written asynchronously and may lag the in-memory conversation».
- Интерактивный TTY не прогонялся. Changelog 2.1.47 «deferring SessionStart hook execution» — по
  логике порядок тот же, но это предположение, а не замер.
- Следствие для синка: ссылка `projects/<enc>` обязана появиться внутри SessionStart, первым шагом
  и без сети. Для `source=resume` хук бесполезен как «pull перед продолжением»: по утверждению
  судьи (не измерено), CLI уже загрузил транскрипт в память до хука. У Desktop pre-launch хука нет
  вовсе — свежесть транскрипта там зависит только от внешнего таймера.

## Таймаут SessionStart и что происходит по нему

- Таймаут — поле `timeout` конкретного хука в секундах (`{"type":"command","command":…,"timeout":20}`).
  Дизайны закладывали 45 и 60 с с сетью внутри; синтез — 10 с и запрет сети.
- По таймауту CLI отбрасывает хук и продолжает старт (hooks: «cancels a … hook that reaches its
  timeout, discarding the hook's output»): ссылки нет → после конца хука (stream-json: +34 мс; в
  `-p` — ~4 с) CLI материализует настоящий каталог `~/.claude/projects/<enc>`, и сессия пишет
  туда до самого конца (Stop-хук её уже не перенесёт). Каждый офлайн-старт с `git fetch` внутри
  хука ждёт полный таймаут fetch (в git-дизайне native-or-different-transport — 15 с).
- Хук, убитый по таймауту посреди merge или пакета rename, оставляет полуперенесённое состояние —
  поэтому всё сетевое и всё, что не идемпотентно, из SessionStart вынесено.
- По оценке судьи, при провале zod-валидации `settings.json` хуки не грузятся вообще — защитный
  гейт в хуке не сработает ровно тогда, когда нужен (см. retention ниже).

## UserPromptSubmit: единственное место, где можно остановить ввод

- Схема вывода в 2.1.255: `hookSpecificOutput{hookEventName:"UserPromptSubmit", additionalContext,
  sessionTitle, suppressOriginalPrompt}` с описанием «When decision is "block", omit the original
  prompt from the block message»; в stream-json причина блокировки уходит хосту текстовым баннером
  («hook feedback (e.g. a UserPromptSubmit hook's block reason)»). Превышение таймаута логируется:
  `Hooks: prompt hook (…) timed out after Nms (limit Mms)`; по документации вывод просроченного
  хука отбрасывается и промпт уходит модели без него — гейт, не уложившийся в свой `timeout`,
  ничего не блокирует.
- Синтез использует это как гейт свежести: на первом промпте сессии и далее раз в >5 мин —
  `git fetch` (5 с; офлайн → пропуск с пометкой); если `origin/main` меняет `<sid>.jsonl` этой
  сессии или `machines/*/live.json` показывает sid живым на другой машине (heartbeat <30 мин) —
  слить файл на диске и ответить `decision: block` («сессию продолжили на <машина>: закрой и открой
  заново»). Вне гонки — no-op за миллисекунды. Это ответ на вилку «продолжили на B, потом ввели в
  старую вкладку Desktop на A»: судьи назвали отсутствие такой проверки провалом всех трёх дизайнов.

## Stop: коммит сразу, push с дебаунсом

- git-дизайн делал на Stop дебаунс 30 с и затем commit+push; судьи: «закрыл крышку» внутри окна
  теряет последние ходы, потому что SessionEnd при сне не срабатывает, а таймер не идёт. Дизайн
  local-hot-auto-durable на Stop зеркалил только `history`/`tasks`, не транскрипт — вкладки Desktop
  живут днями, значит сессия невидима второй машине днями.
- Синтез: Stop с `async: true`, commit немедленно, push в фоне с дебаунсом 20 с. Снимок живого
  транскрипта берётся до последнего `\n`, драйвер отбрасывает неразбираемые строки: `git add` по
  append-only файлу может захватить оборванную строку (O_APPEND многомегабайтных tool-result не
  атомарен). Поле `"async": true` документировано (hooks: только для `type: "command"`, `timeout`
  на такой хук не действует, блокировать или управлять он не может); в строках 2.1.232 и 2.1.255
  хук-раннер несёт поля `asyncResponse`/`asyncRewake`.

## SessionEnd: бюджет 1.5 с — это пол, а не потолок

- Строки 2.1.232 и 2.1.255 совпадают: таймаут SessionEnd-хуков =
  `max(1500, min(max(timeout×1000 по всем SessionEnd-хукам), 60000))`, переопределяется
  `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS` (константы `1500`/`60000` в обоих бинарях; в
  документации hooks/env-vars — «1.5 s … raised … up to 60 seconds», настраиваемо с 2.1.74); в
  `shutdown()` взводится `armShutdownFailsafe(max(5000, timeout+5000))` мс (в 2.1.232 —
  `max(5000, timeout+3500)`), сами хуки запускаются с `AbortSignal.timeout(timeout)`, дальше процесс
  уходит принудительно. Неудачный хук пишет в stderr `SessionEnd hook [<command>] failed: …`.
- Значит «бюджет 1.5 с по умолчанию» из cloud-first-дизайна верен только без поля `timeout`; синтез ставит
  `timeout: 60` и делает commit синхронно, push — отсоединённым процессом. Судья пометил как
  непроверенное, переживёт ли отсоединённый процесс выход родителя при спавне из Git Bash.
- stdin SessionEnd (sstest): `session_id, transcript_path, cwd, prompt_id, hook_event_name,
  reason:"other"`. По утверждению судей (не измерено; документация перечисляет только штатные
  `reason` — `clear`, `resume`, `logout`, `prompt_input_exit`, …, `other`), SessionEnd не приходит
  при сне машины и при убийстве CLI Desktop-ом — на это нельзя вешать единственный commit.

## Desktop передаёт спавну CLI пользовательские настройки

- `ps` по pid 6278 и 11657 (встроенный CLI 2.1.255, родитель
  `/Applications/Claude.app/Contents/Helpers/disclaimer`): `--output-format stream-json
  --input-format stream-json --permission-prompt-tool stdio --resume=<id>
  --setting-sources=user,project,local --settings {}`. Хуки из `~/.claude/settings.json`
  применяются к Desktop-сессиям — на этом держится вся Desktop-половина синка.
- Окружение CLI Desktop собирает из белого списка (asar `index.chunk-jfL6G7d3.js`: `A_n = new
  Set(['PATH','CLAUDE_CONFIG_DIR','CLAUDE_CODE_TMPDIR','CLAUDE_CODE_EFFORT_LEVEL',
  'MCP_SERVER_CONNECTION_BATCH_SIZE','CLAUDE_CODE_SHELL','CLAUDE_CODE_SHELL_PREFIX','SSH_AUTH_SOCK',
  'NODE_EXTRA_CA_CERTS','SSL_CERT_FILE','SSL_CERT_DIR', + телеметрия/`OTEL_*`])`; док: «reads your
  shell profile … to extract PATH and a fixed set of Claude Code variables»); в env pid 6278 —
  `CLAUDE_CONFIG_DIR`, `CLAUDE_CODE_ENTRYPOINT=claude-desktop`, `CLAUDE_CODE_HOST_SESSION_ID=local_…`.
  Хук не может рассчитывать на произвольные переменные шелла. `CLAUDE_CODE_PROJECT_DIR_NAME` в
  белом списке нет; в бандле Desktop приложение само ставит `CLAUDE_CODE_PROJECT_DIR_NAME:
  'session'` только для сессий типа `3p` (`index2.chunk-CTdL-Pvd.js`) — per-repo для вкладки Code
  через Desktop не задать (исследователь помечал применимость к Desktop как непроверенную, синтез —
  как неприменимую; вживую не пробовалось).
- Desktop-сессии не пишут `history.jsonl` (в `~/.claude/history.jsonl` одна строка от 14.08, все
  сессии после — `entrypoint claude-desktop`).

## Windows: Git Bash и `$HOME`

- Схема хука в 2.1.255 (те же строки и в 2.1.232): «When absent, `command` runs through a shell
  (bash on POSIX, PowerShell on Windows without Git Bash)»; поле `shell`: `'bash'` — ваш `$SHELL`,
  `'powershell'` — pwsh, по умолчанию bash. Без Git Bash: «Git Bash not found; BashTool will be unavailable»; путь задаётся
  `CLAUDE_CODE_GIT_BASH_PATH` (есть в 2.1.232 и 2.1.255).
- Под Git Bash `HOME=%USERPROFILE%`, поэтому одна команда `"$HOME/ClaudeSync/bin/claudesync" …` в
  общем `settings.json` работает на обеих ОС (macOS — `sh -c`). Это расчёт дизайна, вживую на
  Windows не проверялся.
- Не проверено на Windows: хуки Desktop-CLI через Git Bash; `node`/`python` в PATH внутри Git Bash;
  путь `$CLAUDE_CONFIG_DIR/sync/hook.sh` с обратными слешами под `sh -c`; переиспользование pid
  (`kill -0`/`tasklist` дают ложное «жив» или «мёртв»).

## `CLAUDE_CODE_SKIP_PROMPT_HISTORY` — машинные прогоны без следов

- env-vars: «Set to 1 to skip writing prompt history and session transcripts to disk. Sessions
  started with this variable set do not appear in `--resume`, `--continue`, or up-arrow history». В
  бинарях есть (2.1.232 — 9 вхождений, 2.1.255 — 6): писатель истории выходит по
  `add(e,t,r){if(Hn(process.env.CLAUDE_CODE_SKIP_PROMPT_HISTORY)||…)return` (в 2.1.255 — `Le(…)`),
  гейт персистенции сессии возвращает `"skip_prompt_history"` с UI-строкой «Transcript saving is
  off — CLAUDE_CODE_SKIP_PROMPT_HISTORY is set · --resume will not find this session». Сборщик
  окружения подпроцессов (там же, где подставляются `AWS_CONFIG_FILE`/`CLOUDSDK_CONFIG`) переменную
  удаляет: `delete r.CLAUDE_CODE_SKIP_PROMPT_HISTORY` (2.1.232) — хук её, судя по строкам, не увидит.
- Нужна раннерам VibeDub: 109 сессий `claude -p` с `cwd="/"` лежат в сторе `-`. Переменная
  рекомендательная, не принудительная; синтез парой ставит `CLAUDESYNC_SKIP=1`, чтобы и хук выходил
  (см. выше — саму `CLAUDE_CODE_SKIP_PROMPT_HISTORY` хук не получает).

## Retention-свип `cleanupPeriodDays`

- Дефолт 30 дней, минимум 1 (`0` отвергается), удаление по mtime через unlink мимо корзины;
  цель — `projects/**/*.jsonl` (и `.cast`, #59248), с 2.1.117 также `tasks/`, `shell-snapshots/`,
  `backups/`; `memory/` исключён; `todos/`, `statsig/`, `logs/` — legacy, выметаются с каталогом.
  С 2.1.248 транскрипты сессий, начатых или последний раз продолженных в Desktop/Cowork, хранятся
  бессрочно, пока не задан `desktopSessionCleanupPeriodDays`. Маркер — `~/.claude/.last-cleanup`
  (сегодня `2026-09-02T08:33:20.499Z`).
- Открытые data-loss issues #62476/#59248: любой restore или sync-клиент, сбивающий mtime, меняет
  исход; 2.1.181 чинил «idle sessions losing their history when another Claude Code process ran
  the 30-day transcript cleanup».
- Невалидный `settings.json`: прецедент #41458 — zod-валидация упала, применился дефолт 30 при
  явно заданном большом значении. В 2.1.232 и 2.1.255 поведение другое — строка «Skipping cleanup:
  a settings file could not be read or parsed, so cleanupPeriodDays may be set to a value that
  cannot be seen. Fix the settings file (see /doctor) to re-enable cleanup.» (fail-closed,
  `settings_unknowable`). Старые сборки, в том числе Windows 2.1.227/229, по общему дереву пройдут
  первыми по старым правилам (в #41458 упоминается, что защита есть «в 2.1.85+», — граница не
  проверялась). По оценке судьи (не проверено), хуки при невалидном файле тоже не грузятся.
- Свип не ходит сквозь симлинки — наблюдение, не документация. `cleanupPeriodDays` в
  `~/.claude/settings.json` не задан (`grep -c` → 0), свип сегодня прошёл, `~/.claude/projects` —
  28 симлинков и 0 реальных каталогов на момент замера (при перепроверке в 12:48 рядом появился
  один реальный каталог `-Volumes-Storage-Projects-VibeCode-VibeMemory`, созданный в 12:39 первой
  сессией в cwd без заготовленной ссылки — ровно тот случай, который обязан закрывать
  SessionStart-хук), а в `-ALL-` уцелели: 17 транскриптов сессий старше 30 дней
  (`find -maxdepth 2 -name '*.jsonl' -mtime +30`; старейшие EventHub 2026-06-30 и 2026-07-02,
  VibeIDE 2026-07-04), 529 `*.jsonl` старше 30 дней на любой глубине (`find -name '*.jsonl'
  -mtime +30`; с `journal.jsonl` сабагентов и workflow; всего на любой глубине 1766, из них 101
  `journal.jsonl`) и 60 sidecar-каталогов старше 30 дней (`find -mindepth 2 -maxdepth 2 -type d
  -mtime +30`; всего на этой глубине 110, включая `memory/`). Любой релиз может это сменить,
  поэтому синтез ставит `cleanupPeriodDays: 3650` в общий `settings.json` и валидирует его в `doctor` — при том что защитное значение живёт в
  том же файле, чью невалидность оно должно пережить, а самый рискованный блок в нём — `hooks`.

## rmdir-свипер пустых каталогов (#86952)

- Отдельный недокументированный проход: rmdir по каждому каталогу под `projects/` раз в процесс,
  через секунды–20 мин после старта, не чаще раза в 24 ч, без оглядки на `cleanupPeriodDays`.
  auditd: 461 rmdir за миллисекунды, `comm="Bun Pool 0"`, 8 всплесков в день; bcherny
  2026-08-17 подтвердил на 2.1.233: «empty directories are removed regardless of age or
  cleanupPeriodDays… placing any file inside the affected directory will protect it». Сносит и
  Syncthing-маркер `.stfolder`. Статус у исследователя — «вероятно» (по issue, локально не ловилось).
- Самое правдоподобное объяснение прецедента 2026-07-25: пустые сторы не пережили синк → 11
  висячих ссылок. Сегодня под `-ALL-` пустых каталогов 0 (`find -type d -empty`), проверить
  вживую нечего; ходит ли свипер по симлинкам — не проверено. Синтез кладёт `.keep` в каждый стор
  независимо от ответа.
- Windows (не проверено): `RemoveDirectory` по junction удаляет саму junction независимо от
  содержимого цели — ферма ссылок может исчезать раз в сутки до следующего tick, и `--resume`
  первой командой в это окно транскрипт не найдёт.

## Реестр живых сессий и свип dead-owner

- `~/.claude/sessions/<pid>.json` (2.1.255, ключи 6278.json): `bridgeSessionId, cwd, entrypoint,
  kind, messagingSocketPath (/tmp/cc-socks/<pid>.sock), name, nameSince, nameSource, peerFeatures,
  peerProtocol, pid, pidDomain, procStart, sessionId, startedAt, version`; рядом
  `<pid>.<sha256>.key` (108 Б, 0600): `peerToken, pidDomain, procStart`. Windows-запись 2.1.227 в
  `~/OneDrive/.claude/sessions/20712.json` — без `pidDomain`; в строках 2.1.232 `pidDomain` нет
  вовсе (0 вхождений; `.key` там пишется как `{peerToken, procStart|procStartFt}`), в 2.1.255 — 6
  строк (20 вхождений). Док: «one small file per running session, used to detect
  concurrent sessions and crashes… removes each file when its session exits and clears crash
  leftovers on the next launch».
- Классификация записей реестра в 2.1.232 (в читателе peer-ключа, строки те же в 2.1.255):
  `isProcessProvablyGone` = `process.kill(pid, 0)` → жив, только `ESRCH` → «доказуемо мёртв»; при
  `requireLiveOwner` запись с мёртвым pid или с несовпавшим `procStart` (`getProcessStartTimeAsync`)
  получает `kind:"dead-owner"`; при публикации своего ключа с флагом `sweepPermitted` прогоняется
  подметание каталога `sessions/`. Проверка живости — только по локальной таблице процессов.
- Почему liveness-маркер в синканном дереве уничтожается другой машиной — сценарий судьи, вживую
  не воспроизводился (документация подтверждает лишь «clears crash leftovers on the next launch»):
  Desktop на Mac держит CLI живым на каждую открытую карточку часами (pid 6278 с 10:49 местного
  по `ps` и `sessions/6278.json`; Desktop-карточка создана 07:24:46Z = 10:24 местного), Windows
  стартует Claude по тому же дереву, пробует pid 6278 у себя — его нет (или занят чужим
  процессом) — и удаляет `sessions/6278.json` + `.key` как crash leftover. Всё, что после этого судит о живости по реестру, считает Mac-транскрипт мёртвым и
  переименовывает или переписывает его под живой append — прецедент расщепления 2026-08-08, но уже
  автоматический. 2.1.232 не различает даже ОС записи.
- Поэтому синтез выносит живость из `sessions/` в `machines/<id>/live.json` (heartbeat со
  Stop/SessionStart, TTL 30 мин) в репо, а локальный реестр читает fail-closed: не читается или не
  парсится — считать живым. Консервативное правило cloud-first-дизайна — чужая запись с совпавшим
  pid даёт ложное «жив» → пропустить — безопасно, но не спасает от удаления самой записи.
- CLI не держит дескриптор транскрипта: `lsof` по pid 6278 и 11657 — 0 открытых `.jsonl`, запись
  идёт open/append/close по пути. Подмена inode сама по себе записи не теряет; чем на самом деле
  вызвано расщепление 08.08, источники не объясняют.
