# Как Claude Desktop хранит Claude-Code-сессии (2026-09-02)

Разобрано на macOS: Claude Desktop 1.40609.1 со встроенным CLI 2.1.255, стор владельца
`~/Library/Application Support/Claude/claude-code-sessions` — симлинк (создан 4 июля 12:33) на
`~/OneDrive/.claude/claude-code-sessions`, Windows-машина GPD-WIN-MAX2 подключалась junction-ом из
`init.ps1` (в сторе лежат её дескрипторы и конфликт-копии; текущее состояние junction на GPD не
проверено). Источники: извлечённые бандлы `app.asar` (`index.chunk-jfL6G7d3.js`,
`index2.chunk-DvXkecAn.js`), строки бинаря CLI, сам стор на диске, `ps`/`lsof`, доки
code.claude.com и issues anthropics/claude-code — всё read-only, по итогам исследовательского
workflow 2026-09-02 (четыре исследователя, три судьи). Что не проверено фактом — так и названо.

Формат транскриптов Anthropic объявила внутренним: «The entry format is internal to Claude Code and
changes between versions, so scripts that parse these files directly can break on any release»
(https://code.claude.com/docs/en/sessions#where-transcripts-are-stored). Формат дескрипторов Desktop
не документирован вовсе — только код бандла.

## Где стор и из чего состоит

- macOS: `<userData>/claude-code-sessions/<accountUuid>/<orgUuid>/`. В коде: `DB='claude-code-sessions'`,
  префикс `OB='local_'`, путь `userData/<store>/<accountId>/<orgId>` (`JFr`, `getStorageDir`).
  accountUuid `28c0ec84…` подтверждён `cowork-enabled-cli-ops.json` (`ownerAccountId`), orgUuid
  `9e0b6fd7…` — URL в `extensions-blocklist.json`.
- Windows — два варианта, зависит от установки: Squirrel/.exe → `%APPDATA%\Claude\claude-code-sessions`;
  MSIX (Store/WinGet/enterprise, а с апреля 2026 и «Download for Windows» с claude.ai — MSIX-бутстраппер,
  пакет `Claude_pzs8sxrjxfjjc`) → `%LOCALAPPDATA%\Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude\claude-code-sessions`.
  Настройки пути данных (вроде `CLAUDE_DATA_DIR`) нет (#57998 open). `init.ps1` (комментарий в строках 17–18,
  код в 63–78) ищет оба и junction-ит каждый найденный в OneDrive. Какая из установок реально в работе на GPD — не проверено.
- Содержимое каталога: `local_<uuid>.json` — дескриптор одной карточки вкладки Code; `deleted_<uuid>` —
  надгробие удалённой сессии, 13 байт, внутри epoch-ms (оба имеющихся — `1785405016562` = 2026-07-30 09:50Z);
  конфликт-копии OneDrive `local_<uuid>-GPD-WIN-MAX2.json`; рядом `scheduled-tasks.json` (125 байт, не
  дескриптор). Всего 168 файлов, 872 КБ на момент исследования (~11:00 MSK); при верификации 12:45 MSK —
  169 файлов, 884 КБ: добавилась карточка `local_f13ad1bd…` (VibeMemory, создана 09:39Z, см. ниже про реальный каталог).
- `local-agent-mode-sessions/<acct>/<org>/` — не Code-сессии, а Cowork/scheduled («Daily ai digest»,
  `sessionType: scheduled`, `systemPrompt` ~57 КБ, `emailAddress`); в OneDrive не синкается.
- Список сессий в GUI рисует веб-код claude.ai внутри Electron (в `app.asar` его нет); его состояние —
  Local Storage origin `https://claude.ai`: `local-code-sessions` (упорядоченный список local_-id),
  `epitaxy-session-result:local_<id>`, `epitaxy:code-session-durability:v1`, `LSS-code-recently-used-repos-<org>`.

## Дескриптор: поля и «горячесть»

По 165 файлам `local_*.json` (на 12:45 MSK их 166 — у свежей карточки VibeMemory ещё нет `completedTurns`,
т.е. «всегда» для него означает «после первого хода»; частоты ниже — по 165). Всегда (165/165):
`sessionId` (`local_<uuid>`), `cwd`, `originCwd`,
`createdAt`, `lastFocusedAt`, `lastActivityAt` (epoch-ms), `model`, `effort`, `isArchived`, `title`,
`permissionMode`, `remoteMcpServersConfig`, `completedTurns`, `alwaysAllowedReasons`,
`sessionPermissionUpdates`, `classifierSummaryEnabled`, `spawnSeed`. Часто: `titleSource` 157
(`auto|user`), `chromePermissionMode` 151, `cliSessionId` 149, `writtenBranches` 129,
`reportFindingsCard` 105, `promptSuggestion` 76, `sessionSettings` 55. Редко: `transcriptUnavailable` 20,
`isStarred` 20, `prs`/`prNumber`/`prUrl` 15–17, `bridgeSessionIds` и `remoteControlAutoEligible` 10,
`spawnedFrom` 5, `forkedFromSessionId` 3. Секретов нет.

- `cliSessionId` — uuid транскрипта CLI (`projects/<enc>/<cliSessionId>.jsonl`). У текущей сессии:
  `local_67c711c9…` → `e3bbbf16-d2a5-4fdf-a493-23555980a80c`.
- `originCwd` — папка, выбранная пользователем; `cwd` отличается только у изолированных worktree
  (`…/.claude/worktrees/<name>`, 6 дескрипторов). Код: при fallback `e.cwd=e.originCwd,e.worktreePath=void 0`.
  Пути абсолютные, ОС-специфичные, без трансляции: 23 дескриптора с `D:\…` лежат в общем сторе.
- Файл переписывается целиком при каждом фокусе (`lastFocusedAt`) и активности: дескриптор текущей
  сессии — mtime 09:39:35Z при `lastActivityAt` 09:39:34.655Z (замер 12:39 MSK; первый замер 12:30 MSK давал
  ту же секунду). Запись — `writeSessionToDisk` через атомарный писатель (по тексту #83584:
  `local_<id>.json.tmp`, flag `'wx'`, rename). Это и есть источник конфликт-копий в OneDrive.

## Загрузка стора (`loadSessionRecords`)

- Фильтр `e.startsWith('local_') && e.endsWith('.json')` — конфликт-копии грузятся наравне с
  оригиналами; файлы >10 МБ (`n.size>10485760`) и пустые (`!f.trim()`) пропускаются.
- Дедуп по внутреннему `sessionId`: `if(this.sessions.has(m.sessionId))return` — побеждает файл,
  прочитанный первым (readdir батчами через `Promise.all`), т.е. недетерминированно. Сегодня 165 файлов
  → 161 уникальных `sessionId`; четыре дубля — ровно конфликт-копии.
- `deletingSessionIds` в загрузчике (`if(this.deletingSessionIds.has(m.sessionId))return`) — набор сессий,
  удаляемых прямо сейчас в памяти, а не файлы `deleted_*`. Надгробия читает `Ks()` →
  `CliSessionDiscovery.getPersistedMarkers` (`tombstoned` + `recorded` cliSessionId): они исключают уже
  удалённые/известные транскрипты из обнаружения терминальных CLI-сессий и в дедупе дескрипторов не
  участвуют (уточнено верификатором по бандлу).
- Платформа не проверяется: Windows-дескрипторы на Mac загружаются, загрузчик сеет
  `folderExistsCache {exists:true}`, затем `getAllSessions` делает `stat(cwd)` (с TTL) и отдаёт UI
  `folderExists:false`; из «недавних папок» такие исключаются (`e.folderExists!==!1`); resume
  отказывает в `prepareSpawnCwd`: «Working directory no longer exists: <cwd>. The folder may have been
  moved, deleted, or unmounted.» Транскрипты этих 17 Windows-сессий с `cliSessionId` на Mac при этом
  физически доступны через ссылку `D--Projects-VibeCode-VibeIDE`, но до них дело не доходит.
- Единственный штатный способ переселить карточку на другой путь — `changeSessionCwd`
  (инструмент `change_directory`): local backend, не архив, не SSH, не worktree, не busy, папка
  существует, UNC запрещён, нужен workspace trust (`needs_trust`). CLI при этом переносит транскрипт в
  новый project-dir и дописывает `{type:"relocated",sessionId,relocatedCwd}` (`relocateSessionTranscript`).

## Дескриптор → транскрипт: сопоставление по `cliSessionId`

Основной путь resume — не чтение транскрипта приложением, а спавн встроенного CLI с процессным
cwd = `descriptor.cwd` и `--resume=<cliSessionId>`; собственные чтения у приложения тоже есть
(preflight/`get_transcript`, bring-home — см. ниже). Проверено `ps` pid 6278:
`…/claude-code/2.1.255/claude.app/Contents/MacOS/claude --output-format stream-json --input-format stream-json
--permission-prompt-tool stdio --resume=e3bbbf16-… --setting-sources=user,project,local --settings {}
--permission-mode auto`, а также `--verbose --effort low --model claude-opus-5 --allowedTools …`; родитель
`/Applications/Claude.app/Contents/Helpers/disclaimer`; env `CLAUDE_CONFIG_DIR=/Users/borodatych/.claude`,
`CLAUDE_CODE_ENTRYPOINT=claude-desktop`, `CLAUDE_CODE_HOST_SESSION_ID=local_67c711c9…`. `CLAUDE_CONFIG_DIR`
попадает в CLI из allowlist env `A_n=new Set(['PATH','CLAUDE_CONFIG_DIR','CLAUDE_CODE_TMPDIR',…])`.

Дальше ищет CLI (`zq()`), кандидаты по порядку: `linkScanPath` → каталог проекта для cwd
(`Ab`: вычисленный `projects/<enc(cwd)>` + скан по префиксу для длинных путей) → worktree-каталоги
(`via:"worktreeProjectDir"`) → полный скан `projects/*` по id (`via:"projectsScan"`) → «No conversation
found with session ID: …» (`failure_reason not_found_explicit_id`). enc =
`replace(/[^a-zA-Z0-9]/g,'-')`, при длине >200 — усечение до 200 + `-` + base36-хэш (CLI 2.1.255:
`Math.abs(t4(t)).toString(36)`, `t4` — цикл `(e<<5)-e+charCode|0`). В приложении та же формула и тот же хэш
(`qrn`/`cliProjectDirSlug`, `Jrn`/`cliSanitizeCwdSimple`), но для host-путей перед заменой ещё `normalize('NFC')`;
в CLI-функции NFC нет.

- **Правило единственности.** `projectsScan` (`jVt`): `if(a!==null)return null; a=o` — id, найденный в
  двух каталогах, даёт `null`, а не «любую копию». То же в доках: «a hand-copied duplicate makes Claude
  Code report not-found rather than resume an arbitrary copy»
  (https://code.claude.com/docs/en/sessions#resume-a-session).
- **Сканы слепы к симлинкам.** И `jVt` в CLI, и `QNr` в приложении фильтруют `Dirent.isDirectory()`,
  а для симлинка Node отдаёт `false`: `~/.claude/projects` = 28 записей, 28 symlink, 0 directory
  (`readdirSync(…,{withFileTypes:true})`; на 12:39 MSK — 29/28/1, см. VibeMemory ниже). Основной путь через
  `projects/<enc(cwd)>`-ссылку работает
  (текущая сессия так и возобновилась), поиск по id через ссылки не находит ничего (#46342 — пустой
  пикер `claude -r`, закрыт stale). Побочный факт: 223 id сегодня лежат более чем в одном каталоге
  `~/.claude/projects` (несколько ссылок на один `-ALL-/VibeIDE`) — будь это реальные каталоги,
  `projectsScan` вернул бы `null` для каждого.
- На Mac все 149 дескрипторов с `cliSessionId` (на момент исследования) разрешаются: 0 отсутствующих в
  `-ALL-`. При верификации 150-й (VibeMemory, `c059da60…`) лежит в **реальном** каталоге
  `~/.claude/projects/-Volumes-Storage-Projects-VibeCode-VibeMemory/` (создан 12:39 MSK): ссылки для этого
  cwd не было, и CLI завёл каталог сам — живая иллюстрация «первая сессия на новом cwd без ссылки уходит
  в реальный каталог».

## Транскрипт не найден: `transcriptUnavailable` и `clearStaleResumeHandle`

- Пометка ставит само приложение (сайты `warm_preflight` / `get_transcript`):
  `r.transcriptUnavailable=!0,this.saveSession(r)` + телеметрия `desktop_ccd_transcript_unavailable_marked` —
  когда транскрипт для `cliSessionId` подтверждённо отсутствует и буфер пуст, и только если
  `canTrustTranscriptMissingVerdict()` истинна: все наблюдаемые значения `CLAUDE_CONFIG_DIR` должны
  совпадать с путём приложения. Следствие (вывод судей): унификация `CLAUDE_CONFIG_DIR` во всех
  источниках (shell, launchctl/setx, Desktop Settings) как раз *включает* доверие к вердикту.
- При результате resume «not found» (`cli_resume_not_found_result` / `…_thrown`) `clearStaleResumeHandle`
  стирает привязку: `e.cliSessionId=void 0` и `e.unarchivedCliSessionId=void 0`. Обратного механизма
  в коде не найдено — карточка остаётся без транскрипта навсегда, даже когда файл потом появится.
- Дегидрированный транскрипт (OneDrive, `blocks=0` при `size>0`) на preflight даёт тот же исход —
  судьи называют это подтверждённым поведением, отдельного прогона в отчётах нет. Сегодня 67 из 353
  транскриптов верхнего уровня в `-ALL-` дегидрированы (`stat -f %b` = 0 при size>0; из 353 — 109 в сторе
  `-` для cwd=`/`, все гидрированы, без них 67 из 244; VibeIDE 27/80, EventHub 4/4, VibeReel 9/10,
  BuzzBang 9/11, Undercut 7/8, VSCodeSync 6/8, VibeSter 4/13, VibeReviewer 1/1; пересчитано верификатором).
- Цифры: 20 дескрипторов с `transcriptUnavailable`, 16 без `cliSessionId`. Пересчёт: все 16 без
  `cliSessionId` помечены; ещё 4 помечены при существующем транскрипте, из них 3 — конфликт-копии
  с Windows. Пары оригинал/копия: `local_077ceed4…` (VibeDub) отличается ровно одним ключом
  `transcriptUnavailable`; `02ea2773…` (Undercut) — им плюс
  `lastFocusedAt`/`lastActivityAt`/`completedTurns`/`promptSuggestion`; `f7905ca2…` (VibeReel) — то же плюс
  `remoteMcpServersConfig`; `244ba07c…` флага не имеет (расходятся только
  `completedTurns`/`lastFocusedAt`/`lastActivityAt`). Windows-машина не нашла Mac-транскрипты и пометила
  свои копии. Четвёртый помеченный при живом транскрипте — не копия: `local_5dee72e9…` (VibeShot S5,
  Mac-cwd).

## «Bring-home» копия транскрипта при resume

Код `index2 @431932`: на resume приложение считает ожидаемый каталог
`s = <config>/projects/<enc(realpath(cwd))>`, вызывает `diskTranscript.resolveProjectDirForSession(cliSessionId)`;
если транскрипт найден в другом каталоге `c` и `srcSize>dstSize` → `migrateTranscriptOnWorktreeFallback(…,
sourceDir:'shared-cli-project', keepSource:true)` → исход `'migrated'` (при нехватке бюджета времени —
`'floor_skipped'`/`'migrate_failed'`/`'timed_out'`); иначе, если в ожидаемом каталоге файл уже есть —
`'repinned'`; при совпадении каталогов `'home'`; телеметрия `desktop_ccd_resume_bring_home_timing`.
`keepSource:true` (таблица режимов `Tl`) — источник не удаляется, т.е. это копия, а не перенос: один
`cliSessionId` оказывается в двух каталогах.

- При симлинк-схеме владельца скан не видит ссылок, исход `'skipped'`, копий нет — вероятно
  (по коду, отдельно не прогонялось). При реальных каталогах в `projects/` приложение само размножает
  транскрипты, а дубль по правилу единственности даёт `projectsScan → null` → `cli_resume_not_found` →
  `clearStaleResumeHandle` — цепочка выведена судьями из кода, на живом сценарии не воспроизведена.
- Маркеры `<cliSessionId>.desktop-released.json` (`v`, `releasedAt`, `reason`) приложение пишет рядом с
  транскриптом, когда «отпускает» сессию, но только если `realpath(dir)` не выходит за
  `realpath(<config>/projects)` — иначе warn «desktop-released marker: resolved dir escapes the projects
  root; skipping». Для всех OneDrive-ссылок это так: в `-ALL-` таких маркеров 0.

## Политика «components under the config root may not be symlinks»

Слой private-file приложения перед create/write/mkdir (`qce`/`Hr`/`tle`/`ti` через `await $r(e)`) проверяет
каждый промежуточный компонент ниже «co-writable boundary» (корень = `CLAUDE_CONFIG_DIR`, зарегистрированные
корни или `/[\\/]\.claude(?=[\\/]|$)/`) и бросает `PlantDetectedError`: «symlink at a non-leaf component below
the co-writable boundary: … — components under the config root may not be symlinks; supported relocations:
symlink the whole config root at the home level (~/.claude) or point CLAUDE_CONFIG_DIR at the relocated
directory via Desktop Settings». Для чтений (`ni`) режим `onBelowBoundarySymlink:'warnAndRead'` —
предупреждение и чтение.

Что политика терпит сегодня: транскрипты через симлинк `projects/<enc>` пишутся и возобновляются
(текущая сессия), единственный видимый эффект — пропуск `.desktop-released.json`. Какие именно файлы
проходят через этот слой — полностью не перечислено; любая будущая запись Desktop под `projects/<enc>/`
на ссылочной раскладке молча провалится. Проверка — `PlantDetectedError` в логах Desktop.

Отдельно Desktop проверяет идентичность самого файла транскрипта (`lstat` vs `fstat`, dev/ino, volume
serial) и отказывается грузить симлинк на файле, FIFO или reparse point с несовпадающим volume serial
(`ETRANSCRIPTNOTREGULAR`; #90903 open, Desktop 1.40609.0 MSIX: «[CCD] Not loading transcript <id>.jsonl:
the path is not a plain regular file»; строки `NOTREGULAR`/«plain regular file» есть в `app.asar` 1.40609.1,
в извлечённых чанках их нет — описание механизма по тексту issue). Симлинк допустим на уровне каталога,
не на `.jsonl`. Плейсхолдеры OneDrive Files On-Demand —
reparse points даже в pinned-состоянии; как этот чек ведёт себя на них на Windows — не проверено.

## CLI-процесс живёт, пока открыта карточка

- Карточка `local_67c711c9…` создана 07:24:46.761Z (`createdAt 1788333886761`); первая запись транскрипта
  `queue-operation` 07:24:47.099Z. Её CLI-процесс pid 6278 запущен 07:49:43Z (`ps lstart`, это уже
  перезапуск с `--resume`) и в 09:34Z всё ещё жив (elapsed 1:44:43). Вторая карточка — pid 11657
  (Promed) с 08:23:14Z. Оба зарегистрированы в `~/.claude/sessions/<pid>.json`
  (`entrypoint: claude-desktop`, `kind: interactive`, `pidDomain: darwin`, `version 2.1.255`); на 12:45 MSK
  живых Desktop-CLI три — добавился pid 17546 (VibeMemory, 12:39 MSK).
- `lsof` по обоим pid не показывает открытых `.jsonl`: CLI пишет транскрипт open/append/close по пути,
  а не через удерживаемый fd. Разговор при этом живёт в памяти процесса и с диска не перечитывается —
  дописанное извне в такую вкладку не попадёт до перезапуска карточки (вывод судей по коду, вживую не прогонялось).
- «Вкладки живут днями/неделями» — практика владельца и утверждение судей, длительность не измерялась;
  измерено только то, что Desktop у владельца открыт постоянно (все сессии с 14.08 — `claude-desktop`).

## Перечитывает ли Desktop стор без рестарта — открыто

Найдена только загрузка на старте (`loadSessionRecords`) и запись при изменениях; вотчера каталога
исследователи в бандлах не искали. Grep верификатора по всем извлечённым чанкам `app.asar`:
`fs.watch`/`watchFile`/`chokidar` — 4 вхождения, все на одиночный файл (вотчер файла настроек в
`index.chunk-jfL6G7d3.js` и size-probe одного пути в `index2.chunk-CIOndNUk.js` для `CliSessionDiscovery`);
вотчера каталога `claude-code-sessions` нет. Вживую не проверено. Оба следствия — «файл, подложенный при работающем Desktop,
появится в списке только после рестарта» и «Desktop перепишет известный ему дескриптор копией из
памяти» — гипотезы, на которых построен синтез. Судьи считают, что новые (неизвестные приложению)
файлы класть безопасно, поскольку перезаписать можно только то, что уже загружено, — тоже не проверено.

## Windows: junction в MSIX не работает по определению

- В MSIX действует AppData write virtualization: любой junction/симлинк под `%APPDATA%\Claude` ломает
  атомарную запись карточек — `writeSessionToDisk` пишет `local_<id>.json.tmp` с flag `'wx'` (ложный
  EEXIST; детали tmp/flag — по тексту #83584, в бандле виден вызов атомарного писателя), rename → EXDEV
  даже на одном диске; сессии не сохраняются и исчезают из сайдбара
  (#83584 open, #91409 open — 693 ошибки за два дня, #48362, #32533, #57998: «NTFS junctions and directory
  symlinks fail — Node's fs.rename() resolves real paths»). Значит junction `claude-code-sessions → OneDrive`
  из `init.ps1` на MSIX-установке не может работать; на Squirrel — только в пределах одного тома
  (иначе EXDEV) и без гарантий поверх плейсхолдеров OneDrive (+R, reparse).
- Слияние конфликт-копий в общем сторе делает OneDrive, а не приложение: у него нет ни стратегии, ни
  предпочтения новейшего — см. «первый прочитанный побеждает» выше.

## Что ещё есть в коде

- Экспорт/импорт сессий за фиче-флагами: zip с манифестом `claude-3p-export.json` («export this computer's
  chats, Cowork tasks, and Code sessions as a zip another install can import»), источники импорта
  `localZip|local1P|local3PBundle|terminalCli|remotePull`, staging `imported-staging`, классификатор папки
  (`userData`, если есть `claude-code-sessions`/`local-agent-mode-sessions`; `projects`, если есть
  `projects/`), для `terminalCli` — адоптация транскриптов с перемаппингом home-путей. Единственный
  официальный механизм «сессии на другой машине», найденный в коде; включён ли для аккаунта — не проверено.
- `bridgeSessionIds`/`remoteControlAutoEligible` в дескрипторе + IPC `findLocalSessionIdForBridgeId` —
  связь карточки с Remote Control; серверный id в `sessions/<pid>.json` (`bridgeSessionId session_01…`)
  и в транскрипте (`type: bridge-session`, `cse_01…`).
- Retention: транскрипты сессий, начатых или продолженных в Desktop/Cowork, с CLI 2.1.248 не удаляются
  `cleanupPeriodDays`, если не задан `desktopSessionCleanupPeriodDays`
  (https://code.claude.com/docs/en/settings-reference#desktopsessioncleanupperioddays).
- Desktop и CLI ведут раздельные истории: «Each maintains separate session history … To move a CLI session
  into Desktop, run /desktop in the terminal» (https://code.claude.com/docs/en/desktop#coming-from-the-cli);
  `/desktop` первой командой падает «CLI session transcript not found» (#62017) — транскрипт рождается
  только с первым промптом. Синхронизацию Desktop↔CLI просят в #28791 (open, 0 ответов Anthropic).

## Цифры на 2026-09-02 (пересчитаны по стору)

Пересчёт верификатора 12:45 MSK: `local_*.json` 166 (162 уникальных), с `cliSessionId` 150 (149 в `-ALL-`
+ 1 в реальном каталоге VibeMemory), `bridgeSessionIds`/`remoteControlAutoEligible` 11, `isStarred` 21,
живых Desktop-CLI процессов 3 (+17546 VibeMemory); остальные строки без изменений.

| Что | Сколько |
|---|---|
| `local_*.json` | 165 (161 уникальных `sessionId`) |
| конфликт-копии `-GPD-WIN-MAX2` | 4: `02ea2773`, `077ceed4`, `244ba07c`, `f7905ca2` |
| надгробия `deleted_*` | 2 |
| с `cliSessionId` / транскрипт найден | 149 / 149 |
| без `cliSessionId` | 16 (6 с Windows-cwd; у desktop-исследователя — «в т.ч. 4 Windows», пересчёт по `cwd` `D:` даёт 6) |
| `transcriptUnavailable` | 20 (16 без `cliSessionId` + 4 при живом транскрипте: 3 конфликт-копии + `local_5dee72e9…` VibeShot) |
| cwd `D:\…` | 23 (21 VibeIDE, 1 EventHub, 1 worktree) |
| изолированные worktree (`cwd ≠ originCwd`) | 6 (у desktop-исследователя — 4; пересчёт даёт 6, включая один Windows) |
| с `bridgeSessionIds` | 10 |
| живых Desktop-CLI процессов | 2 (pid 6278 VibeSweep, 11657 Promed) |
