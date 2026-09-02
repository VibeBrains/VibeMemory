# Ограничения движка из 64 фатальных находок судей (2026-09-02)

Три дизайна синка `~/.claude` между Mac и Windows — cloud-first («CloudRoot»: весь `CLAUDE_CONFIG_DIR`
в OneDrive), local-hot-auto-durable («AutoLink + Outbox»: ссылка по SessionStart, outbox на машину) и
git-transport («ClaudeSync», рабочее имя движка, ставшего VibeMemory) — прошли три судейские панели
(data-safety, ergonomics, operations). Фатальных находок: cloud-first 19, local-hot 24, git-transport 21,
итого 64 — [judgeFindings.md](../research/judgeFindings.md); фактура —
[researchReport.md](../research/researchReport.md) (метки `[verified]`/`[likely]`/`[unverified]`
перенесены смыслом), итог — [synthesisReport.md](../research/synthesisReport.md). Ниже — что из этого
обязан и чего никогда не делает движок; у каждого пункта — почему и откуда. Перепроверено на этой
машине 2026-09-02 ~12:50 (read-only): `~/.claude/projects` — 28 ссылок и 1 реальный каталог
`-Volumes-Storage-Projects-VibeCode-VibeMemory` (создан 12:39 первой сессией VibeMemory в cwd без ссылки —
движка ещё нет, CLI сделал реальный каталог; при написании записи было 28/0); три живых Desktop-CLI 2.1.255
(pid 6278 VibeSweep, 11657 Promed, 17546 VibeMemory с 12:39), `lsof` — 0 открытых `.jsonl` у всех трёх;
реестр `sessions/<pid>.json` несёт `procStart` и `pidDomain`; git 2.50.1, python 3.9.6, `git-lfs` и
`gitleaks` не установлены; текущий транскрипт — 563 строки, 197 без `uuid`, 17 немонотонных пар
`timestamp` из 395, 0 неразбираемых (числа растут с каждым ходом; при написании было 535/190/17 из 375).

## Старт сессии: ссылка — первым шагом и без сети

- **Обязан** создать `projects/<enc>` → стор первым действием SessionStart-хука, до лока, fetch, merge и
  push, и всегда выходить с кодом 0. Почему: CLI ждёт завершения SessionStart перед первой записью, а
  транскрипт рождается после выхода хука (racetest в режиме Desktop-спавна `--output-format stream-json`:
  хук завершился 11:59:26.376, транскрипт родился 11:59:26.392 → +16 мс; `-p`: +4 с; issue #62017 —
  `/desktop` первой командой не находит транскрипт). В git-дизайне ensure-link стоял пятым шагом — после
  ожидания лока до 20 с, fetch до 15 с, merge и push внутри таймаута 45 с; по таймауту CLI отбрасывает
  хук и через ~4 с создаёт реальный каталог — сессия форкается до конца жизни (судьи ergonomics и
  data-safety по git). Интерактивный TTY не прогонялся — не проверено; у Desktop-сессии первая запись
  легла через 338 мс после `createdAt`.
- **Никогда** не ходит в сеть из SessionStart. Почему: первый ssh без known_hosts, Defender на дереве
  500 МБ, каждый офлайн-старт ждёт таймаут fetch (operations по git). Сеть — в async-хуках и tick.
- **Обязан** брать `enc` из `transcript_path` (`basename(dirname())`), а не переизобретать кодировку.
  Почему: формула уже менялась (2.1.224: >200 символов → усечение + base36-хэш, NFC), а Windows-CLI
  ключует по `realpathSync` без разворота junction/subst, тогда как Desktop/VS Code — по native realpath
  (#71224, #78461 открыты) — ссылка нужна под тем слагом, который вычислил именно этот CLI.
  Уточнение 2026-09-02 (реализация ядра): формула воспроизведена в `vibememory-core` по strings
  обоих бинарей и пришпилена оракулом `fixtures/naming/encodeCwdOracle.js` — но допустима ровно в
  одном месте, реконсилере тика, как **предсказание** для cwd с другой машины, где `transcript_path`
  нет по определению; предсказанная ссылка сверяется с `transcript_path` на первом SessionStart,
  никогда не удаляется и не считается подтверждённой для гейта Desktop. Факты: CLI сам подаёт в
  формулу `NFC(realpath(cwd))` (2.1.232 — прогон, 2.1.255 — strings и прогон верификатора), поэтому
  ядро делает NFC, а realpath — CLI; `CLAUDE_CODE_PROJECT_DIR_NAME` принимает только
  `[A-Za-z0-9_-]{1,64}` минус device-имена и молча откатывается на формулу, значит enc всегда
  `[A-Za-z0-9_-]+` — [claudeCode/projectDirEncoding.md](../claudeCode/projectDirEncoding.md).

## Живая сессия: никогда не перелинковывать

- **Никогда** не переключает ссылку под живой сессией. Rename реального каталога в ссылку — только при
  `source=startup`, когда `<session_id>.jsonl` там ещё нет и в `sessions/*.json` нет живого pid с этим cwd
  (pid + procStart); реестр не читается или не парсится → считать живым (fail-closed); иначе copy-import
  по uuid + pending на tick. Почему: прецедент 2026-08-08 (4 версии одного транскрипта); caveat claude-mv
  («don't run from a session whose own directory is being moved»); формат реестра недокументирован —
  в 2.1.232 нет `pidDomain`, в 2.1.255 есть, при смене формата интерлок «отказывает в открытую»
  (operations по git).
- Посылка «CLI держит открытый fd» опровергнута: `lsof` на pid 6278, 11657 и 17546 показывает 0 `.jsonl` —
  CLI пишет open/append/close по пути. Опасность иная: после rename следующий append создаст новый файл по
  старому пути (форк), а разговор, загруженный в память, обновить нельзя (data-safety по cloud-first и
  git). Прецедент 08.08 при этом остаётся необъяснённым до конца — помечено судьёй как открытое.
- **Никогда** не сливает в файлы сессий, живых на этой машине, и никогда не пишет `.claude.json`
  (seedTrust). Почему: каскад порчи #28847/#29153 (35 КБ → 77 Б), файл переписывается ≥5 раз за 27 минут
  двумя клиентами; ценность нулевая — Desktop-сессии работают при `hasTrustDialogAccepted=false` (7 из 8
  путей `/Volumes/Storage/*` при исследовании; при перепроверке 7 из 9 — добавился VibeMemory),
  `allowedTools`/`mcpServers` пусты во всех проектах (25 при исследовании, 26 при перепроверке). Все три
  судьи по local-hot: убрать, не гейтить.

## Межмашинная живость: heartbeat в сторе

- **Обязан** писать `machines/<id>/live.json` (sid → heartbeat, cwd) на SessionStart/Stop/SessionEnd и
  `tails.json` (sid → {lastUuid, lines, bytes, at}); sid с heartbeat моложе 30 мин на любой машине —
  неприкосновенен. Почему: `sessions/*.json` по построению не покидает машину, сессия другой машины для
  вотчера невидима — «живых pid нет» и канонический транскрипт переписывается под чужим append
  (data-safety по local-hot); хуже — если `sessions/` синхронизировать, CLI на машине B при старте удалит `sessions/<pid>.json`
  машины A как crash leftover (строки есть и в 2.1.232, и в 2.1.255: `isProcessProvablyGone`/`dead-owner`/
  `sweepPermitted`; доки: «clears crash leftovers on the next launch»; межмашинный сценарий — вывод судьи
  operations по cloud-first, не измерен), и метаданные защиты исчезают раньше, чем ими воспользуются.
  Desktop держит CLI-процесс живым для каждой открытой карточки часами (pid 6278: карточка Desktop создана
  07:24:46Z, сам процесс — с 07:49:43Z, т.е. 10:49 по местному, и жив на 12:50; судья писал «с 07:24»).
- **Никогда** не синхронизирует `sessions/` и `*.key` (pid, сокет `/tmp/cc-socks/<pid>.sock`, peerToken).
- Остаточный риск: вкладка Desktop открыта неделями → heartbeat протухает (>30 мин), остаётся только гейт
  по `tails.json` при следующем вводе — работает, но зависит от доставки push с другой машины.

## Свежесть: UserPromptSubmit-гейт вместо таймера

- **Обязан** на первом промпте сессии и далее раз в >5 мин делать `git fetch` (таймаут 5 с, офлайн →
  пропуск с пометкой); если `origin/main` меняет `projects/*/<sid>.jsonl` или `live.json` показывает sid
  живым на другой машине — слить файл на диске и заблокировать промпт («сессию продолжили на `<машина>`:
  закрой и открой заново»). Почему: SessionStart(resume) срабатывает после того, как CLI загрузил
  транскрипт в память, а merge под резюмируемой сессией отложен — ровно в целевом сценарии «закрыл
  крышку A, открыл B» B продолжает устаревшую копию (data-safety по git); CLI 2.1.255 (`fAr`: перебирает
  `leafUuids`, пропускает sidechain, берёт запись с max(`timestamp`) — сверено по строкам бинаря) выбирает
  лист с max(timestamp), ветка одной машины исчезает из видимого разговора. Это единственное место, где
  допустим ритуал, и только в гонке; вне гонки — no-op.
- **Никогда** не полагается на таймер для «pull перед resume». Почему: на Mac нет триггера «по
  пробуждению» — launchd `StartInterval` во сне пропускается (`man launchd.plist`: «that interval will be
  missed due to shortcomings in kqueue»), окно до 10 мин в git-дизайне; на Windows unlock-триггер есть.

## Слияние: keep-both для памяти, union для JSONL, abort при сбое

- **`memory/*.md` — keep-both, никогда LWW.** Своя версия остаётся, чужая → `~/.claude-sync/quarantine/`,
  `additionalContext` «сведи память». Почему: авто-память — единственный общий артефакт, который обе
  машины легитимно правят в каждой сессии; при «новее побеждает» проигравший `MEMORY.md` уезжает в
  `.conflicts`, куда модель не смотрит — тихая потеря памяти (ergonomics по local-hot); конфликт-копия
  `VibeIDE/memory/MEMORY-GPD-WIN-MAX2.md` (31.07) уже лежит в сторе; auto memory официально
  machine-local, нативного слияния нет.
- **JSONL: union по uuid, строки без uuid — побайтно, стабильная сортировка; снимок — до последнего
  `\n`; не парсится — байтовый режим, не порча.** Почему: `git add` живого файла может захватить оборванную
  строку (O_APPEND многомегабайтных tool-results не атомарен), и byte-дедуп сохранит обрывок как
  «уникальную строку» посреди файла (ergonomics по git); формат объявлен внутренним («can break on any
  release»); строк без uuid в живом транскрипте 142 из 423 на момент суда (queue-operation, custom-title,
  bridge-session, last-prompt, ai-title, atis-latch, agent-name, mode), при перепроверке ~12:50 — 197 из 563.
- **Обязан** иметь тест-гейт драйвера на фикстурах текущего формата (queue-operation, bridge-session,
  relocated, summary без uuid, обрывок строки) и прогонять его на каждую новую версию CLI.
- **Никогда** не ставит гейт приёмки «timestamp монотонны, нет осиротевших parentUuid». Почему:
  опровергнуто на чистых одномашинных файлах — VibeSweep e3bbbf16: 13 немонотонных пар из 304 и один
  сирота (tool_result, строка 148 — пересчитано); VibeShot 7bad7d59: 75 из 1785 (пересчитано); при
  перепроверке ~12:50 — 17 из 395. Такой гейт отклонит почти каждое слияние (data-safety по local-hot). Валидировать только парсимость и покрытие всех uuid.
- **Обязан** делать `git merge --abort` при любом исключении драйвера и **никогда** не коммитить при
  `MERGE_HEAD` / непустом `git ls-files -u`. Почему: упавший драйвер (нет Python, новый формат, обрывок)
  оставляет `MERGE_HEAD` с локальной версией в дереве, а `git add -A && git commit` следующего хука
  фиксирует «ours» как разрешение — строки другой стороны выпадают из линии слияния, восстановимо только
  из reflog (ergonomics и operations по git). Стейл `index.lock` и mkdir-лок снимаются только по проверке
  живости владельца — лок несёт pid.
- **Обязан** покрыть драйвером каждый тип под `projects/` (`* merge=…keepboth`, `**/*.jsonl merge=…jsonl`).
  Почему: в git-дизайне `.project.json`, `custom-title.json`, `workflows/*.json`, `tool-results/*.txt` шли
  дефолтным 3-way → маркеры `<<<<<<<` в дереве, которые Stop-хук коммитит и пушит (data-safety по git).
  Всё, что не транскрипт и не память, — outbox одного писателя `machines/<id>/…`, конфликтов нет по
  построению.
- **Никогда** не использует LFS. Почему: драйвер получает от git содержимое блобов — pointer-файлы, union
  двух указателей — мусор; сбой smudge оставляет 130-байтный pointer, CLI читает пустой транскрипт,
  Desktop ставит `transcriptUnavailable` и сбрасывает `cliSessionId`; LFS хранит каждую версию целиком —
  сессия с транскриптом 45 МБ при покоммитной записи исчерпает 1 ГБ бесплатной квоты за ~20 ходов, и
  GitHub блокирует push всего репо (data-safety по git). Вместо LFS: файлы ≥45 МБ не коммитятся (сегодня
  один — 64 MiB tool-result, производный, для resume не нужен).
- **Никогда** не ставит gitleaks гейтом на push `projects/**`. Почему: транскрипты полны JWT/base64/env —
  ложные срабатывания штатны, каждое молча останавливает репликацию до следующего старта (ergonomics по
  git). Только scan-and-warn в `status`.

## Часы двух машин расходятся

- **Никогда** не выводит причинность из `timestamp` между машинами. Почему: union-переписанный
  транскрипт — синтетический файл, который ни одна версия CLI не писала; сортировка по часам GPD и Mac
  ломает порядок относительно `parentUuid` (data-safety по git). Сортировка стабильная, тай-брейк
  детерминирован, uuid/parentUuid — единственная истина о порядке.
- **Никогда** не решает LWW по mtime ничего. Почему: Desktop переписывает `lastFocusedAt` при каждом
  фокусе, а зеркало MSIX-стора «новее побеждает» с надгробиями в обе стороны — классическая тихая потеря
  (data-safety и operations по cloud-first).
- **Обязан** после вилки честно сказать в `additionalContext`: CLI покажет цепочку от листа с
  max(timestamp), вторая ветвь в файле, но не в разговоре — `/rewind` или fork. Почему: для владельца
  архивированная ветвь неотличима от потери (operations по local-hot).

## Окно потери при «закрыл крышку»: commit сразу на Stop, откладывать только push

- **Обязан** в Stop-хуке (`async: true`) коммитить немедленно (сотни мс), писать heartbeat и хвост, и лишь
  push дебаунсить (20 с); SessionEnd — commit синхронно (timeout 60), push отсоединённым процессом.
  Почему: Desktop при выходе может убить CLI без SessionEnd (утверждение дизайна и судей, не измерено), а
  дебаунс 30 с перед commit+push в git-дизайне при закрытой крышке оставлял последние ходы на A; tick спящей
  машины не идёт (ergonomics и data-safety по git).
- **Обязан** в Stop-хуке копировать `<sid>.jsonl` в стор (tmp+rename), если транскрипт родился в реальном
  каталоге (хук проиграл гонку). Почему: иначе вкладка Desktop живёт днями, а сессия невидима другой
  машине до конца жизни (ergonomics по local-hot).

## Дескрипторы Desktop: guard на экспорте, не в merge-драйвере

- **Обязан** держать guard на экспорте: дескриптор, потерявший `cliSessionId` или получивший
  `transcriptUnavailable` относительно последней экспортированной версии, не экспортируется, а локально
  чинится из outbox (shadow `cliSessionId`). Почему: Desktop при not-found ставит `transcriptUnavailable`
  и `clearStaleResumeHandle` стирает `cliSessionId` навсегда (сегодня 20 помечены, 16 без id); драйвер
  `…-newest` с coalesce git вызывает только при конфликте — односторонняя правка на B уходит fast-forward
  и перетирает хорошую копию A (ergonomics и data-safety по git). Единый `CLAUDE_CONFIG_DIR` во всех
  источниках, который движок требует, включает `canTrustTranscriptMissingVerdict` — вердикту поверят.
- **Обязан** импортировать новые дескрипторы в любое время (Desktop не перепишет того, чего не загружал),
  обновления известных — с оговоркой про копию из памяти; **никогда** не требует «Desktop закрыт».
  Почему: Desktop у владельца живёт постоянно — все сессии с 14.08 идут через него; «выйти → дождаться
  tick → перезапустить» и есть ритуал (все три судьи по local-hot). Перечитывает ли Desktop стор без
  рестарта — открыто.
- **Никогда** не кладёт reparse-точки под деревом Desktop: стор дескрипторов — реальный каталог. Почему:
  в MSIX любой junction/symlink под AppData ломает атомарную запись `local_<id>.json.tmp` (`wx` → EEXIST,
  rename → EXDEV) — #83584, #91409, #48362; карточки не сохраняются и исчезают из сайдбара.

## Имя стора

- **Обязан** выводить имя от `git rev-parse --path-format=absolute --git-common-dir`:
  `basename(dirname(commondir))` для репо, worktree и подкаталогов; сабмодуль — последний компонент
  пути после `.git/modules/` (имя сабмодуля; standalone-клон того же репозитория — отдельный стор);
  вне git — `basename(cwd)`; корень ФС — ошибка `rootCwd`, лечение только `ignoreCwd` (реализация
  2026-09-02 заменила унаследованное от git-дизайна `cwd=/ → root`: у строки не было своего «почему»,
  а стор `root` слил бы `/` Mac, `D:\` и UNC-корни всех машин в одну память; единственный реальный
  корень — раннер VibeDub — владелец игнорирует); коллизии — `nameOverrides` в
  `config.json`. Почему: `--show-toplevel` в linked worktree возвращает сам worktree → отдельные сторы
  `.claude/worktrees/<name>`, невидимые с другой машины (перепроверено: 6 дескрипторов с cwd под
  `.claude/worktrees/` — 5 VibeIDE, 1 Undercut, у всех `originCwd` = корень репо; researchReport насчитал
  4, судья git — 6);
  без `--path-format=absolute` git отдаёт относительный путь — проверено: `VibeDub/server` → `../.git`,
  абсолютно → `/…/VibeDub/.git` (ergonomics по local-hot, data-safety и operations по git). Сабмодуль проверен на
  VibeIDEA: commondir = `.git/modules/vibe-plugins/vibe-agent/resources/vibeDefaults`, то есть
  `basename(dirname())` дал бы `resources` — нужна отдельная ветка, а не общая формула.
- Край, найденный при проверке (в источниках его нет; перепроверено 2026-09-02): `VibeIDE/.claude/worktrees/
  recursing-cannon-5da42a/.git` = `gitdir: D:/Projects/VibeCode/VibeIDE/.git/worktrees/…` — worktree создан
  на Windows, на Mac `git rev-parse` в нём падает с `fatal: not a git repository`, и фолбэк `basename(cwd)`
  дал бы стор `recursing-cannon-5da42a`. **Закрыт правилом** (2026-09-02): при отказе git `.git`-файл
  разбирается в ядре — `gitdir:` (относительный — от каталога файла), хвостовая пара `worktrees/<id>`
  отбрасывается, далее те же правила commondir → `VibeIDE`; `.git`-каталог при отказе git → ошибка
  `gitRefused`, никогда basename. Фикстурный тест на worktree/подкаталог/сабмодуль/не-git —
  `fixtures/naming/resolveStoreName.json`.
- **Обязан** принимать commondir от git, только если он объясняет ближайшую запись `.git` над cwd
  (каталог равен commondir, либо gitdir из файла после отбрасывания `worktrees/<id>` равен
  commondir); иначе — ошибка `gitLayoutMismatch`. Почему: git 2.50.1 молча проходит сквозь битый
  (`HEAD` = мусор) или пустой вложенный `.git` к внешнему репо с exit 0 и слушается утёкшего
  `GIT_DIR` — проекты уехали бы в чужой стор ([git/repoLayoutEdges.md](../git/repoLayoutEdges.md)).
  CLI спавнит git со снятым окружением `GIT_*` (кроме `GIT_EXEC_PATH`) и `GIT_CONFIG_*`. Bare,
  `--separate-git-dir` → `unrecognizedGitLayout`; git не запустился → `gitUnavailable`; обход `.git`
  не завершился → `dotGitProbeFailed` — всё ошибки, не basename.
- **Обязан** сравнивать компоненты путей после NFC (git печатает форму на диске — NFD на APFS,
  CLI отдаёт NFC; байтовое сравнение даёт ложный `gitLayoutMismatch` на любом не-ASCII пути) и
  выводить имена сторов в NFC. **Обязан** считать имена, равные после NFC и свёртки регистра,
  одним стором и переиспользовать существующее написание (список существующих — из дерева git,
  не readdir: APFS схлопывает регистр); два существующих написания одного ключа →
  `storeNameCollision`. Почему:
  `VibeIDE` и `vibeide` сливаются в дереве git без конфликта, а на APFS/NTFS ложатся в один
  каталог — `MEMORY.md` перезаписан, `git add -A` навсегда падает на file alias (прогон
  верификатора). Имя валидируется для APFS/NTFS/git (`.git`, `CON`, `aux.js`, хвостовая точка,
  255 байт) — невалидное имя падает здесь, а не ломает клон на второй машине.
- `nameOverrides` и `ignoreCwd` — один glob-диалект (литерал = точное совпадение, поддерево —
  явным `/**`); override без литерального компонента (`/`, `/**`, `D:/**`) и `\` в шаблоне →
  `configInvalid`; два override на один cwd → `ambiguousOverride`. Почему: префиксная семантика с
  приоритетом над git превращала бы опечатку в одном ключе в ложное слияние всех проектов машины.
- `CLAUDE_CODE_PROJECT_DIR_NAME`, не равная имени стора, → ничего не линковать (`Ignored`):
  Desktop ставит `session` для 3p-сессий — один каталог на все cwd.
- Остаточный риск: одноимённые не-git каталоги и одноимённые сабмодули разных суперпроектов на
  двух машинах сливаются в один стор и одну память (VibeSweep — не git, проверено); `nameOverrides` —
  ручной предохранитель; doctor предупреждает о сабмодуле, питаемом разными суперпроектами (этап 2).
- **Никогда** не переписывает содержимое транскриптов; перевод путей `{ROOT}/rel` — только в данных
  outbox (`history.project`, `cwd/originCwd` дескрипторов).

## Удаление: `.keep`, `cleanupPeriodDays` явно, push-guard

- **Обязан** класть `.keep` в каждый стор. Почему: недокументированный rmdir-свипер
  (https://github.com/anthropics/claude-code/issues/86952, bcherny на 2.1.233: rmdir каждого каталога
  под `projects/`, ≤1 раз в сутки, независимо от `cleanupPeriodDays`; любой файл внутри защищает);
  прецедент 2026-07-25 — 11 висячих ссылок после исчезновения пустых сторов — свипер назван в researchReport
  лишь самым вероятным объяснением (`[likely]`), причинно-следственная связь не доказана.
- **Обязан** ставить `cleanupPeriodDays: 3650` явно и валидировать settings.json на zod-валидность.
  Почему: свип делает unlink мимо корзины по mtime (#59248); невалидный settings.json → дефолт 30 дней
  даже с заданным значением (https://github.com/anthropics/claude-code/issues/41458) — но в строках 2.1.232
  и 2.1.255 есть «Skipping cleanup: a settings file could not be read or parsed…», то есть свежие сборки на
  нечитаемом settings.json свип пропускают (fail-closed; судья data-safety по cloud-first), а #41458 —
  про более старые; обе версии оставлены. 17 транскриптов старше 30 дней уцелели сегодня (пересчитано) —
  по наблюдению researchReport свип не ходит через симлинки, недокументированно; Desktop-транскрипты
  хранятся бессрочно лишь с 2.1.248, а Windows пока на 2.1.227/229 — там ни fail-closed, ни бессрочное
  хранение не проверены.
- **Обязан** держать push-guard: удалённые в рабочей копии файлы `projects/**` восстанавливаются
  `git checkout --`, удаление — только `forget <sid>`. Почему: свип, `claude project purge`, удаление в
  Desktop, шифровальщик — уезжают на вторую машину за секунды; зеркальный «бэкап» `rsync --delete` /
  `robocopy /MIR` реплицирует удаление за сутки (data-safety по cloud-first); история git — второй слой.

## Секреты и `.claude.json` никогда не покидают машину

- **Никогда** не экспортирует: `.claude*.json*`, `backups/`, Keychain / `.credentials.json`,
  `sessions/` и `*.key`, `session-env/`, `shell-snapshots/`, `telemetry/`, `debug/`, `cache/`, `plugins/`,
  `mcp-needs-auth-cache.json`, `policy-limits.json`, `remote-settings.json`, `stats-cache.json`,
  `.last-*`, `local-agent-mode-sessions/` (Cowork: systemPrompt + email), `ant-device-registry.json`.
  Почему: в OneDrive с 6 июля лежит `.credentials.json` с mcpOAuth-токенами 8 серверов; `sessions/*.key`
  несут peerToken (проверено по ключам `.key`: `peerToken`, `pidDomain`, `procStart`), `.claude.json` —
  pid живого процесса в `replBridgePlaceholders` (в локальном Mac-файле ключ есть; в Windows-копии в
  OneDrive его нет — проверено; «pid чужой машины» — что случится при синке файла); близкий список зашит
  Anthropic в бинарь 2.1.232 (`rOE`: `.claude.json`, `.claude.json.backup`, `.credentials.json`, `projects`,
  `sessions`, `todos`, `shell-snapshots`, `statsig`, `file-history`, `history.jsonl`, `ide`, `logs`,
  `backups`, `.session_ingress_token` + маска `.claude(-suffix)?.json*`) как исключения снапшота конфига —
  в нём нет `session-env/`, `telemetry/`, `debug/`, `cache/`, `plugins/`, зато есть `projects` и
  `history.jsonl`, которые движок как раз экспортирует; cloud-first обнаруживал секрет уже
  после выгрузки (fails open: всё, что Anthropic добавит в конфиг-дир, само едет в облако), а
  `CLAUDE_SECURESTORAGE_CONFIG_DIR` недокументирована и на Windows-сборках 2.1.227/229 не проверена.
- **Обязан** держать защитный `.gitignore` (чёрный список) плюс белый список экспорта; миграция удаляет
  OneDrive-копию с корзиной и историей версий и отзывает 8 грантов. Транскрипты на remote — открытым
  текстом: приватность равна приватности remote и SSH-ключей (остаточный риск).

## Windows

- **Обязан** проверять живость по pid + procStart. Почему: Windows переиспользует pid — `kill -0` /
  `tasklist` дают ложное «жив» или «мёртв» (data-safety по git); реестр 2.1.255 несёт `procStart` и
  `pidDomain` — проверено по `sessions/6278.json`.
- **Обязан** снимать OneDrive из схемы целиком, а не бороться с `+R`. Почему: OneDrive штампует `+R` на
  каждый досинканный каталог, бандлированный Bun даёт EEXIST на `mkdir` по read-only
  (oven-sh/bun#34413; #50886 → фикс 2.1.163, agents 2.1.181, installer #81004 открыт; до 2.1.234 rename
  `.claude.json` под `+R` стопорил старт); почасовой `attrib -R` оставляет окна (ergonomics по cloud-first).
  Doctor проверяет отсутствие reparse-точек и текстовых «ссылок» (<4 КБ, содержимое — путь) на пути.
- **Обязан** делать ссылки каталогов junction (`mklink /J`, без привилегий), а `CLAUDE.md`/`settings.json`
  — копиями с 3-way по last-synced-хешам. Почему: файловые symlink требуют Developer Mode или
  администратора; атомарная запись CLI может подменить симлинк обычным файлом (#78162 открыт).
- **Обязан** писать хуки одной shell-form строкой под Git Bash (`$HOME` = `%USERPROFILE%`). Почему: git
  нужен транспорту, а shell-form хуки на Windows по умолчанию идут через Git Bash — «Git Bash on Windows,
  or PowerShell when Git Bash isn't installed» (доки hooks; строки 2.1.255 упоминают Git Bash). Утверждение
  git-дизайна «Claude Code на Windows требует Git for Windows» доками не подтверждено: без Git Bash хуки
  уходят в PowerShell, а Bash-тул не регистрируется — движок обязан проверять наличие Git Bash в doctor.
  Не проверено: node/python в PATH внутри Git Bash, выживание отсоединённого push после SessionEnd,
  hook-путь с обратными слешами под `sh -c`.
- Не проверено вживую: вся Windows-сторона (хуки Desktop-CLI через Git Bash, junction под реальным
  каталогом, pid reuse); утверждение судьи, что rmdir-свипер удаляет саму junction (RemoveDirectory) —
  вывод, не измерение; реконсилер на tick пересоздаёт ссылки, чтобы окно было ограничено.
