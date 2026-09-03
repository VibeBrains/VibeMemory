# Roadmap — VibeMemory

План и хроника. Один чекбокс — одна итерация; по завершении — `[x]` с датой, веткой и сутью.
Архитектура — [spec/architecture.md](spec/architecture.md), концепт — [idea.md](../idea.md).

## Проектирование

- [x] **Исследование и выбор схемы** — ✅ (2026-09-02, сессия VibeSweep) 17 агентов: 4
  исследователя (нативные возможности Claude Code/Desktop, раскладка `~/.claude`, стор
  Desktop, практика сообщества) → 3 независимых дизайна (cloud-first, local-hot-auto-durable,
  git-transport) → 9 судей по 3 линзам → синтез. Победитель — git-транспорт (30.3/50; 64
  фатальные находки по всем трём легли в [knowledge/design/engineConstraints.md](knowledge/design/engineConstraints.md)).
  Сырые отчёты — [knowledge/research/](knowledge/research/). Заодно на Mac слиты 7
  разъехавшихся историй (6314 записей) и вычищен `~/.claude/projects` — 28 ссылок, 0 реальных
  каталогов.
- [x] **Решения владельца** — ✅ (2026-09-02): имя **VibeMemory** (не VibeClaude — продукт
  мультиагентный; не VibeParty — занято скиллом `party`); remote — **свой хост по SSH**,
  зеркало в приватный GitHub делает хост; **одновременная работа** на машинах — норма, с
  прицелом на SaaS для команд; Windows-миграцию делает ассистент; VibeDub пишет локально, его
  раннер (cwd `/`) движок игнорирует; ядро — Rust без start0 (осознанно: бинарь без рантайма),
  SaaS-слой — на start0 по правилу.
- [x] **Каркас проекта** — ✅ (2026-09-02, main) README, idea.md, CLAUDE.md, docs/README,
  functional.md, spec/architecture.md, база знаний из исследования с индексом; ветки `main`/`next`.
  Пункт в roadmap VibeDub про общую память между инстансами.

## Гейты качества

Всё перечисленное входит в обычный `cargo test` и падает вместе с ним: правило, которое никто не
проверяет, держится ровно до того дня, когда перестаёт.

- [x] **Гейт документации** — ✅ (2026-09-03, next) `tests/docs_integrity.rs` проверяет то, что
  раньше приходилось смотреть глазами на каждом чекбоксе: все относительные ссылки в `docs/`,
  `README.md`, `CLAUDE.md` и `fixtures/README.md` разрешаются (ссылки внутри блоков кода не
  считаются), у каждой записи `knowledge/**` есть строка в индексе, каждый мануал перечислен в
  дереве `docs/README.md`, имена в `docs/` — camelCase, а завершённый пункт roadmap несёт дату и
  ветку. Проверен подделкой: ловит все четыре нарушения.
- [x] **Гейт приватности фикстур** — ✅ (2026-09-03, next) `tests/fixture_hygiene.rs`: в
  `fixtures/**` нет ключей, токенов, адресов почты и путей в облачную папку, а в транскриптах
  каждый `sessionId` — синтетический. Проверен подделкой: подсунутый настоящий id ловится.
- [x] **Property-тесты слияния** — ✅ (2026-09-03, next) `tests/merge_properties.rs`:
  детерминированный генератор (без новых зависимостей) строит 600 троек транскриптов с
  удалениями, дублями, строками состояния, оборванными хвостами и чужими файлами и проверяет
  законы обоих драйверов; отдельный тест закрепляет форму, в которой закон второй машины ложен
  без предусловия. Мутационная проверка нашла две дыры в самих тестах (keep-both мог молча взять
  чужую версию; предусловие не проверялось) — обе закрыты.

## Этап 1 — ядро (`vibememory-core`, чистая логика)

- [x] **Каркас Rust workspace** — ✅ (2026-09-02, next) `Cargo.toml` (members `crates/*`, edition
  2024, workspace.lints), `Cargo.lock`, пин `rust-toolchain.toml` 1.97.1, `clippy.toml` с гейтом
  «ядро без I/O» (`disallowed-methods`/`-types`), `.gitattributes` (`eol=lf`, `fixtures/** -text`),
  крейт `vibememory-core`; `cli` и `mcp` появятся со своими чекбоксами. Грабли тулчейна —
  [knowledge/rust/crossPlatformPaths.md](knowledge/rust/crossPlatformPaths.md).
- [x] **Кодировка имени проекта и имя стора** — ✅ (2026-09-02, next) модуль `naming`: `enc` из
  `transcript_path` с алфавитом `[A-Za-z0-9_-]`; `encode_cwd` по точной формуле CLI (NFC, UTF-16,
  хэш) как предсказание для реконсилера; имя стора от commondir (репо / подкаталог / linked
  worktree / сабмодуль = последний компонент) с проверкой, что commondir объясняет ближайший
  `.git`; `.git`-указатель при отказе git (закрыт край Windows-worktree); корень → `rootCwd`;
  `nameOverrides`/`ignoreCwd` в одном glob-диалекте; идентичность имени NFC+регистр;
  `CLAUDE_CODE_PROJECT_DIR_NAME`. 144 кейса в `fixtures/naming/` (observed / computed /
  unverified), спека — [manuals/configSpec.md](manuals/configSpec.md), мануал —
  [manuals/namingFixtures.md](manuals/namingFixtures.md), факты —
  [knowledge/claudeCode/projectDirEncoding.md](knowledge/claudeCode/projectDirEncoding.md),
  [knowledge/git/repoLayoutEdges.md](knowledge/git/repoLayoutEdges.md). Изменены решения
  architecture §3/§4/§5/§7 и engineConstraints «Имя стора» (root → ошибка; ссылка никогда не
  перенацеливается автоматически).
- [x] **Слияние JSONL** — ✅ (2026-09-03, next) модуль `merge::jsonl`: ключ строки
  `(uuid | байты, номер копии)`, база решает членство (удаление уважается, кроме предков выживших
  записей), локальный файл — хребет, чужие строки вставляются после последней общей записи с
  `uuid` и чередуются по `timestamp`, отбрасывается только хвост без `\n`, целые неразбираемые
  строки сохраняются; двойная правка одной записи — keep-both; постусловие проверяется в ядре
  (единственная ошибка драйвера); отчёт для лога и `additionalContext` (`fork.visible` по правилу
  читателя, `beforeBoundary`, `resurrected`, `absent`). 50 кейсов в `fixtures/merge/` на пяти
  записанных транскриптах (2.1.232, 2.1.255, сайдчейн, журнал), скраббер `scrubTranscript.js`,
  спека — [manuals/mergeFixtures.md](manuals/mergeFixtures.md). Ожидания сверены с независимой
  эталонной реализацией (Python): 50/50 совпали побайтно; её фаззинг (9000 троек) нашёл
  предусловие «закона второй машины». Замеры: 28 МБ — 190 мс, строка 64 МиБ — 87 мс. Исправлены
  факты и решения в transcriptFormat («лист по max timestamp» → ветвь последнего `last-prompt`;
  неразбираемые строки не отбрасываются; таблица версий 2.1.232/255/258), engineConstraints
  («Слияние», «Свежесть», «Часы»), architecture §1/§2/§3/§4/§8/§11, idea.md, hooksLifecycle,
  configDirLayout; новые записи [knowledge/git/mergeDriverInvocation.md](knowledge/git/mergeDriverInvocation.md)
  и [knowledge/design/jsonlMergeInvariants.md](knowledge/design/jsonlMergeInvariants.md).
- [x] **Keep-both для `memory/*.md` с карантином** — ✅ (2026-09-03, next) модуль
  `merge::keep_both`: побайтный выбор целого файла для всего, что не транскрипт (память,
  `custom-title.json`, `workflows/*.json`, бинарные `tool-results/**`) — арбитраж базой, а правка
  с обеих сторон оставляет нашу версию в файле и откладывает чужую в `~/.vibememory/quarantine/`
  под детерминированным именем из пути и штампа машины; отчёт различает память и сайдкар, чтобы
  беспокоить владельца только из-за памяти. 18 кейсов в `fixtures/merge/keepBothScenarios.json`
  (ожидаемые имена посчитаны независимой реализацией правила), инварианты проверяются на каждом
  кейсе. Уточнены engineConstraints (путь карантина `~/.vibememory/`, арбитраж базой, действие на
  все не-JSONL файлы) и architecture §2/§3; в knowledge — переопределяемый каталог памяти
  (`autoMemoryDirectory`, `CLAUDE_CODE_REMOTE_MEMORY_DIR`) и серверный синк памяти Anthropic.
- [x] **Модель записей памяти и проекция в markdown** — ✅ (2026-09-03, next) модуль `memory`:
  запись (`id`, вид, проект, заголовок, однострочное описание, тело, ссылки `[[id]]`, агент,
  созданo/изменено), журнал событий (`uuid` + `parent`), свёртка в состояние, проекция в
  `MEMORY.md` и `<id>.md` и импорт правок обратно. Причинность вместо часов: две версии от одного
  родителя обе остаются, вторая — файлом `<id>.rival-<версия>.md`, индекс просит свести; удаление
  только явным событием и уступает конкурентной правке; чтение нетронутой проекции не пишет
  ничего (проверяется на каждом кейсе). Журнал — обычный JSONL с `uuid`, поэтому сливается уже
  готовым драйвером — своих правил слияния у памяти нет. 16 кейсов в
  `fixtures/memory/memoryScenarios.json`, спека — [manuals/memoryRecordsSpec.md](manuals/memoryRecordsSpec.md),
  решения — [knowledge/design/memoryRecords.md](knowledge/design/memoryRecords.md); architecture §6
  приведена в соответствие.
- [x] **Guard дескрипторов Desktop** — ✅ (2026-09-03, next) `core::desktop`: храповик экспорта
  (карточка может получить транскрипт, но не потерять; `transcriptUnavailable` не выпускается —
  это вердикт про диск одной машины), гейт импорта на трёх измерениях этой машины (каталог
  существует, транскрипт в сторе, ссылка подтверждена `transcript_path` — предсказанная не
  проходит), локальная починка теневым `cliSessionId` из outbox со снятием пометки, перевод cwd
  `{ROOT}/rel` ↔ локальный с выбором самого длинного корня и регистром Windows. Незнакомые поля
  дескриптора переносятся нетронутыми: формат Desktop не документирован. 36 кейсов в
  `fixtures/desktop/`, гейт проверен пятью мутациями — [manuals/desktopFixtures.md](manuals/desktopFixtures.md).
- [x] **Белый список экспорта** — ✅ (2026-09-03, next) `core::export`: отказ по умолчанию, чёрный список побеждает внутри разрешённых каталогов, порог 45 МиБ; 29 кейсов в `fixtures/export/`, гейт проверен двумя мутациями (снятое правило и перевёрнутый дефолт).

## Этап 2 — движок на Mac (`vibememory-cli`)

- [ ] `install / doctor / status`: git-конфиг, `.gitattributes`, ссылки по `links.json`, копии
  `CLAUDE.md`/`settings.json`, skills-ссылка, LaunchAgent тика.
- [ ] Хук `SessionStart`: ссылка первым шагом и без сети; copy-import реального каталога;
  правило «не перелинковывать под живой сессией»; существующая ссылка на другую цель не
  перенацеливается автоматически (`additionalContext` + `vibememory relink` только без живого sid);
  `hookInputInvalid` при неразборе stdin; `CLAUDE_CODE_PROJECT_DIR_NAME` из окружения хука.
- [ ] Спавн git для имени стора: `env_remove` всех `GIT_*` (кроме `GIT_EXEC_PATH`), таймаут →
  `Unavailable`; обязательный обход `.git` вверх до корня тома с таймаутом → `Unknown`.
- [ ] Канонизация cwd в хуке и реконсилере — одна функция, один раз, до всего (обход `.git`,
  `current_dir` спавна git, `NamingInput.cwd`, `links.json`, `encode_cwd`): Mac — realpath;
  Windows — снять `\\?\` (verbatim ломает и слаг, и сверку), junction/subst не разворачивать
  (как `fs.realpathSync` Node, которым ключует Windows-CLI); проверка — этап 4.
- [ ] `existing` для идентичности имён — `git ls-tree -d --name-only HEAD:projects` ∪ readdir с
  дедупликацией (readdir на APFS не видит коллизию регистра); имена, не проходящие
  `StoreName::parse`, пропускаются с предупреждением doctor, а не валят хук.
- [ ] Тип `LinkRecord` для `links.json` в ядре (`enc`, `name`, `cwd` как `{ROOT}/rel` + синтаксис,
  `source`, `predicted`, `confirmedBy`) с `Serialize`/`Deserialize` и фикстурой реального файла;
  сравнение имён — по `StoreName::key()`.
- [ ] Хуки `Stop` (commit сразу, push с дебаунсом), `SessionEnd`, `UserPromptSubmit`-гейт
  свежести с блоком промпта; heartbeat `live.json`, `tails.json`. Снимок живого транскрипта —
  `snapshot_boundary` ядра и `git hash-object -w --stdin`, никакого `git add` живого файла и
  `git add -A`; «грязно» меряется индексом против HEAD.
- [ ] Relocation транскрипта при `cd` (2.1.258 создаёт **реальный** каталог `projects/<enc>` и
  переносит файл с сайдкарами): проверить на живой сессии и решить, что делает хук и тик.
- [ ] Merge-драйверы как подкоманды: `vibememory merge-driver %O %A %B` → `merge_jsonl`, результат
  в `%A`, exit 0; ошибка постусловия → `%A` не трогать, exit 1 → `git merge --abort`;
  `merge.*.recursive` не задавать; `merge --abort` при сбое; никогда commit при `MERGE_HEAD`.
  Отчёт слияния — строкой в лог, `fork.visible`/`beforeBoundary` — в `additionalContext`,
  `dropped`/`resurrected`/`opaque`/`conflicts`/`truncated`/`absent` — предупреждения doctor.
- [ ] `forget <sid>` против дописи на другой машине даёт конфликт дерева **мимо** драйвера
  (`DU`/`UD`): тик обязан abort'ить, удаление — через outbox-tombstone, не через слияние.
- [ ] Проверить вживую: показывает ли список resume вторую ветвь после слияния вилки; поведение
  драйвера под Git Bash на Windows (CRLF в `%O`/`%A`/`%B` при `* -text`); criss-cross с
  виртуальным предком через наш драйвер.
- [ ] `tails.json` (`lastUuid`, число строк) и текст `additionalContext` берут разбор головы и
  правило листа из ядра — вынести `merge::jsonl` наружу одной функцией тогда же, чтобы правило
  «лист = последний `last-prompt`» не было реализовано дважды.
- [ ] Doctor: fail-closed проверка `core.autocrlf=false` и `* -text` в `.gitattributes` стора —
  иначе редактор с CRLF задваивает блок состояния (ключ строки без `uuid` — её байты).
- [ ] Тик: fetch → merge (только неживые здесь сессии; fail-closed при коллизии ключа имён
  `projects/*`) → push → реконсилер ссылок (только при совпадении локального разрешения с
  `links.json`, иначе `nameDisagreement`; предсказанные ссылки помечены и не удаляются) → импорт
  outbox → push-guard → стейл-локи.
- [ ] Проекция памяти в хуках и тике: где живёт `memory.jsonl`, когда идёт импорт правок (Stop) и
  генерация файлов (SessionStart), куда пишется `additionalContext` при расхождении записи.
- [ ] Каталог памяти читать из настроек (`autoMemoryDirectory`, `CLAUDE_CODE_REMOTE_MEMORY_DIR`,
  `CLAUDE_COWORK_MEMORY_PATH_OVERRIDE`), а не выводить из `enc`: иначе стор синхронизирует пустой
  каталог. Карантин: каталог `~/.vibememory/quarantine/`, штамп `<machineId>-<ISO-время>`, текст
  `additionalContext` при `kind: memory`, лог и doctor при `kind: other`. Файл карантина
  пишется `create_new` с суффиксом при совпадении имени: два разных пути могут дать одно имя
  после санитизации (`a/b.md` и `a-b.md`), и отложенная версия не имеет права быть перезаписанной.
- [ ] Outbox: `history.jsonl` под mkdir-локом CLI, `tasks/`, дескрипторы Desktop.
- [ ] Спека `config.json` (дописать `machineId`, `remote`, `roots`, `desktopStore` в
  [manuals/configSpec.md](manuals/configSpec.md)); полный тип конфига в CLI с
  `deny_unknown_fields` и `#[serde(flatten)] RawNamingConfig` (опечатка в ключе → `configInvalid`,
  не «правил нет»); мануал «как начать» + засев закомментированного образца; doctor: git ≥ 2.31 (`--path-format`; по памяти, сверить по RelNotes), предупреждение о
  сабмодуле, питаемом разными суперпроектами, список предсказанных ссылок без подтверждения.
- [ ] Измерить гонку SessionStart ↔ первая запись в интерактивном TTY (сейчас измерено только
  `-p` и stream-json).

## Этап 3 — миграция Mac

- [ ] Хост: bare-репо по SSH, `post-receive` → зеркало в приватный GitHub.
- [ ] `migrate --from ~/OneDrive/.claude`: dry-run, sha256-манифест 100 % (иначе стоп),
  `-ALL-/<repo>` → `projects/<repo>`, конфликт-копии (`*-MacMini.jsonl`, `*-GPD-WIN-MAX2.*`,
  `MEMORY-GPD-WIN-MAX2.md`) сводятся по uuid / keep-both, дескрипторы → outbox с починкой
  `cliSessionId`, конфиги → `config/`; стор `-` (cwd `/`, сессии раннера) не импортируется —
  остаётся в архиве, `/` в `ignoreCwd`. Перед этим — пин OneDrive-папки (932 файла
  дегидрированы).
- [ ] Перевод 28 ссылок в стор, Desktop-стор из симлинка в реальный каталог, проверка resume в
  терминале и Desktop; искусственная вилка одной сессии на двух клонах → resume в терминале
  (2.1.232) и в Desktop: видима ли ветвь последнего `last-prompt`, сверить с `fork.visible`.
- [ ] Гигиена OneDrive: `.credentials.json` удалить (+корзина, +версии), отозвать гранты;
  `~/OneDrive/.claude` → архив read-only, снести через 30 дней после Windows.
- [ ] Вывод `init.sh` / `sync-repo`: раскопки → замысел в knowledge → удаление; скилл
  сводится к обёртке над `vibememory status/doctor`.

## Этап 4 — Windows

- [ ] `CLAUDE_CONFIG_DIR=%USERPROFILE%\.claude` (setx + Desktop Settings), `.claude.json`
  переезжает локально, `.credentials.json` не переносится (relogin), junction Desktop-стора
  снимается, реальный каталог в userData действующей установки (MSIX / Squirrel).
- [ ] `bootstrap.ps1`: clone, `config.json`, junction `projects/D--…`, Task Scheduler, хуки через
  Git Bash, `procStart` против pid reuse.
- [ ] Проверка: resume Mac-сессии на GPD и обратно, список Desktop на обеих.

## Этап 5 — MCP-память для любых агентов

- [ ] Разобраться с серверной памятью самой Anthropic (мультисторы, манифесты, push-удаление —
  есть во всех трёх версиях CLI): что из этого включено у владельца и как соотносится с
  MCP-памятью VibeMemory. До этого пункта считать позиционирование этапа 5 не подтверждённым.
- [ ] `vibememory-mcp`: `memory_search / get / save / update / delete`, `history_search`;
  stdio локально, HTTP с bearer на хосте. Поверх `memory::journal` — сервер только читает
  состояние и дописывает события, своей модели у него нет.
- [ ] Подключение и проверка: Claude Code, Codex, Gemini CLI, Cursor — одна память у всех.

## Этап 6 — SaaS для команд (start0)

- [ ] Кабинет, команды, роли, токены per member × agent, биллинг. Начинать с навыка `start0`.

## Открытое

- [ ] Перечитывает ли Desktop стор дескрипторов без рестарта.
- [ ] Политика Desktop «components under the config root may not be symlinks» — следить за
  `PlantDetectedError`; запасной план для CLI — `CLAUDE_CODE_PROJECT_DIR_NAME` через обёртку.
- [ ] Нативная кросс-девайс сессия у Anthropic (экспорт/импорт в Desktop за фиче-флагом): когда
  появится — снять движок одной командой без потери данных.
