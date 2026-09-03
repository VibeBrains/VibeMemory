# Как добавить кейс в `fixtures/desktop/`

Гейт отвечает на три вопроса про карточку вкладки Code: **уедет ли она в outbox**,
**примет ли её эта машина** и **как её `cwd` называется на другой машине**. Правила —
`crates/vibememory-core/src/desktop/`, ожидания — `fixtures/desktop/descriptorScenarios.json`,
драйвер — `crates/vibememory-core/tests/desktop_fixtures.rs`.

## Почему кейсы не `observed`

Стор дескрипторов владельца лежит в облачной папке, а её файлы дегидрированы: чтение тянет их из
сети. Поэтому значения синтетические, а форма полей взята из уже опубликованных частот по 165
реальным файлам `local_*.json` — [knowledge/claudeDesktop/sessionStore.md](../knowledge/claudeDesktop/sessionStore.md).
Метка `computed` — правило гейта; `unverified` — то, что пишет другая платформа (пути `D:\…`,
worktree-карточки). Появится читаемый стор — кейсы можно будет переразметить в `observed`, но
переписывать ожидания из вывода модуля нельзя никогда.

## Четыре списка кейсов

- `machines` — корни машин (`config.json` → `roots`) и синтаксис путей; кейс ссылается на машину
  по имени, а не повторяет корни.
- `rootCases` — `toPortable` и/или `toLocal`; `thenToLocalOn` делает круг «Windows → портативно →
  Mac». Ожидание — либо путь, либо `error` (`outside` | `unknownRoot` | `notPortable`).
- `exportCases` — `local` (карточка на диске) и необязательный `exported` (последняя версия в
  outbox). Ожидание: `export` с переведённым `cwd`/`originCwd` либо `withhold` с причиной
  (`lostTranscript` | `markedUnavailable` | `cwdOutsideRoots`).
- `importCases` — `remote` (карточка из outbox) и `facts`: три измерения этой машины
  (`cwdExists`, `transcriptInStore`, `confirmedTranscriptPath`). Ожидание: `import` либо `skip`
  с причиной (`noTranscript` | `transcriptMissing` | `cwdMissing` | `linkUnconfirmed` |
  `rootUnknown`).
- `repairCases` — `local`, `exported`, `facts`; ожидание `repaired` и `cliSessionId`.

`keeps` в ожидании перечисляет ключи, которых движок не понимает: гейт требует, чтобы карточка
донесла их нетронутыми. Добавляя кейс с новым полем Desktop, кладите его в `keeps`, а не в
структуру дескриптора.

## Порядок добавления

1. Кейс в JSON — ожидание пишется руками по правилу или по знанию о Desktop.
2. `cargo test --test desktop_fixtures` — падение до правки кода и есть смысл кейса.
3. Правило в `desktop/`; отказ несёт человеческую причину (её показывает `doctor`).
4. Мутационная проверка **в копии файла, не через `git checkout`**.

## Мутации, которыми проверен гейт (2026-09-03, все пять роняют тест)

1. Снять храповик «потерял `cliSessionId`» в `export_verdict` → `lost-transcript-withheld`.
2. Снять требование подтверждённой ссылки в `import_verdict` → `import-link-only-predicted`.
3. Взять первый подошедший корень вместо самого длинного → `portable-longest-root-wins`.
4. Не снимать `transcriptUnavailable` при починке → `repair-drops-the-mark`.
5. Не переносить незнакомые ключи дескриптора → круговой тест и три кейса с `keeps`.

Годная мутация бьёт в само правило. Негодная — та, что проходит зелёной, потому что до правила
дело не доходит: так у гейта экспорта ведёт себя «разрешить `sessions/`»
([exportFixtures.md](exportFixtures.md)).
