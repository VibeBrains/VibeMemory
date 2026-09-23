# Кабинет на хосте

Кабинет — сайт `app.vibememory.ru`: команды, коды-подарки, приглашения, выдача токенов агентам и
страница владельца хоста. Он живёт на том же хосте, что и сторы, под учёткой `vmcab`, и говорит с
сервером памяти только двумя файлами в `/srv/vibememory/access/`: сам пишет снимок прав
`access.json` ([спека](accessSnapshotSpec.md)), читает отчёт хоста `host.json`
([спека](hostStatusSpec.md)).

Все скрипты запускаются на Mac владельца из корня репозитория. Работа на сервере в каждом — один
ssh-сеанс; повторный запуск безопасен и печатает «без изменений».

## Порядок

1. `./infra/hostBootstrap.sh` — учётки `vmgit`, `vmcab`, каталоги `/srv/vibememory`, sshd
   ([hostSetup.md](hostSetup.md)).
2. `./infra/hostMcp.sh --binary …` — сервер памяти, первый снимок прав, команды хоста.
3. `./infra/backupSetup.sh` — ночной бэкап; повторите его после обновления `infra/hostBackup.sh`,
   он же доставляет скрипт на хост.
4. `./infra/hostCabinet.sh --owner-email <адрес>` — кабинет.
5. `./infra/cabinetPassword.sh` — пароль владельца, вводит сам владелец.
6. `./infra/cabinetSecrets.sh` — ключ Resend, бот Telegram, ссылка «поддержать», вводит сам владелец.
7. Релиз: `./infra/releaseBuild.sh` на Mac, `infra\releaseBuild.ps1` на GPD, затем
   `./infra/hostRelease.sh <версия>`.

## Выкладка: `hostCabinet.sh`

Скрипт собирает кабинет на Mac (`bun run build:code` в `cabinet/`) — на хосте нет ни сборки, ни
проверки типов. На хост едут только `dist/`, манифесты, схема Prisma и миграции; передача
возобновляется после обрыва.

На хосте, по шагам:

- Postgres 17 из PGDG, только `localhost`, `shared_buffers = 64MB`, `max_connections = 20`
- `.env` кабинета — `/home/vmcab/cabinet/.env`, 0600, `vmcab`; секреты (`BETTER_AUTH_SECRET`,
  пароль базы, `OPENAPI_CREDENTIALS`) генерируются на хосте один раз и не печатаются
- Роль `vmcab` — владелец базы `cabinet`; роль `vmgit` — `pg_read_all_data` и вход по peer, без
  пароля: ею ночью снимается дамп
- Bun той версии, на которой кабинет тестируется, под `vmcab`
- Зависимости только production, затем `prisma migrate deploy`
- Импорт владельца из `/srv/vibememory/access/access.json` — снимка, на котором хост уже работает
- Юнит `vibememory-cabinet` на `127.0.0.1:3000`, джейл fail2ban `vibememory-cabinet`, журнал не
  больше 200 МБ, в конце Caddy из [шаблона](../../infra/Caddyfile.tmpl) — два сайта, один файл

**Первый запуск без `--owner-email` не запускает кабинет — так задумано.** Юнит спрашивает
`access:seeded` перед стартом и не поднимается на базе без владельца: кабинет на пустой базе
опубликовал бы пустой снимок поверх настоящего. Импорт берёт владельца (`handle`), личный стор и
строку `tk_legacy` из снимка хоста, адрес — из `--owner-email`; пароля импорт не ставит.

Снимок хоста перед импортом должен описывать только личный стор: `teamCount: 1`, `applied.teamCount`
в `host.json` — тоже 1. Импорт принимает только такой снимок.

## Пароль владельца

```bash
./infra/cabinetPassword.sh
```

Пароль вводится на терминале хоста дважды, без эха, и не проходит ни через аргументы, ни через
файлы. Повторный запуск меняет пароль и закрывает сессии владельца.

## Письма и тревоги

Письма уходят через Resend, срочные тревоги хоста — ещё и в Telegram.

1. Аккаунт Resend, домен `vibememory.ru` подтверждён записями SPF и DKIM, которые Resend покажет.
2. Бот у @BotFather; в его чат владелец пишет `/start`, номер чата — из
   `https://api.telegram.org/bot<токен>/getUpdates` (открыть в браузере самому).
3. Внести всё в кабинет:

```bash
./infra/cabinetSecrets.sh
```

Секреты вводятся без эха и едут на хост через stdin ssh. Пустой ответ оставляет прежнее значение.

4. На странице владельца `Admin → Dashboard` кнопка «Send a test message» отвечает, что сделал
   каждый канал: `sent`, `skipped` или `failed`.

Тревоги: диск ниже 1 ГиБ или 10 %, служба не `active`, бэкап старше 25 часов, команда у 90 % квоты,
проблемы применения снимка, отчёт хоста старше двух часов, снимок не применён 15 минут, снимок не
опубликован сторожем. Одна и та же тревога повторяется не чаще раза в сутки; ушедшая забывается.

## Снимок и сторож

Кабинет публикует снимок после каждого изменения прав, при старте и раз в пять минут. Он **не**
публикует, если в базе нет владельца, если строк команд меньше, чем хост уже применял
(`applied.teamCount`), если `host.json` не читается и если снимок отвергает `access check`. Хост тогда
работает на последнем своём снимке, владелец получает тревогу.

Осознанный обход второго и третьего — например, после восстановления базы из дампа:

```bash
ssh vibememory "sudo -u vmcab -H bash -c 'cd /home/vmcab/cabinet && NODE_ENV=production /home/vmcab/.bun/bin/bun run access:publish --force --reason \"база восстановлена из дампа 2026-09-22\"'"
```

Причина попадает в журнал и в строку публикации. Владельца и бинарь не обходит ничто.

## Восстановление базы из дампа

Дамп лежит в ночном бэкапе (`cabinet.pgdump.age`, [hostSetup.md](hostSetup.md)).

1. На Mac: `age -d -i ~/.vibememory/keys/backup.age cabinet.pgdump.age > cabinet.pgdump`,
   скопировать на хост.
2. На хосте: `sudo systemctl stop vibememory-cabinet`, затем
   `sudo -u vmcab pg_restore --clean --if-exists --no-owner --dbname=cabinet cabinet.pgdump`.
3. `sudo systemctl start vibememory-cabinet` — сторож откажет публиковать: база моложе хоста. Это
   правильно: без отказа кабинет стёр бы из снимка команды, созданные после дампа.
4. Опубликовать осознанно — командой из раздела выше. Каталоги команд, которых нет в базе, хост не
   трогает и показывает как сирот.
5. На странице владельца, раздел «Orphaned directories»: «Take it back» с владельцем команды и
   причиной. Члены, токены и ключи после дампа не возвращаются: участников приглашают заново, и они
   делают `vibememory connect` снова; старые токены отвечают `401`.

## Удалённые команды

Удаление в кабинете мягкое: хост переименовывает стор в `teams/<слаг>.deleted-<дата>.git` и ничего не
стирает. Убрать каталог — решение владельца хоста:

```bash
ssh vibememory "sudo -u vmgit rm -rf /srv/vibememory/teams/<слаг>.deleted-<дата>.git"
```

Следующий отчёт хоста его не покажет, и кабинет уберёт маркер из снимка сам. Слаг не освобождается
никогда.

## Восстановление стора команды

- **`memory`-стор** — из ночного бэкапа: `age -d` бандла и `git clone --bare`, как в
  [hostSetup.md](hostSetup.md); на хост кладётся копией под `vmgit` в `/srv/vibememory/teams/<слаг>.git`.
- **`sync`-стор** — из клона любого участника, **копией, не push'ем**: `pre-receive` отвергнет push
  чужих `machines/*`. У участника `git clone --bare ~/.vibememory/stores/<id>/store /tmp/<слаг>.git`,
  копия на хост под `vm`, затем `sudo install -d -o vmgit -g vibememory -m 2770
  /srv/vibememory/teams/<слаг>.git`, `sudo rsync -a --chown=vmgit:vibememory` содержимого туда же и
  `sudo -u vmgit /srv/vibememory/bin/storeInit.sh /srv/vibememory/teams/<слаг>.git` — хуки клона не
  переносятся. Если строки команды в базе нет — «Take it back» на странице владельца.

## Релизы

```bash
./infra/releaseBuild.sh              # macOS arm64 и x86_64, Linux x86_64 в контейнере
./infra/hostRelease.sh 0.1.0         # проверка сумм и состава, выкладка, index.json
```

На GPD: `powershell -ExecutionPolicy Bypass -File infra\releaseBuild.ps1` — архив Windows и сумма
уходят в тот же `~/releases/incoming/<версия>/`. `hostRelease.sh` отвергает версию целиком, если
сумма не сходится или в архиве не ровно два бинаря (`vibememory`, `vibememory-mcp`, для Windows —
с `.exe`). Опубликованное раздаётся с `https://app.vibememory.ru/dl/<версия>/`, страница «Скачать»
читает `index.json`. Копия сборок Mac — в `/Volumes/Storage/Caches/VibeMemory/releases/<версия>/`.

## Локальная среда тестов

В `cabinet/`: Postgres в Docker — `docker compose up -d db` (порт 5433, базы `cabinet` и
`cabinet-test`); `.env` и `.env.test` задают `VIBEMEMORY_MCP_BIN` — абсолютный путь к
`target/debug/vibememory-mcp` после `cargo build -p vibememory-mcp`, и `ACCESS_DIR` в
`/Volumes/Storage/Caches/VibeMemory/`. Проверка перед коммитом: `bun run check && bun run test`.
