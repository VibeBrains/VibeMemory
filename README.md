# VibeMemory

Память и сессии AI-агентов, которые ходят за вами между машинами — и между агентами.

Работали в Claude Code на Mac, закрыли крышку, открыли Windows — та же сессия продолжается,
та же память проекта на месте, список сессий в Claude Desktop совпадает. Без ритуалов:
один `bootstrap` на машину, дальше всё автоматически. Любой MCP-совместимый агент (Claude,
Codex, Gemini CLI, Cursor, Zed, Cline) читает и пишет ту же память через MCP-сервер.

Локально-first, self-hosted, с полной историей в git — ничего не теряется тихо.

## Статус

Этап 1, ядро (2026-09-02): в `crates/vibememory-core` готов модуль naming — кодировка
`projects/<enc>` и имя стора на фикстурах реального формата; движка (хуки, тик) и MCP-сервера ещё
нет. Концепт — [idea.md](idea.md), архитектура —
[docs/spec/architecture.md](docs/spec/architecture.md), план — [docs/roadmap.md](docs/roadmap.md),
проверенные факты про Claude Code, Desktop и облачные папки — [docs/knowledge/](docs/knowledge/README.md).

## Из чего состоит

- **Движок `vibememory`** — один бинарь (Rust) под macOS и Windows: хуки Claude Code, фоновый
  тик, git-транспорт с детерминированным слиянием транскриптов, реконсиляция ссылок,
  миграция, `doctor`.
- **MCP-сервер памяти** — `memory_search / get / save / update` для любого агента поверх того
  же стора.
- **SaaS для команд** — позже, на старт-паке start0.
