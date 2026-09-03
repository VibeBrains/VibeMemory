# CLAUDE.md — VibeMemory

> **База — в глобальном `$CLAUDE_CONFIG_DIR/CLAUDE.md`** (действует во всех проектах). Здесь —
> только специфика VibeMemory.

## Что это

Движок синхронизации сессий и памяти Claude Code между машинами + MCP-память для любых
агентов. Концепт — [idea.md](idea.md), архитектура — [docs/spec/architecture.md](docs/spec/architecture.md),
план — [docs/roadmap.md](docs/roadmap.md). Проверенные факты про поведение Claude Code, Desktop и
облачных папок — [docs/knowledge/](docs/knowledge/README.md): **прежде чем опираться на
предположение о том, как ведёт себя CLI или Desktop, проверить там** — большинство
очевидных предположений уже опровергнуты фактом.

## Чего нельзя делать никогда

- **Писать в `.claude.json`** и вообще трогать горячие per-machine файлы (`sessions/`,
  ключи, кэши). У файла история порчи, переносимого в нём нет.
- **Перелинковывать или сливать транскрипт живой сессии.** Живость — межмашинная
  (heartbeat в сторе), локальный pid-реестр не годится.
- **«Новее побеждает» для памяти.** `memory/*.md` только keep-both + карантин.
- **Выпускать секреты за пределы машины.** Защитный `.gitignore` + белый список экспорта;
  тест на это обязателен.
- **Полагаться на монотонность timestamp** между машинами — часы расходятся.
- **Git LFS** в сторе — ломает union-слияние и квоту.

## Стек

Rust workspace: `crates/vibememory-core` (стор, имена, слияние — чистая логика без I/O),
`crates/vibememory-cli` (бинарь: хуки, тик, install, doctor, migrate), `crates/vibememory-mcp`
(MCP-сервер памяти). Ноль рантайм-зависимостей на целевой машине. SaaS-слой — start0.

## Два репозитория — не путать

- **Код продукта** — этот репозиторий.
- **Стор данных владельца** — отдельный приватный git на своём хосте (клон в
  `~/.vibememory/store`), зеркало в приватный GitHub делает хост.

## Проверка перед завершением задачи

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test`. В `cargo test`
входят гейты: документации (ссылки, индекс knowledge, дерево мануалов, camelCase, формат
завершённых пунктов roadmap), приватности фикстур и property-тесты законов слияния — руками эти
правила больше не проверяются. Драйверы
слияния — только через фикстуры реального формата JSONL (`fixtures/`). Тесты не запускать при
ошибках компиляции.

Гейт «ядро без I/O» — `disallowed-methods`/`disallowed-types`/`disallowed-macros` в корневом
`clippy.toml`; крейт, который делает I/O по замыслу, отключает его одной строкой
`#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::disallowed_macros)]`
в своём `lib.rs`/`main.rs`.

## Правила кода (следствия `clippy::pedantic` под `-D warnings`)

- Имена и кодировка — только через `fixtures/naming/*.json` ([manuals/namingFixtures.md](docs/manuals/namingFixtures.md)),
  слияние — только через `fixtures/merge/{mergeScenarios,keepBothScenarios}.json`
  ([manuals/mergeFixtures.md](docs/manuals/mergeFixtures.md)):
  новых unit-тестов с литеральными путями и строками транскрипта не заводить. Транскрипт-фикстуры
  попадают в репозиторий только через `fixtures/merge/scrubTranscript.js`.
- В `tests/*.rs` нет `pub`-элементов (`missing_docs`), а хелперы теста разрешают panic/unwrap
  файловым `#![allow(...)]` (ключи `allow-*-in-tests` их не видят).
- Не-ASCII в Rust-литералах — только `\u{…}` (`unicode_not_nfc`); идентификаторы и CamelCase в
  doc-комментариях — в бэктиках (`doc_markdown`).
- Грабли крейтов и тулчейна — [knowledge/rust/crossPlatformPaths.md](docs/knowledge/rust/crossPlatformPaths.md).

## Ветки

`next` — повседневная; `main` — только выпущенное.
