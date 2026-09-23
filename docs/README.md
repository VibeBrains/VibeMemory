# Документация VibeMemory

```
docs/
├── roadmap.md              # план и хроника: один чекбокс — одна итерация
├── functional.md           # каталог возможностей продукта (что уже умеет)
├── spec/
│   ├── architecture.md     # целевая схема: стор, транспорт, хуки, тик, MCP, миграция
│   └── stage6Plan.md       # план этапа 6 (SaaS для команд): проект, ждёт решений владельца
├── manuals/                # руководства «как сделать» по шагам
  - [mcpServer.md](manuals/mcpServer.md) — как подключить память VibeMemory к любому агенту по MCP
│   ├── accessSnapshotSpec.md # спека снимка прав /srv/vibememory/access/access.json для модели: поля, правила, коды access check
│   ├── configSpec.md       # спека ~/.vibememory/config.json для модели (сейчас: nameOverrides, ignoreCwd)
│   ├── desktopFixtures.md  # как добавить кейс в fixtures/desktop/, четыре списка кейсов, мутации гейта
│   ├── exportFixtures.md   # как добавить кейс в fixtures/export/, метки provenance, мутационная проверка гейта
│   ├── hostStatusSpec.md   # спека отчёта хоста: applied.json, host.json и ответ status участнику
│   ├── hostSetup.md        # хост стора: ключ (seedKey), bare-репозиторий и зеркало (hostBootstrap), привязка машины (connectStore)
│   ├── linksFixtures.md    # как добавить кейс в fixtures/links/, два раздела файла, годные подделки
│   ├── mergeFixtures.md    # как добавить кейс в fixtures/merge/, скраббер транскриптов, грамматика сценариев
│   ├── namingFixtures.md   # как добавить кейс в fixtures/naming/, метки provenance, оракул, новая версия CLI
│   ├── serverHardeningPrompt.md # промпт агенту соседнего проекта: аудит и ужесточение доступа к серверу, каждый пункт — итог реальной проверки
│   └── memoryRecordsSpec.md # формат памяти: журнал, документ-проекция, правила расхождений
└── knowledge/              # база знаний: проверенные факты и грабли
    ├── README.md           # индекс — запись без строки здесь не существует
    ├── claudeCode/         # CLI: раскладка каталога, формат транскрипта, кодировка projects/<enc>, хуки, нативный кросс-девайс
    ├── claudeDesktop/      # стор сессий Desktop и его поведение
    ├── cloudSync/          # почему облачные папки не транспорт
    ├── git/                # раскладки репозиториев, края discovery, вызов драйвера слияния
    ├── rust/               # грабли крейтов и тулчейна ядра
    ├── design/             # ограничения движка и инварианты слияния транскриптов
    └── research/           # сырые отчёты исследования 2026-09-02 (первоисточник)
```

Имена файлов и папок в `docs/` — camelCase. Исключения только для общепринятых
верхнеуровневых имён: `README.md`, `roadmap.md`, `functional.md`.
