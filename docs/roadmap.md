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

## Этап 1 — ядро (`vibememory-core`, чистая логика)

- [ ] Кодировка имени проекта и имя стора: `enc` из `transcript_path`, `--git-common-dir`
  (worktree, подкаталоги, сабмодули), `nameOverrides`, `ignoreCwd`.
- [ ] Слияние JSONL: union по `uuid`, побайтный режим для строк без `uuid`, стабильная
  сортировка по `timestamp`, обрезка оборванной последней строки. Фикстуры реального формата
  2.1.255: `queue-operation`, `bridge-session`, `custom-title`, `summary` без uuid,
  `journal.jsonl`, обрывок строки. Гейт: слияние двух чистых одномашинных транскриптов не
  отклоняется (немонотонные timestamp и осиротевшие `parentUuid` — норма).
- [ ] Keep-both для `memory/*.md` с карантином и текстом подсказки.
- [ ] Модель записей памяти и проекция в markdown (туда и обратно).
- [ ] Guard дескрипторов Desktop (не понижать `cliSessionId`, не экспортировать
  `transcriptUnavailable`), перевод cwd `{ROOT}/rel` ↔ локальный.
- [ ] Белый список экспорта + тест «ни один файл из чёрного списка не попадает в стор».

## Этап 2 — движок на Mac (`vibememory-cli`)

- [ ] `install / doctor / status`: git-конфиг, `.gitattributes`, ссылки по `links.json`, копии
  `CLAUDE.md`/`settings.json`, skills-ссылка, LaunchAgent тика.
- [ ] Хук `SessionStart`: ссылка первым шагом и без сети; copy-import реального каталога;
  правило «не перелинковывать под живой сессией».
- [ ] Хуки `Stop` (commit сразу, push с дебаунсом), `SessionEnd`, `UserPromptSubmit`-гейт
  свежести с блоком промпта; heartbeat `live.json`, `tails.json`.
- [ ] Merge-драйверы как подкоманды; `merge --abort` при сбое; никогда commit при `MERGE_HEAD`.
- [ ] Тик: fetch → merge (только неживые здесь сессии) → push → реконсилер ссылок → импорт
  outbox → push-guard → стейл-локи.
- [ ] Outbox: `history.jsonl` под mkdir-локом CLI, `tasks/`, дескрипторы Desktop.
- [ ] Спека `config.json` и мануал «как начать» в `docs/manuals/` + закомментированный образец.
- [ ] Измерить гонку SessionStart ↔ первая запись в интерактивном TTY (сейчас измерено только
  `-p` и stream-json).

## Этап 3 — миграция Mac

- [ ] Хост: bare-репо по SSH, `post-receive` → зеркало в приватный GitHub.
- [ ] `migrate --from ~/OneDrive/.claude`: dry-run, sha256-манифест 100 % (иначе стоп),
  `-ALL-/<repo>` → `projects/<repo>`, конфликт-копии (`*-MacMini.jsonl`, `*-GPD-WIN-MAX2.*`,
  `MEMORY-GPD-WIN-MAX2.md`) сводятся по uuid / keep-both, дескрипторы → outbox с починкой
  `cliSessionId`, конфиги → `config/`. Перед этим — пин OneDrive-папки (932 файла
  дегидрированы).
- [ ] Перевод 28 ссылок в стор, Desktop-стор из симлинка в реальный каталог, проверка resume в
  терминале и Desktop.
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

- [ ] `vibememory-mcp`: `memory_search / get / save / update / delete`, `history_search`;
  stdio локально, HTTP с bearer на хосте.
- [ ] Подключение и проверка: Claude Code, Codex, Gemini CLI, Cursor — одна память у всех.

## Этап 6 — SaaS для команд (start0)

- [ ] Кабинет, команды, роли, токены per member × agent, биллинг. Начинать с навыка `start0`.

## Открытое

- [ ] Перечитывает ли Desktop стор дескрипторов без рестарта.
- [ ] Политика Desktop «components under the config root may not be symlinks» — следить за
  `PlantDetectedError`; запасной план для CLI — `CLAUDE_CODE_PROJECT_DIR_NAME` через обёртку.
- [ ] Нативная кросс-девайс сессия у Anthropic (экспорт/импорт в Desktop за фиче-флагом): когда
  появится — снять движок одной командой без потери данных.
