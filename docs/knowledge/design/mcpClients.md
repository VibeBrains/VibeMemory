# Клиенты MCP: как каждый подключает сервер

**30.09.2026.** Проверено по документации клиентов и по настоящим файлам клиентов, стоящих на Mac владельца.
Повод — DSH два дня подключался: взял токен на одно имя агента, а сервер звал под другим, и всё выглядело зелёным.

## Правило

Движок не пишет в файлы настроек клиентов — так же, как не пишет в `.claude.json`.
Формат у каждого свой и меняется с версиями, а молча испорченный чужой конфиг хуже подсказки.
`vibememory mcp-config <клиент>` печатает фрагмент под эту машину, вставляет его человек.
Подключать нужно на каждой машине: регистрация лежит в машинном файле клиента.

## Что проверено

| Клиент | Где | Команда | `cwd` | После правки | Источник |
|---|---|---|---|---|---|
| Codex (CLI, расширение IDE, приложение ChatGPT) | `~/.codex/config.toml`, `[mcp_servers.<имя>]`: `command`, `args`, `env`, `cwd` | `codex mcp add <имя> [--env K=V] -- <команда>`; флага `--cwd` нет | есть | новая сессия; приложение — Settings → MCP servers → Restart | learn.chatgpt.com/docs/extend/mcp, config-reference, `codex-rs/cli/src/mcp_cmd.rs`; живой `config.toml` Mac |
| Gemini CLI | `~/.gemini/settings.json`, `mcpServers` | `gemini mcp add -s user <имя> <команда> -- <аргументы>`; без `-s user` — сервер проекта | есть | `/mcp reload` | github.com/google-gemini/gemini-cli, docs/tools/mcp-server.md |
| Cursor | `~/.cursor/mcp.json`, `mcpServers`, `type: "stdio"` | нет (только deeplink) | не документирован | перезапуск | cursor.com/docs/context/mcp |
| Claude Desktop | macOS `~/Library/Application Support/Claude/claude_desktop_config.json`, Windows `%APPDATA%\Claude\…` | нет | не документирован | закрыть полностью и открыть | modelcontextprotocol.io/docs/develop/connect-local-servers |
| ChatGPT в браузере | локальные серверы не запускает | — | — | — | developers.openai.com/api/docs/guides/developer-mode, plugins/build/auth |
| DeepSeek Harness | `~/.dsh/profiles/<профиль>/cordis.patch.yml`, `insert` с `name: "@deepseek-ai/dsh-mcp-client"`: `serverName`, `transport`, `command`, `args`, `cwd` | нет | есть | перезапуск приложения | живой файл профиля на Mac |
| VibeIDE | подключает сам, `--agent vibeide`, `cwd` — домашний каталог; своя запись `vibememory` в `~/.vibeide/mcp.json` сильнее | — | есть | перезагрузка окна после установки | `vibeMemoryServerDiscovery.ts`, `mcpService.ts` |
| VibeIDEA | подключает сама, `--agent vibeidea`: ACP-агентам в `session/new`, прямому чату | — | ACP — нет | не нужно | `MemoryServerOffer.kt`, `AgentPanel.kt` |

**ChatGPT в браузере** подключает только удалённые серверы (SSE, streaming HTTP) и только с OAuth или без входа.
Постоянный токен в заголовке он не передаёт — документация говорит это прямо.
Удалённый сервер памяти требует токен, поэтому из браузера он недоступен, пока у хоста нет OAuth.
Приложение ChatGPT на компьютере запускает серверы из файла Codex — ему подходит запись Codex.

## Не проверено

- Пути Codex, Gemini CLI и Cursor под Windows: документация пишет `~`; `%USERPROFILE%` выведен, а не прочитан.
- Подхватывает ли работающая сессия Codex правку `config.toml`.
- Путь профиля DSH под Windows.
- Бюджет файлов инструкций DSH (65 536 байт по умолчанию) поднимает строка `agent-instructions` того же
  `cordis.patch.yml` — её печатает `mcp-config dsh`, разбор — [sharedRules.md](sharedRules.md).

## Найдено у соседей

- **VibeIDEA:** своя запись `vibememory` в `.vibe/mcp.json` уходит агенту вместе с найденной, а не вместо неё: дубль.
  Прямой чат не смотрит на выключатель `use_custom_mcp`.
- **VibeIDE:** сервер, поставленный после запуска IDE, подхватывается только после перезагрузки окна или правки `mcp.json`.
- Оба переданы хендоффами в их память.

## Как узнать, что клиент подключён

Сервер памяти при старте пишет время и свою версию в `~/.vibememory/clients/<агент>`.
`doctor` перечисляет каждое имя и у каждого токена пишет, запускался ли сервер под его именем.
Это единственный признак, одинаковый для всех клиентов, и чужих конфигов он не читает.
