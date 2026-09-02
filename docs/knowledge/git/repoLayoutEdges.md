# Раскладки git и края discovery (git 2.50.1, 2026-09-02)

Что печатает `git rev-parse --path-format=absolute --git-common-dir` и что лежит в `.git`-файлах
в раскладках, из которых движок выводит имя стора. Всё снято на этой машине (Apple Git-155) в
scratch-репозиториях и на реальных проектах владельца; правила — `crates/vibememory-core/src/naming/git.rs`,
кейсы — `fixtures/naming/resolveStoreName.json`.

## Что печатает `--git-common-dir`

| Где выполнено | Вывод | Имя стора |
|---|---|---|
| корень репо, его подкаталог (`VibeDub/server`) | `<repo>/.git` | `basename(parent)` |
| linked worktree (`git worktree add`) и его подкаталог | `<main>/.git` главного репо | главный репо |
| absorbed-сабмодуль (`VibeIDEA/vibe-plugins/vibe-agent/resources/vibeDefaults`) и его подкаталог | `<super>/.git/modules/<path>` | последний компонент (`vibeDefaults`) |
| worktree, созданный из сабмодуля | `<super>/.git/modules/<path>` | тот же сабмодуль |
| bare-репо `store.git` | сам `store.git` | ошибка `unrecognizedGitLayout` |
| `git init --separate-git-dir=<dir>` | `<dir>` | ошибка `unrecognizedGitLayout` |
| не репо (VibeSweep), `/` | exit 128 | plain dir / ошибка `rootCwd` |

Вывод заканчивается одним `\n`; текст ошибки «not a git repository» различается на границе
монтирования (`Stopping at filesystem boundary (GIT_DISCOVERY_ACROSS_FILESYSTEM not set)`) —
ещё одна причина классифицировать по коду выхода и файловой системе, а не по stderr.

## Что лежит в `.git`-файлах

- linked worktree по умолчанию — абсолютный путь: `gitdir: <main>/.git/worktrees/<id>`;
  с `--relative-paths` (git 2.48+) — относительный: `gitdir: ../main/.git/worktrees/wtrel`.
- absorbed-сабмодуль — относительный: `gitdir: ../../.git/modules/libs/foo`; у VibeIDEA —
  `gitdir: ../../../../.git/modules/vibe-plugins/vibe-agent/resources/vibeDefaults`.
- worktree из сабмодуля: `gitdir: <super>/.git/modules/libs/foo/worktrees/subwt` — между `.git` и
  `worktrees` стоит `modules/<path>`, поэтому правило «отбросить хвостовую пару `worktrees/<id>`»
  обязано применяться до разбора раскладки, а не искать `.git/worktrees` как соседей.
- `--separate-git-dir`: `gitdir: <абсолютный dir>`.
- Worktree, созданный на Windows и открытый на Mac
  (`VibeIDE/.claude/worktrees/recursing-cannon-5da42a/.git`): `gitdir: D:/Projects/VibeCode/VibeIDE/.git/worktrees/…`;
  git на Mac падает с «not a git repository», но указатель называет репозиторий — имя выводится из
  него (`VibeIDE`), не из basename(cwd).
- Относительный указатель разрешается от каталога файла лексически: сперва join, потом
  нормализация `..` (см. [rust/crossPlatformPaths.md](../rust/crossPlatformPaths.md)).

## Discovery молча проходит сквозь битый `.git`

- `outer/inner/.git/HEAD` = `garbage` или `outer/tpl/.git` — пустой каталог: из `outer/inner/sub` и
  `outer/tpl` git отвечает `<outer>/.git` с exit 0. Тот же пустой `.git` без внешнего репо — exit 128.
  Следствие: commondir от git нельзя принимать без проверки, что он объясняет ближайшую запись
  `.git` над cwd (каталог равен commondir, либо gitdir из файла после отбрасывания `worktrees/<id>`
  равен commondir); иначе — `gitLayoutMismatch`.
- `GIT_DIR=<чужой репо>` из каталога с пустым `.git` (и вовсе без `.git` — тот же ответ): git
  отвечает чужим commondir с exit 0 — снимать все `GIT_*` (кроме `GIT_EXEC_PATH`) перед спавном. `GIT_CEILING_DIRECTORIES=<родитель репо>` внутри репо discovery не
  остановил (репо найдено до потолка).
- Вложенный сабмодуль (`.git/modules/a/modules/b`) в сессии не инициализировался — кейс помечен
  `unverified`.

## git печатает имена в форме на диске: NFD против NFC

Каталог создан как `cafe` + U+0301 (NFD), вход в него — через NFC-строку: `pwd` показывает
NFC, а `git rev-parse --path-format=absolute --git-common-dir` печатает `…/cafe<U+0301>/.git`
(NFD), и с `core.precomposeunicode=true` — тоже. CLI при этом отдаёт cwd в NFC. Поэтому ядро
сравнивает пути по компонентам после NFC и выводит имена сторов в NFC; байтовое сравнение дало
бы ложный `gitLayoutMismatch` на каждом не-ASCII пути репозитория.

## Отбрасывание `worktrees/<id>` — эвристика без git

Указатель `.git` читается двумя способами: как написано и без хвостовой пары `worktrees/<id>`.
Когда git ответил, верно то чтение, которое равно его commondir; когда git отказал, берётся
укороченное — оно ошибётся только для сабмодуля, чей собственный путь оканчивается на
`worktrees/<имя>` (тогда `unrecognizedGitLayout`, не чужое имя).

## Имена, различающиеся регистром, ломают checkout стора

Прогон верификатора в этой сессии (не повторён самостоятельно): ветка `mac` добавляет
`projects/VibeIDE/…`, ветка `win` — `projects/vibeide/…`; `git merge` проходит без конфликта,
на APFS оба ложатся в один каталог, `MEMORY.md` одной стороны перезаписан, `git status` вечно
грязный, свежий клон печатает «the following paths have collided», `git add -A` падает на
`will not add file alias`. Поэтому ключ идентичности имени стора — NFC + свёртка регистра, а
существующее написание всегда переиспользуется (`StoreName::key`).
