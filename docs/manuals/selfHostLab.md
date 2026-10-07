# Стенд своего сервера: как прогнать

> Мануал «как сделать». Как на Mac владельца поднять сервер VibeMemory без кабинета и машину участника, пройти путь
> ключа машины и снести всё обратно.

Стенд проверяет [selfHosting.md](selfHosting.md) так, как его пройдёт клиент: хост поднимается теми же скриптами
`infra/`, участник подключается теми же командами.
Прогонять перед выпуском, который трогает `install`, `connect`, тик сторов команд, консоль хоста или скрипты `infra/`.
Что нашёл первый прогон — [knowledge/design/selfHostLab.md](../knowledge/design/selfHostLab.md).

## 1. Что нужно

- Mac на Apple Silicon
- `limactl` (`brew install lima`) и Docker — у владельца это colima
- Около 4 ГБ памяти на две машины и 20 ГБ на диске `/Volumes/Storage`

Машины живут в `/Volumes/Storage/Caches/lima`, образы — в `/Volumes/Storage/Caches/lima-cache`, ключ стенда и сборка — в
`/Volumes/Storage/Caches/VibeMemory/selfHostLab`.

## 2. Поднять хост

```bash
./infra/selfHostLab.sh up
```

Что делает:
1. Собирает `vibememory` и `vibememory-mcp` текущего дерева под aarch64 Linux в контейнере `rust`
2. Создаёт машины `vmtest-host` и `vmtest-client` с двумя сетями: vzNAT — чтобы их видел Mac, `user-v2` — чтобы они
   видели друг друга
3. Заводит на хосте учётку `vm` с sudo и ключом стенда — то, что выдаёт провайдер — и алиас `vmselftest` в
   `~/.ssh/config` между метками `vibememory selfHostLab`
4. Прогоняет `hostBootstrap.sh` и `hostMcp.sh` и пишет домен консоли

Домен — адрес хоста в сети `user-v2` через sslip.io, например `192-168-104-1.sslip.io`.
Строка «fail2ban не установлен» — ожидаемая: на стенде его не ставят, а на настоящем сервере он ставится по шагу 3
[serverHardeningPrompt.md](serverHardeningPrompt.md).

Повторный `up` безопасен: машины не пересоздаются, скрипты `infra/` идемпотентны.

## 3. Проверить путь ключа машины

```bash
./infra/selfHostLab.sh check
```

Каждый прогон заводит свою команду `lab-<время>` и пересоздаёт машину участника с нуля.
Дальше по шагам [selfHosting.md](selfHosting.md) и [teamSetup.md](teamSetup.md):
- `install`
- `connect --key-request` → `admin key add` → `connect --grant`
- маршрут проекта, сессии до и после него
- отзыв ключа и отказ после него

Каждый шаг печатает `PASS` или `FAIL`, в конце — итог; код выхода не нулевой, если упал хоть один.
Проверки описывают обещанное поведение, и обходов в скрипте нет: дефект роняет прогон, а не пропускается.
На 0.8.1 — 12 PASS.

## 4. Снести

```bash
./infra/selfHostLab.sh down
```

Удаляет обе машины и алиас `vmselftest` из `~/.ssh/config`.
Образы и кэш сборки остаются в `/Volumes/Storage/Caches`: следующий `up` их не скачивает и не собирает заново.

## 5. Если что-то пошло не так

- Вывод `limactl` — в `/Volumes/Storage/Caches/VibeMemory/selfHostLab/limactl.log`
- Зайти на хост: `ssh vmselftest`; на машину участника: `LIMA_HOME=/Volumes/Storage/Caches/lima limactl shell vmtest-client`
- Консоль хоста: `ssh vmselftest sudo /srv/vibememory/bin/vibememory-mcp admin show`
