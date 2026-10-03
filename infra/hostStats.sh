#!/usr/bin/env bash
# The product and the host in one report: teams and their plans, trials and when they end, projects with
# their memory and sessions, members, agents and machines, and the host's CPU, memory and disk.
#
# Read-only. Runs on the OWNER'S machine; the database is read through the host's postgres role by peer,
# the store facts from the host's own report (host.json), so nothing secret crosses the wire.
#
#   ./infra/hostStats.sh [--alias vibememory]
set -euo pipefail

readonly DEFAULT_ALIAS=vibememory
readonly SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=15 -o ConnectionAttempts=4)

sshAlias="${VIBEMEMORY_SSH_ALIAS:-$DEFAULT_ALIAS}"
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

while [ "$#" -gt 0 ]; do
  case "$1" in
    --alias) sshAlias="${2:-}"; shift 2 ;;
    -h | --help) printf 'Статистика продукта и сервера.\n\n  ./infra/hostStats.sh [--alias %s]\n' "$DEFAULT_ALIAS"; exit 0 ;;
    *) fail "неизвестный аргумент $1" ;;
  esac
done

# the report is built on the host by Python from stdin: no quoting of the script through ssh
ssh "${SSH_OPTIONS[@]}" "$sshAlias" python3 - <<'PY'
import json, os, shutil, subprocess, time
from datetime import datetime, timezone

def sql(query):
    out = subprocess.run(["sudo", "-n", "-u", "postgres", "psql", "-d", "cabinet", "-At", "-c",
                          f"select coalesce(json_agg(t), '[]') from ({query}) t"],
                         capture_output=True, text=True, check=True).stdout
    return json.loads(out)

def mb(n):
    return "—" if n is None else f"{n / 1048576:.1f} МБ"

def day(s):
    return s[:16].replace("T", " ") if s else "—"

now = datetime.now(timezone.utc)
report = json.loads(subprocess.run(["sudo", "-n", "cat", "/srv/vibememory/access/host.json"],
                                   capture_output=True, text=True, check=True).stdout)
facts = report.get("teams", {})

teams = sql("""
select t.slug, t.name, t.mode, t.adopted, t."createdAt",
  (select u.handle from member m join "user" u on u.id = m."userId" where m."teamId" = t.id and m.role = 'owner' limit 1) as owner,
  (select count(*) from member m where m."teamId" = t.id) as members,
  (select json_agg(json_build_object('source', g.source, 'quota', g."quotaBytes", 'starts', g."startsAt", 'until', g."until", 'price', g."priceRub"))
     from team_grant g where g."teamId" = t.id) as grants
from team t where t."deletedAt" is null order by t."createdAt"
""")
tokens = sql("""
select t.slug, u.handle, a.agent, a.machine, a."createdAt"
from agent_token a join team t on t.id = a."teamId" join "user" u on u.id = a."userId"
where a."revokedAt" is null and t."deletedAt" is null order by t.slug, u.handle, a.agent
""")
machines = sql("""
select u.handle, mc.name, mc.personal, mc."createdAt",
  (select count(*) from machine_key k where k."machineId" = mc.id and k."revokedAt" is null) as keys
from machine mc join "user" u on u.id = mc."userId" order by u.handle, mc.name
""")
users = sql("""select count(*) as total,
  count(*) filter (where "emailVerified") as verified,
  count(*) filter (where "createdAt" > now() - interval '1 day') as day,
  count(*) filter (where "createdAt" > now() - interval '7 days') as week
from "user" """)[0]
waitlist = sql("select count(*) as n from waitlist_entry")[0]["n"]

def plan(grants):
    running = [g for g in (grants or []) if g["starts"] <= now.isoformat() < g["until"]]
    if not running:
        return "free", None
    top = max(running, key=lambda g: int(g["quota"]))
    return top["source"], top

counts = {"free": 0, "trial": 0, "paid": 0, "personal": 0}
print("ПРОДУКТ")
print(f"Пользователей: {users['total']}, почту подтвердили {users['verified']}, новых за сутки {users['day']}, за неделю {users['week']}")
print(f"В очереди ожидания: {waitlist}")
print()
for team in teams:
    kind, top = plan(team["grants"])
    if team["adopted"]:
        label = "личный стор"; counts["personal"] += 1
    elif kind == "free":
        label = "бесплатная, 50 МБ"; counts["free"] += 1
    elif kind == "trial":
        label = f"пробная {mb(int(top['quota']))} до {day(top['until'])}"; counts["trial"] += 1
    else:
        label = f"платная {mb(int(top['quota']))} до {day(top['until'])}, {top['price']} ₽/мес"; counts["paid"] += 1
    host = facts.get(team["slug"], {})
    print(f"■ {team['slug']} «{team['name']}» — {label}; сессии {'вкл' if team['mode'] == 'sync' else 'выкл'}; "
          f"владелец {team['owner']}, участников {team['members']}; создана {day(team['createdAt'])}")
    print(f"  Занято: {mb(host.get('treeBytes'))} (стор с историей {mb(host.get('sizeBytes'))}), последняя запись {day(host.get('lastCommitAt'))}")
    for name, p in sorted((host.get("projectFacts") or {}).items()):
        size, memory = p.get("sizeBytes") or 0, p.get("memoryBytes") or 0
        print(f"    {name}: память {mb(memory)}, сессии {mb(max(size - memory, 0))}")
    agents = [t for t in tokens if t["slug"] == team["slug"]]
    by_user = {}
    for t in agents:
        by_user.setdefault(t["handle"], []).append(t["agent"] + (f"@{t['machine']}" if t["machine"] else ""))
    for handle, list_ in by_user.items():
        print(f"  Агенты {handle}: {len(list_)} — {', '.join(list_)}")
print()
print(f"Команд: {len(teams)} — бесплатных {counts['free']}, пробных {counts['trial']}, платных {counts['paid']}, личных сторов {counts['personal']}")
print(f"Активных токенов агентов: {len(tokens)}")
live = [m for m in machines if m["keys"] > 0]
print(f"Машин подключено: {len(live)} из {len(machines)}")
for m in machines:
    print(f"  {m['handle']}/{m['name']}{' (личная)' if m['personal'] else ''}: ключей {m['keys']}, с {day(m['createdAt'])}")

print()
print("СЕРВЕР")
load = os.getloadavg()
print(f"CPU: {os.cpu_count()} ядра, нагрузка {load[0]:.2f} / {load[1]:.2f} / {load[2]:.2f} (1/5/15 мин)")
meminfo = dict(line.split(":", 1) for line in open("/proc/meminfo"))
kb = lambda key: int(meminfo[key].split()[0]) * 1024
total, available = kb("MemTotal"), kb("MemAvailable")
print(f"RAM: занято {mb(total - available)} из {mb(total)}, свободно {mb(available)}")
disk = shutil.disk_usage("/srv")
print(f"Диск: занято {disk.used / 1e9:.1f} ГБ из {disk.total / 1e9:.1f} ГБ, свободно {disk.free / 1e9:.1f} ГБ ({disk.used * 100 // disk.total}%)")
uptime = float(open("/proc/uptime").read().split()[0])
print(f"Работает без перезагрузки: {int(uptime // 86400)} дн {int(uptime % 86400 // 3600)} ч")
services = ["vibememory-cabinet", "vibememory-mcp", "postgresql", "caddy"]
states = subprocess.run(["systemctl", "is-active", *services], capture_output=True, text=True).stdout.split()
print("Службы: " + ", ".join(f"{s} {st}" for s, st in zip(services, states)))
print(f"Отчёт хоста от {day(report.get('generatedAt'))}")
PY
