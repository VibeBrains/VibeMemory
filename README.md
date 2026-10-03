<h1 align="center">VibeMemory</h1>

<p align="center">
  <strong>Общая память и сессии ИИ-агентов: между машинами, между агентами, на всю команду.</strong>
</p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-AGPL--3.0-blue.svg" alt="AGPL-3.0" /></a>
  <a href="https://github.com/VibeBrains/VibeMemory/releases"><img src="https://img.shields.io/badge/версия-0.7.0-green.svg" alt="Версия" /></a>
  <img src="https://img.shields.io/badge/macOS%20·%20Windows%20·%20Linux-lightgrey.svg" alt="macOS, Windows, Linux" />
  <a href="https://vibememory.ru"><img src="https://img.shields.io/badge/сайт-vibememory.ru-purple.svg" alt="vibememory.ru" /></a>
</p>

<p align="center">
  macOS и Linux: <code>curl -fsSL https://app.vibememory.ru/install.sh | sh</code><br />
  Windows (PowerShell): <code>irm https://app.vibememory.ru/install.ps1 | iex</code>
</p>

---

## Что такое VibeMemory?

Агенты умные, но память у них короткая.
Сессия Claude Code живёт на одном компьютере, выводы одного агента не видит другой, а знания одного разработчика не доходят до команды.

VibeMemory это чинит:
- **Сессия переезжает на другой компьютер** — закрыли ноутбук, открыли другую машину, та же сессия продолжается
- **Память проекта общая для любых агентов** — Claude Code, Codex, Cursor, Gemini CLI, VibeIDE, VibeIDEA, DeepSeek Harness читают и пишут одно и то же
- **Знание одного доходит до всей команды** — поиск по истории всех сессий проекта, чьими бы они ни были

Один бинарь на Rust, ноль зависимостей на машине.
Стор — обычный git на вашем сервере: полная история, ничего не теряется тихо.

---

## Свой сервер или наш

| | Свой сервер | [vibememory.ru](https://vibememory.ru) |
|---|---|---|
| **Цена** | Бесплатно | Бесплатно до 50 МБ, дальше тарифы |
| **Где данные** | У вас, в вашем контуре | На нашем сервере в России |
| **Настройка** | Скрипты из `infra/` и мануалы | Ничего: зарегистрировались и подключили агента |
| **Управление** | Консоль хоста: команды, участники, токены, ключи машин | Веб-кабинет в браузере |
| **Обновления** | `vibememory update` | `vibememory update` |
| **Поддержка** | Issues на GitHub | Напрямую |
| **Веб-кабинет на своём сервере** | Платное внедрение | — |

Код открыт целиком: организация может поставить всё у себя и ни от кого не зависеть.
Не хотите заниматься сервером — приходите на [vibememory.ru](https://vibememory.ru).

---

## Возможности

Здесь — главное. Полный каталог — [docs/functional.md](docs/functional.md).

- **Синхронизация сессий Claude Code** между macOS, Windows и Linux: хуки, фоновый тик раз в две минуты, детерминированное слияние транскриптов, список сессий в Claude Desktop совпадает на всех машинах.
- **Сервер памяти по MCP** — `memory_search`, `memory_get`, `memory_save`, `memory_update` и `history_search` для любого MCP-агента; `vibememory mcp-config <агент>` печатает готовое подключение.
- **Сессии любого агента в общей истории** — `vibememory session put` принимает сессию в простом формате JSONL; агента без своих хуков регистрирует `session agent add`, и его логи отдаёт тик.
- **DeepSeek Harness одной командой** — `vibememory session agent add --agent dsh-desktop --preset dsh --backfill`.
- **Команды** — общий стор команды на сервере, права участников и агентов, сессии по переключателю.
- **Память без потерь** — записи памяти не перезаписываются «кто новее», конфликт сохраняет обе версии.
- **Секреты не уезжают** — белый список экспорта, файл с токеном агента остаётся на машине.
- **Живая сессия не ломается** — сессию, продолжающуюся на другой машине, движок не трогает; промпт в устаревшую копию придерживается.
- **`vibememory doctor`** — проверка всего: хуки, тик, стор, токены, агенты, версия сервера памяти.
- **Обновление одной командой** — `vibememory update`, с проверкой суммы архива.

---

## Установка

**На свою машину:**

```bash
curl -fsSL https://app.vibememory.ru/install.sh | sh
```

```powershell
irm https://app.vibememory.ru/install.ps1 | iex
```

Подключить агента к памяти: `vibememory mcp-config` покажет список, `vibememory mcp-config claude-code` — готовую команду.

**Свой сервер** — по шагам в [selfHosting.md](docs/manuals/selfHosting.md): три скрипта ставят хост, дальше командами,
участниками, агентами и машинами управляет консоль хоста:

```bash
vibememory-mcp admin team add acme --owner alice --sessions
vibememory-mcp admin member add acme bob
vibememory-mcp admin token issue acme bob claude-code   # грант → на машине bob: vibememory connect --grant
```

Веб-кабинет на своём сервере — платное внедрение: i@borodatych.ru.

**Из исходников:**

```bash
git clone https://github.com/VibeBrains/VibeMemory.git
cd VibeMemory
cargo build --release
```

Бинари — `target/release/vibememory` и `target/release/vibememory-mcp`, затем `./target/release/vibememory install`.

---

## Документация

- [docs/functional.md](docs/functional.md) — что умеет продукт
- [docs/spec/architecture.md](docs/spec/architecture.md) — архитектура
- [docs/manuals/](docs/README.md) — мануалы и спеки форматов: их можно целиком отдать своей модели
- [docs/knowledge/](docs/knowledge/README.md) — проверенные факты о Claude Code, Desktop и облачных папках
- [docs/roadmap.md](docs/roadmap.md) — план и сделанное

Родственные продукты: [VibeIDE](https://github.com/VibeBrains/VibeIDE) (IDE на базе VS Code) и [VibeIDEA](https://github.com/VibeBrains/VibeIDEA) (IDE класса PhpStorm) подключают память VibeMemory сами.

---

## Поддержать проект

Если VibeMemory оказался полезным — буду рад благодарности 🙏

<a href="media/QR-Code.jpg" target="_blank" rel="noopener noreferrer">
  <img src="media/QR-Code.jpg" width="120" alt="QR-код для поддержки проекта" />
</a>

---

## Лицензия

[GNU AGPL-3.0](LICENSE).

Пользоваться, ставить у себя, менять под себя — бесплатно, в том числе внутри компании.
Если изменённую версию дают пользоваться другим по сети, её исходники открываются под той же лицензией.

Нужна лицензия без условий AGPL — для закрытого продукта или сервиса на базе VibeMemory — напишите: i@borodatych.ru.

Участие в разработке — [CONTRIBUTING.md](CONTRIBUTING.md), уязвимости — [SECURITY.md](SECURITY.md).
