# Архитектура VibeMemory

Целевая схема на 2026-09-02. Основа — схема-победитель судейства
([knowledge/research/synthesisReport.md](../knowledge/research/synthesisReport.md)) с решениями
владельца: свой хост как remote, одновременная работа на машинах — норма, память как записи,
MCP-доступ для любых агентов. Каждое утверждение о поведении Claude Code/Desktop подтверждено в
[knowledge/](../knowledge/README.md); ограничения, которые движок не имеет права нарушать, —
[knowledge/design/engineConstraints.md](../knowledge/design/engineConstraints.md).

## 1. Вердикт

Транспорт — **git с приватным remote** и движок `vibememory` (хуки + фоновый тик). Не облачная
папка: OneDrive дегидрирует, плодит конфликт-копии, расплющивает ссылки и ставит `+R`; Desktop
на недоступном транскрипте навсегда стирает привязку. Не Syncthing: для «закрыл крышку — открыл
другую» нужны оба узла онлайн. Не нативно: у Anthropic переноса сессии между машинами нет.
Git даёт то, чего облачная папка не даёт принципиально: детерминированное слияние JSONL по `uuid`
(дозапись плюс редкие удаления записей самим CLI — [knowledge/claudeCode/transcriptFormat.md](../knowledge/claudeCode/transcriptFormat.md)),
полную историю, отсутствие reparse-точек и запись только там, где разрешено.

## 2. Что общее, что локальное

| Сущность | Где | Режим |
|---|---|---|
| `projects/<name>/<sid>.jsonl`, `<sid>/{tool-results,subagents,workflows,custom-title.json}` | стор (git) | union по ключу `(uuid \| байты, номер копии)`; база решает членство (удаление уважается, кроме предков выживших записей), но не порядок; локальный файл — хребет, чужие строки вставляются после последней общей записи с `uuid` и чередуются по `timestamp`; отбрасывается только хвост без `\n`; файлы ≥ 45 МБ не коммитятся (производные, для resume не нужны) |
| `projects/<name>/agents/<agent>/<sid>.jsonl` | стор (git) | сессии чужих агентов, отданные `vibememory session put` плоским JSONL ([спека](../manuals/foreignSessionSpec.md)); слияние то же, что у транскриптов; вне верхнего уровня, чтобы не попасть в `/resume` Claude Code |
| `projects/<name>/memory/**` | стор | **записи** (§6); проекция в markdown; арбитраж базой, а правка с обеих сторон — keep-both: наша версия в файле, чужая в `~/.vibememory/quarantine/` + `additionalContext` «сведи», никогда LWW |
| `projects/<name>/.keep` | стор | защита от rmdir-свипера пустых каталогов |
| `config/{CLAUDE.md,settings.json}`, `config/skills/` | стор | skills — ссылка; CLAUDE.md/settings.json — управляемые копии с 3-way по last-synced-хешам |
| `machines/<id>/live.json` (sid → heartbeat, cwd), `tails.json` (sid → lastUuid, lines, at) | стор, пишет только владелец | межмашинная живость и свежесть — **основной** механизм при одновременной работе |
| `machines/<id>/links.json` (enc ↔ name ↔ cwd), `history.jsonl` (project как `{ROOT}/rel`), `tasks/<sid>/N.json`, `desktop/<acct>/<org>/local_*.json` + `deleted_*` | стор, пишет только владелец | outbox: читают все, конфликтов нет по построению |
| `~/.claude/.claude.json*`, `backups/`, Keychain / `.credentials.json`, `sessions/*.json`+`*.key`, `session-env/`, `local-agent-mode-sessions/`, `ant-device-registry.json`, `shell-snapshots/`, `telemetry/`, `debug/`, `cache/`, `statsig/`, `file-history/`, `logs/`, `downloads/`, `plugins/`, `todos/`, `tasks/`, `history.jsonl`, `ide/`, `mcp-needs-auth-cache.json`, `policy-limits.json`, `remote-settings.json`, `stats-cache.json`, `.last-*`, `.DS_Store` | локально | никогда не покидают машину: защитный `.gitignore` + белый список экспорта + тест. Гейт — `core::export`: разрешено только `projects/**`, `skills/**`, `CLAUDE.md`, `settings.json`, всё остальное отказ по умолчанию, а чёрный список бьёт белый и внутри разрешённого каталога |
| `~/.claude/projects/<enc>` | локально | только ссылки (symlink / junction) в `~/.vibememory/store/projects/<name>` |
| `~/.claude/history.jsonl`, `~/.claude/tasks/` | локально | живые файлы CLI; реплика через outbox |
| Desktop-стор `claude-code-sessions/` | локально, **реальный** каталог | Desktop не терпит reparse-точек под своим корнем; реплика через outbox с guard'ом |
| `~/.vibememory/{config.json,state.json,lock,quarantine/,log/,last-hook.json}` | локально | корни путей машины, хеши, лок с pid-владельцем |

`.claude.json` не засевается ничем (trust и прочее): переносимого там нет, история порчи есть.

## 3. Стор и remote

- Клон стора — `~/.vibememory/store` на загрузочном томе (не OneDrive, не съёмный диск).
- `origin` — bare-репо на **своём хосте** по SSH. Хост зеркалит в приватный GitHub
  (`post-receive` → `git push --mirror`): бэкап не зависит от того, чья машина пушила последней.
- Git-конфиг ставит `install`: `core.autocrlf=false`, `core.filemode=false`,
  `core.symlinks=false`, `core.longpaths=true` (Win), `core.precomposeunicode=true` (Mac),
  `core.fsmonitor=true`, `core.untrackedCache=true`. LFS не используется.
- `.gitattributes`: `* -text`, `* merge=vibememory-keepboth`, `**/*.jsonl merge=vibememory-jsonl`.
  Второй драйвер — побайтный выбор целого файла (`merge::keep_both`): он покрывает и память, и
  сайдкары сессии, включая бинарные, поэтому ничего не разбирает и маркеров не оставляет.
  Оба драйвера детерминированы, маркеров `<<<<` не бывает; исключение в драйвере →
  `git merge --abort`, запись в лог, `additionalContext` на следующем старте. При `MERGE_HEAD`
  движок никогда не коммитит. `merge.*.recursive` не задаётся: виртуального предка при criss-cross
  строит тот же драйвер, а union замкнут. Пустой `%O` (файл создан с обеих сторон) — не особый
  режим, а тот же вызов, что copy-import; единственная ошибка драйвера — нарушение его
  постусловия ([knowledge/git/mergeDriverInvocation.md](../knowledge/git/mergeDriverInvocation.md)).
- **Имя стора** (ядро `naming`, порядок: относительный cwd → ошибка; `ignoreCwd`; корень →
  ошибка; `nameOverrides` → git → имя каталога): хук один раз канонизирует cwd (realpath; git
  печатает физические пути), CLI спавнит `git rev-parse --path-format=absolute --git-common-dir`
  со снятым окружением `GIT_*` (кроме `GIT_EXEC_PATH`) и **всегда** ищет ближайшую запись `.git` над cwd (git молча проходит сквозь
  битый или пустой вложенный `.git` к внешнему репо — [knowledge/git/repoLayoutEdges.md](../knowledge/git/repoLayoutEdges.md)).
  Commondir принимается, только если он объясняет эту запись; `<repo>/.git` → `basename(repo)`
  (репо, подкаталог, linked worktree); `<super>/.git/modules/<path>` → последний компонент (имя
  сабмодуля; standalone-клон того же репозитория — отдельный стор, объединение — `nameOverrides`);
  bare, `--separate-git-dir`, утёкший `GIT_DIR` → ошибка, не имя. git отказал, а `.git` — файл-указатель
  (worktree с другой машины) → имя из `gitdir:`; `.git` — каталог → ошибка; `.git` нет → `basename(cwd)`.
  Корень ФС — ошибка `rootCwd`, лечение только `ignoreCwd` (стора `root` не бывает: `/` Mac и `D:\`
  Windows ничего не объединяет). Имя валидируется для APFS/NTFS/git; выведенные из путей имена —
  в NFC (git печатает форму на диске, CLI — NFC); идентичность — NFC + свёртка регистра,
  существующее написание переиспользуется (`existing` — имена из дерева стора, не readdir:
  APFS схлопывает регистр). Ошибка — конечное состояние: ни ссылки, ни
  стора, `additionalContext` с кодом. Полные правила — [knowledge/design/engineConstraints.md](../knowledge/design/engineConstraints.md).
- **`nameOverrides` и `ignoreCwd`** — один glob-диалект (`/` как разделитель, литерал = точное
  совпадение, поддерево — явным `/**`, Windows-шаблоны без учёта регистра); неоднозначный override —
  ошибка; игнор сильнее override; для игнорируемого cwd (раннер VibeDub — `/`) движок не делает
  ничего, сессия пишется в реальный каталог CLI. Спека — [manuals/configSpec.md](../manuals/configSpec.md).

## 4. Хуки (в локальном `settings.json` машины, в общую копию не попадают; путь в команде — абсолютный, в кавычках; на Windows — через Git Bash)

- **SessionStart** (все matcher, timeout 10, **без сети**): (a) `enc = basename(dirname(transcript_path))` — кодировку не переизобретаем; (b) `projects/<enc>` отсутствует → стор + `.keep` + ссылка; ссылка на другую цель → **не трогать** (никакой автоматической перелинковки: `additionalContext` + `vibememory relink`, допустимый только без живого sid с этим enc в `live.json` всех машин); **реальный каталог** → copy-import по uuid, а rename в ссылку только при `source=startup`, когда `<session_id>.jsonl` там ещё нет и в `live.json` всех машин нет живого sid с этим cwd; иначе pending для тика; (c) `links.json`; (d) heartbeat; (e) `exit 0` всегда, stdout — только `additionalContext` при проблеме. Транскрипт рождается после выхода хука (измерено для `-p` и stream-json; интерактивный TTY — открытый пункт).
- **UserPromptSubmit** (timeout 15) — гейт свежести: на первом промпте и далее раз в 5 мин `git fetch` (5 с; офлайн → пропуск с пометкой). Если `origin/main` меняет `projects/*/<sid>.jsonl` этой сессии или `live.json` другой машины показывает этот sid живым (heartbeat < 30 мин) → union на диске + **блок промпта**: «сессию продолжили на `<машина>` в `<время>`: закрой и открой заново — транскрипт обновлён». Единственное место, где допустим ритуал, и только в гонке.
- **Stop** (`async`): **commit сразу** — снимок живого файла до последнего `\n` (`snapshot_boundary` ядра) через `git hash-object -w --stdin`, а не `git add` живого файла; целые неразбираемые строки сохраняются, отбрасывается только оборванный хвост; «грязно ли» меряется индексом против HEAD, `git add -A` под запретом. Дальше heartbeat, `tails.json`, экспорт outbox, push с дебаунсом 20 с. «Закрыл крышку» теряет секунды, не ходы.
- **SessionEnd** (timeout 60): commit синхронно, снятие heartbeat, push отсоединённым процессом.

## 5. Тик

Mac — LaunchAgent (`RunAtLoad` + `StartInterval 120`; во сне интервал пропускается, лаг после
пробуждения ≤ 2 мин). Windows — Task Scheduler: таймер каждые 2 мин с `StartWhenAvailable` — пропущенный
запуск догоняется сразу; триггеры входа и разблокировки без администратора Windows не регистрирует; задание — XML в UTF-16 с BOM,
`schtasks /Create /XML … /F`, только для движка под домашним каталогом; запуск через `conhost.exe --headless`,
чтобы ни движок, ни его git не открывали консольное окно. Linux — таймер `systemd --user`
(`vibememory-tick.timer`, при старте и каждые 2 мин), без пользовательского systemd — строка `cron` с
пометкой. Интервал — одна константа на все три.
Шаги: (1) `fetch`; (2) `merge origin/main`, только если входящий diff не трогает файлы сессий,
живых **на этой машине** (fail-closed); (3) push, если dirty; (4) реконсилер: для каждого
`machines/*/links.json` × локальные корни из `config.json` — если переведённый cwd существует и
локальное разрешение имени совпадает с именем из `links.json` (иначе — запись `nameDisagreement`
для doctor), гарантировать ссылку `projects/<enc>` с `enc = encode_cwd(NFC(realpath(cwd)))`
(фолбэк-сканы CLI и Desktop ссылок не видят — ссылка обязана существовать заранее); такая ссылка
помечена в `links.json` как `predicted`, при первом SessionStart сверяется с `transcript_path`
(расхождение → вторая ссылка под реальным enc) и **никогда не удаляется**; существующая ссылка
никогда не перенацеливается; (5) импорт outbox: `tasks/` для неживых sid, `history`
под mkdir-локом CLI, дескрипторы Desktop (§7); (6) push-guard: удалённые в рабочей копии
файлы `projects/**` восстанавливаются, удаление только через `vibememory forget <sid>`;
(7) стейл-локи снимаются по проверке живости владельца (pid + procStart — Windows
переиспользует pid).

## 6. Память как записи и MCP

Первоисточник памяти — **записи** в сторе: `{id, type, project, agent, createdAt, updatedAt,
title, body, links[]}` плюс однострочный `description` для индекса (append-only журнал
`projects/<имя>/memory.jsonl` + материализованное состояние). Markdown Claude Code
(`memory/MEMORY.md` + `memory/*.md`) — проекция: движок генерирует файлы из записей и
импортирует правки файлов обратно в записи (Claude пишет файлы, как привык).

Реализовано в `vibememory-core::memory` (2026-09-03), решения — [knowledge/design/memoryRecords.md](../knowledge/design/memoryRecords.md),
формат — [manuals/memoryRecordsSpec.md](../manuals/memoryRecordsSpec.md): каждое событие журнала
несёт `uuid` (журналы двух машин сливает **тот же** JSONL-драйвер) и `parent` — версию, которую
видел писавший. Конфликт правок одной записи — keep-both по причинности, а не по часам: обе версии
остаются, вторая проецируется файлом `<id>.rival-<версия>.md`, индекс просит их свести. Удаление —
только явным событием `delete`, пропавший файл проекции удалением не считается; чтение нетронутой
проекции не порождает событий.

MCP-сервер `vibememory-mcp` (stdio локально; HTTP с bearer-токеном на хосте — для агентов без
локального стора): `memory_search` (индекс: id, тип, дата, превью), `memory_get` (полный текст
по id), `memory_save`, `memory_update`, `memory_delete` (в карантин, не навсегда),
`history_search` (полнотекстовый поиск по транскриптам всех агентов — читать, не продолжать).
Подключение: Claude/Claude Code (`mcpServers`), Codex, Gemini CLI, Cursor, Zed, Cline — одна
и та же память у всех.

## 7. Desktop

Стор дескрипторов — реальный локальный каталог. Экспорт в outbox с **guard'ом**: дескриптор,
потерявший `cliSessionId` или получивший `transcriptUnavailable` относительно последней
экспортированной версии, не экспортируется, а локально чинится из outbox (shadow
`cliSessionId`). Импорт чужих дескрипторов — только когда переведённый cwd существует,
транскрипт в сторе есть и ссылка создана **и подтверждена** `transcript_path` этой машины
(предсказанная реконсилером ссылка гейт не проходит: Desktop на промахе резюме стирает `cliSessionId`); cwd переписывается в локальную форму. Перевод — через объявленные `roots` как `{ROOT}/rel` (самый длинный подходящий корень, ничья по имени, регистр Windows без учёта); путь вне всех корней не переводится и потому не экспортируется. Незнакомые поля дескриптора движок не интерпретирует и переносит нетронутыми — формат Desktop не документирован. Решения и их цена — [knowledge/design/desktopGuard.md](../knowledge/design/desktopGuard.md). Перечитывает
ли Desktop стор без рестарта — открытый пункт; если нет, карточки другой машины появляются
после перезапуска Desktop.

## 8. Известные провалы и как закрыты

- **Конфликт-копии `.claude.json`** — файл не покидает машину. Единственный shared-write формат — транскрипты и записи памяти; всё остальное — outbox одного писателя.
- **Расплющивание ссылок** — облачной папки нет; ссылки живут в `~/.claude` (снаружи репо), `core.symlinks=false`, в репо ссылок нет.
- **Разъезд транскрипта при перелинковке живой сессии** — ссылка никогда не переключается под живой сессией; merge не трогает файлы живых локальных сессий; вилка «продолжили на B, ввели на A» блокируется гейтом; в оставшемся случае union сохраняет обе ветви, а `additionalContext` честно говорит, какую покажет CLI: ветвь последнего `last-prompt` (`fork.visible` в отчёте слияния), причём свежий ход другой машины без своего `last-prompt` в разговор не попадёт, а в файле > 5 МиБ строки раньше последней `compact_boundary` не загружаются (`beforeBoundary`).
- **Пустые каталоги и retention** — `.keep`; `cleanupPeriodDays: 3650` в общем settings.json; doctor валидирует settings.json (невалидный → дефолт 30 дней); git-история; push-guard.
- **Windows на старой схеме** — `CLAUDE_CONFIG_DIR=%USERPROFILE%\.claude`, `.claude.json` локально, `.credentials.json` из облака удаляется (relogin), junction Desktop-стора снимается, `projects/D--…` — junction без привилегий.
- **Порча дескрипторов Desktop** — guard на экспорт (§7).

## 9. Формат `config.json` (`~/.vibememory/config.json`)

```jsonc
{
  "machineId": "mac-main",                       // уникально на машину, попадает в имена outbox
  "remote": "ssh://git@host/vibememory/store.git",
  "roots": { "PROJECTS": "/Volumes/Storage/Projects", "HOME_PROJECTS": "/Users/borodatych/Projects" },
  "nameOverrides": { "/Volumes/Storage/Projects/VibeCode/VibeSweep": "VibeSweep" },  // литерал = точное совпадение, поддерево — «/**»
  "ignoreCwd": ["/"],                             // сессии раннера VibeDub — локально
  "desktopStore": "auto"                         // или явный путь (MSIX / Squirrel)
}
```

Спека формата для модели — [manuals/configSpec.md](../manuals/configSpec.md) (`nameOverrides` и
`ignoreCwd` — полностью, остальные ключи — с `install`); мануал «как начать» и засев образца —
вместе с `install` (правило: фича с форматом обязана иметь спеку и образец).

## 10. Реализация

Rust workspace: `vibememory-core` — чистая логика (имена, кодировка, слияние JSONL, keep-both,
модель записей, guard дескрипторов) без I/O, тестируется на фикстурах реального формата;
`vibememory-cli` — бинарь (`install / doctor / status / tick / migrate / forget / hook …`,
merge-драйверы, LaunchAgent / Task Scheduler); `vibememory-mcp` — сервер памяти. Сборки под
macOS (`aarch64-apple-darwin`, `x86_64-apple-darwin`), Linux (`x86_64-unknown-linux-gnu`) и Windows
(`x86_64-pc-windows-gnu`) — все на Mac; на целевой машине ноль зависимостей кроме
git и (для хуков на Windows) Git Bash, который Claude Code и так требует.

## 11. Остаточные риски

Формат JSONL и дескрипторов объявлен внутренним — драйвер под тест-гейтом, при неразборе
байтовый режим, при исключении abort. Офлайн-resume на двух машинах по очереди даёт вилку:
union сохранит обе ветви, CLI покажет одну. Открытая вкладка Desktop неделями: heartbeat
протухнет, защита остаётся у гейта по `tails.json`. Идентичность по basename для не-git
каталогов и для сабмодулей (сабмодуль VibeIDEA — репозиторий `VibeBrains`: как standalone-клон он
получит стор `VibeBrains`, как сабмодуль — `vibeDefaults`) — `nameOverrides`. Слияние транскриптов
добавляет свои остаточные риски: удаление записи одной машиной уважается (кроме предков выживших),
двойная правка одной записи оставляет обе строки (читатель схлопнет их по `uuid`), свежий ход без
`last-prompt` не виден до следующего промпта, `forget <sid>` против чужой дописи даёт конфликт
дерева мимо драйвера, а включённый удалённо GC транскриптов может урезать файл на одной машине. Политика Desktop про ссылки под корнем конфига может ужесточиться —
doctor следит за `PlantDetectedError`, запасной план для CLI — `CLAUDE_CODE_PROJECT_DIR_NAME`
через обёртку. Транскрипты на remote открытым текстом — приватность = приватность хоста и
ключей; секреты в выводе инструментов уезжают туда же.
