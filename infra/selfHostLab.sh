#!/usr/bin/env bash
# A self-hosted VibeMemory on this Mac: two Debian VMs under Lima, a host without the cabinet and a member's machine,
# Built the way docs/manuals/selfHosting.md tells a customer to build one, so the manual is what gets tested
#
#   up     — both VMs, binaries of the current tree for aarch64 Linux, the host through hostBootstrap.sh and hostMcp.sh,
#            the console with the host's domain
#   check  — a team of its own with sessions on, and a fresh member machine walking the machine-key path: key request,
#            grant, clone, a session reaching the host, revocation and the refusal after it; every step says PASS or FAIL
#   down   — both VMs and the ssh alias removed; the images and the build cache stay in /Volumes/Storage/Caches
#
# The checks state the behaviour the product promises, so a defect fails the run instead of being worked around
#
# Runs on the OWNER'S Mac (Apple Silicon): limactl, docker (colima) and ssh are needed
# The VMs live in /Volumes/Storage/Caches/lima, Lima's image downloads in /Volumes/Storage/Caches/lima-cache
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
readonly ROOT
readonly LAB=/Volumes/Storage/Caches/VibeMemory/selfHostLab
readonly LIMA_DIR=/Volumes/Storage/Caches/lima
readonly LIMA_DOWNLOADS=/Volumes/Storage/Caches/lima-cache
readonly HOST_VM=vmtest-host
readonly CLIENT_VM=vmtest-client
readonly TEMPLATE=template:debian-13
readonly ALIAS=vmselftest
readonly RUST_IMAGE=rust:1.97-bookworm
readonly OWNER=labowner
# Each check makes a team of its own: a machine's records in a team's store outlive the machine, and a new run
# Under an old team would inherit the live sessions of the last one
readonly TEAM_PREFIX=lab
readonly TEAM_OWNER=alice
readonly MEMBER=bob
readonly MACHINE=laptop
readonly FIXTURE="$ROOT/fixtures/merge/cli255Session.jsonl"
readonly FIXTURE_CWD=/private/tmp/scratch/work
readonly FIXTURE_SESSION=11111111-1111-4111-8111-111111111111
# vzNAT lets the Mac reach a VM, but Apple's NAT keeps VMs apart; user-v2 joins them to each other
readonly VZNAT_PREFIX=192.168.64.
readonly USERV2_PREFIX=192.168.104.
# A name for an address without touching /etc/hosts: sslip.io answers 192-168-104-1.sslip.io with 192.168.104.1
readonly NAME_SERVICE=sslip.io
readonly BEGIN_MARK="# >>> vibememory selfHostLab >>>"
readonly END_MARK="# <<< vibememory selfHostLab <<<"
readonly ADMIN="sudo /srv/vibememory/bin/vibememory-mcp admin"
# How many failing ticks pause a store cycle: the third one does (guard::TickState)
readonly FAILING_TICKS=3

export LIMA_HOME="$LIMA_DIR"

say() { printf '%s\n' "$*"; }
fail() { printf 'Ошибка: %s\n' "$*" >&2; exit 1; }

passed=0
failed=0
pass() { passed=$((passed + 1)); say "PASS  $*"; }
flunk() { failed=$((failed + 1)); say "FAIL  $*"; }
summary() {
  say ""
  say "Итог: $passed PASS, $failed FAIL"
  [ "$failed" -eq 0 ]
}

usage() {
  cat <<TEXT
Стенд своего сервера VibeMemory на этом Mac: хост без кабинета и машина участника.

  ./infra/selfHostLab.sh up      поднять хост по мануалу selfHosting.md
  ./infra/selfHostLab.sh check   пройти путь ключа машины на свежей машине участника
  ./infra/selfHostLab.sh down    снести обе машины и алиас ssh
TEXT
}

onHost() { ssh -o BatchMode=yes "$ALIAS" "$@"; }
onClient() { limactl shell "$CLIENT_VM" bash -lc "$1"; }

vmExists() { limactl list -q 2>/dev/null | grep -qx "$1"; }

# The address of a VM in one network, picked by its prefix: interface names differ between images
vmAddress() {
  limactl shell "$1" ip -4 -o addr show | awk -v prefix="$2" '{split($4, a, "/"); if (index(a[1], prefix) == 1) print a[1]}'
}

dashed() { printf '%s' "$1" | tr . -; }

# Lima downloads images into os.UserCacheDir, which on macOS is always ~/Library/Caches; the link keeps them out of it
ensureDownloadsLink() {
  local link="$HOME/Library/Caches/lima"
  mkdir -p "$LIMA_DOWNLOADS"
  if [ -L "$link" ]; then
    return
  fi
  if [ -d "$link" ]; then
    cp -R "$link/." "$LIMA_DOWNLOADS/"
    rm -rf "$link"
  fi
  ln -s "$LIMA_DOWNLOADS" "$link"
}

# Both networks from the first start: added later, they make cloud-init take the VM for a new instance,
# And it makes new ssh host keys
createVm() {
  local name="$1"
  if vmExists "$name"; then
    return
  fi
  limactl create --tty=false --name="$name" --vm-type=vz --cpus=2 --memory=2 --disk=10 --mount-none --containerd=none \
    --set '.networks = [{"vzNAT": true}, {"lima": "user-v2"}]' "$TEMPLATE" >>"$LAB/limactl.log" 2>&1 ||
    fail "limactl create $name — подробности в $LAB/limactl.log"
  limactl start --tty=false "$name" >>"$LAB/limactl.log" 2>&1 || fail "limactl start $name — подробности в $LAB/limactl.log"
}

buildBinaries() {
  mkdir -p "$LAB/bin" "$LAB/cargo-target"
  docker run --rm -v "$ROOT":/src -v "$LAB/cargo-target":/target -e CARGO_TARGET_DIR=/target -w /src \
    "$RUST_IMAGE" cargo build --release -q -p vibememory-cli -p vibememory-mcp
  cp "$LAB/cargo-target/release/vibememory" "$LAB/cargo-target/release/vibememory-mcp" "$LAB/bin/"
}

writeAlias() {
  local address="$1" config="$HOME/.ssh/config"
  removeAlias
  {
    printf '\n%s\n' "$BEGIN_MARK"
    printf 'Host %s\n' "$ALIAS"
    printf '    HostName %s\n' "$address"
    printf '    User vm\n'
    printf '    IdentityFile %s\n' "$LAB/id_ed25519"
    printf '    IdentitiesOnly yes\n'
    printf '    StrictHostKeyChecking accept-new\n'
    printf '    UserKnownHostsFile %s\n' "$LAB/known_hosts"
    printf '%s\n' "$END_MARK"
  } >> "$config"
}

removeAlias() {
  local config="$HOME/.ssh/config"
  [ -f "$config" ] || return 0
  awk -v begin="$BEGIN_MARK" -v end="$END_MARK" '
    $0 == begin { skip = 1; next }
    $0 == end { skip = 0; next }
    !skip { print }
  ' "$config" > "$config.selfHostLab" && cat "$config.selfHostLab" > "$config" && rm -f "$config.selfHostLab"
}

up() {
  for tool in limactl docker ssh ssh-keygen; do
    command -v "$tool" >/dev/null || fail "нет команды $tool"
  done
  ensureDownloadsLink
  mkdir -p "$LAB"
  say "1/6 Бинари текущего дерева под aarch64 Linux"
  buildBinaries

  say "2/6 Машины $HOST_VM и $CLIENT_VM"
  local fresh=0
  vmExists "$HOST_VM" || fresh=1
  createVm "$HOST_VM"
  createVm "$CLIENT_VM"
  local macSide memberSide domain
  macSide=$(vmAddress "$HOST_VM" "$VZNAT_PREFIX")
  memberSide=$(vmAddress "$HOST_VM" "$USERV2_PREFIX")
  [ -n "$macSide" ] && [ -n "$memberSide" ] || fail "у $HOST_VM нет адресов в $VZNAT_PREFIX и $USERV2_PREFIX"
  domain="$(dashed "$memberSide").$NAME_SERVICE"
  printf '%s\n' "$domain" > "$LAB/domain"
  say "    хост: $macSide для Mac, $memberSide для участника, домен $domain"

  say "3/6 Учётка vm с sudo и ключом — то, что выдаёт провайдер"
  [ -f "$LAB/id_ed25519" ] || ssh-keygen -q -t ed25519 -N '' -C vibememory-selfhostlab -f "$LAB/id_ed25519"
  [ "$fresh" -eq 0 ] || : > "$LAB/known_hosts"
  local pub
  pub=$(cat "$LAB/id_ed25519.pub")
  limactl shell "$HOST_VM" sudo bash -c "
    id vm >/dev/null 2>&1 || useradd -m -s /bin/bash vm
    printf 'vm ALL=(ALL) NOPASSWD:ALL\n' > /etc/sudoers.d/vm && chmod 440 /etc/sudoers.d/vm
    install -d -m 700 -o vm -g vm /home/vm/.ssh
    printf '%s\n' '$pub' > /home/vm/.ssh/authorized_keys
    chown vm:vm /home/vm/.ssh/authorized_keys && chmod 600 /home/vm/.ssh/authorized_keys"
  writeAlias "$macSide"
  onHost true

  say "4/6 hostBootstrap.sh"
  "$ROOT/infra/hostBootstrap.sh" --alias "$ALIAS" | sed 's/^/    /'

  say "5/6 hostMcp.sh"
  "$ROOT/infra/hostMcp.sh" --binary "$LAB/bin/vibememory-mcp" --owner "$OWNER" --alias "$ALIAS" \
    --domain "$domain" --app-domain "app.$domain" | sed 's/^/    /'

  say "6/6 Консоль: домен"
  onHost "$ADMIN init --domain $domain" | sed 's/^/    /'
  say ""
  say "Хост готов. Дальше: ./infra/selfHostLab.sh check"
}

# A session of the fixture under a new id, placed where Claude Code would write it for the project
placeSession() {
  local session="$1"
  onClient "P=\$HOME/work/project; ENC=\$(printf %s \"\$P\" | sed 's/[^A-Za-z0-9]/-/g'); mkdir -p \$P ~/.claude/projects/\$ENC
    sed -e \"s#$FIXTURE_CWD#\$P#g\" -e 's#$FIXTURE_SESSION#$session#g' /tmp/session.jsonl > ~/.claude/projects/\$ENC/$session.jsonl"
}

# One hook of the engine with the input Claude Code gives it
hook() {
  local event="$1" subcommand="$2" session="$3"
  onClient "P=\$HOME/work/project; ENC=\$(printf %s \"\$P\" | sed 's/[^A-Za-z0-9]/-/g')
    T=\$HOME/.claude/projects/\$ENC/$session.jsonl
    printf '{\"session_id\":\"%s\",\"transcript_path\":\"%s\",\"cwd\":\"%s\",\"hook_event_name\":\"%s\"}' \\
      $session \"\$T\" \"\$P\" $event | VIBEMEMORY_DIR=\$HOME/.vibememory ~/.vibememory/bin/vibememory hook $subcommand >/dev/null"
}

# A finished session as Claude Code makes one: SessionStart, which links a project routed to a team into its store,
# Then the transcript where the hook left the project's directory, then SessionEnd
startSession() {
  local session="$1"
  hook SessionStart session-start "$session"
  placeSession "$session"
  hook SessionEnd session-end "$session"
}

hostHasSession() {
  onHost "sudo git --git-dir=/srv/vibememory/teams/$TEAM.git ls-tree -r --name-only HEAD" | grep -q "/$1.jsonl$"
}

# Whether this machine still counts a session as running, by the record the hooks and the tick keep in the clone
stillLive() {
  onClient "cat ~/.vibememory/stores/$TEAM/store/machines/$MEMBER-$MACHINE/live.json 2>/dev/null" | grep -q "\"$1\""
}

# The member's keys on this machine for this check's team, by `admin show`: revoked ones are not listed
memberKeys() {
  onHost "$ADMIN show" | grep "$MEMBER's $MACHINE, opens $TEAM" | grep -o 'mk_[a-z0-9]*' || true
}

newSessionId() { uuidgen | tr '[:upper:]' '[:lower:]'; }

check() {
  vmExists "$HOST_VM" || fail "стенда нет: ./infra/selfHostLab.sh up"
  local engine
  TEAM="$TEAM_PREFIX-$(date +%s)"
  onHost "$ADMIN team add $TEAM --owner $TEAM_OWNER --sessions && $ADMIN member add $TEAM $MEMBER" >/dev/null
  say "Команда $TEAM с сессиями, участник $MEMBER"

  say ""
  say "Хост"
  if onHost 'systemctl is-enabled --quiet vibememory-repack.timer'; then
    pass "ночная упаковка личного стора стоит таймером systemd"
  else
    flunk "ночной упаковки личного стора нет: в образе нет cron, а таймера hostBootstrap.sh не поставил"
  fi
  # Expanded on the member's machine, not here
  # shellcheck disable=SC2088
  engine='~/.vibememory/bin/vibememory'

  say "Свежая машина участника"
  limactl delete --force "$CLIENT_VM" >>"$LAB/limactl.log" 2>&1 || true
  createVm "$CLIENT_VM"
  limactl copy "$LAB/bin/vibememory" "$LAB/bin/vibememory-mcp" "$FIXTURE" "$CLIENT_VM:/tmp/"
  onClient "mv /tmp/$(basename "$FIXTURE") /tmp/session.jsonl
    sudo DEBIAN_FRONTEND=noninteractive apt-get update -qq && sudo DEBIAN_FRONTEND=noninteractive apt-get install -y -qq git >/dev/null"

  say ""
  say "Установка по teamSetup.md: программы, потом движок"
  onClient "cd /tmp && ./vibememory install" >/dev/null
  if onClient "test -x $engine"; then pass "install кладёт программы"; else flunk "install не положил программы"; fi

  say ""
  say "Ключ машины"
  local request grant answer
  request=$(onClient "$engine connect --key-request 2>/dev/null" | head -n 1)
  case "$request" in ssh-ed25519\ *) pass "connect --key-request печатает открытый ключ" ;; *) flunk "connect --key-request: $request" ;; esac
  grant=$(printf '%s\n' "$request" | onHost "$ADMIN key add $MEMBER $MACHINE $TEAM" 2>/dev/null)
  if [ -n "$grant" ]; then pass "admin key add выдаёт грант"; else flunk "admin key add не выдал гранта"; fi
  printf '%s\n' "$grant" > "$LAB/grant.json"
  limactl copy "$LAB/grant.json" "$CLIENT_VM:/tmp/grant.json"
  answer=$(onClient "$engine connect --grant --machine-id $MEMBER-$MACHINE < /tmp/grant.json 2>&1" || true)
  if printf '%s' "$answer" | grep -q "^connected: team $TEAM"; then
    pass "connect --grant сразу после install настраивает движок и подключает машину"
  else
    flunk "connect --grant после install: $(printf '%s' "$answer" | grep '^connect:' | head -n 1)"
    summary
    return
  fi
  if onClient "git -C ~/.vibememory/stores/$TEAM/store config core.sshCommand" | grep -q "StrictHostKeyChecking=yes"; then
    pass "клон ходит своим ключом и ключом хоста из гранта"
  else
    flunk "у клона нет своей команды ssh со строгой проверкой ключа хоста"
  fi

  say ""
  say "Сессия"
  local before after
  before=$(newSessionId)
  placeSession "$before"
  onClient "$engine route add ~/work/project --to $TEAM" >/dev/null
  after=$(newSessionId)
  startSession "$after"
  if stillLive "$after"; then
    flunk "закончившаяся сессия в настоящем каталоге проекта ещё числится живой — тик не импортирует проект"
  else
    pass "SessionEnd снимает сессию с живых"
  fi
  onClient "$engine tick" >/dev/null 2>&1 || true
  if hostHasSession "$after"; then pass "сессия, начатая после route add, доезжает до хоста"; else flunk "сессии после route add нет на хосте"; fi
  if hostHasSession "$before"; then flunk "сессия до route add уехала в команду"; else pass "сессия до route add осталась на машине"; fi

  say ""
  say "Правила и навыки"
  local doctor
  # an agent that reads ~/.dsh lives here: the person's rules are assembled for it too
  onClient "mkdir -p ~/.dsh"
  onClient "printf 'Гонять тесты перед коммитом.\\n' | $engine rule add --level project --id lab-tests --title 'Lab tests' ~/work/project" >/dev/null
  onClient "printf 'Отвечать коротко.\\n' | $engine rule add --level personal --id lab-tone --title 'Lab tone' ~/work/project" >/dev/null
  onClient "$engine tick" >/dev/null 2>&1 || true
  if onHost "sudo git --git-dir=/srv/vibememory/teams/$TEAM.git ls-tree -r --name-only HEAD" | grep -q "^projects/project/rules/lab-tests.md$"; then
    pass "проектное правило уезжает на хост команды"
  else
    flunk "проектного правила нет на хосте"
  fi
  if onClient "grep -q 'Гонять тесты перед коммитом' ~/work/project/.claude/rules/vm-lab-tests.md && grep -q 'Гонять тесты перед коммитом' ~/work/project/AGENTS.local.md"; then
    pass "проектное правило разложено в рабочий каталог для Claude Code и DSH"
  else
    flunk "проектного правила нет в .claude/rules или AGENTS.local.md рабочего каталога"
  fi
  if onClient "grep -q 'Отвечать коротко' ~/.claude/rules/vm-lab-tone.md && grep -q 'Отвечать коротко' ~/.dsh/AGENTS.md"; then
    pass "личное правило у Claude Code и в собранном ~/.dsh/AGENTS.md"
  else
    flunk "личного правила нет у агентов машины"
  fi
  onClient "printf 'Как у команды.\\n' | $engine rule add --level team --id lab-style --title 'Lab style' ~/work/project" >/dev/null
  for _ in $(seq "$FAILING_TICKS"); do onClient "$engine tick" >/dev/null 2>&1 || true; done
  # doctor exits non-zero when something needs a person: its words are read, not its status
  doctor=$(onClient "$engine doctor 2>&1" || true)
  if printf '%s' "$doctor" | grep -q "(rulesDenied)"; then
    pass "хост не принимает командное правило от рядового участника: rulesDenied"
  else
    flunk "командное правило участника не отвергнуто хостом: $(printf '%s' "$doctor" | grep "^team" | head -n 1)"
  fi
  onClient "$engine store reclone $TEAM" >/dev/null 2>&1 || true
  onClient "$engine tick" >/dev/null 2>&1 || true
  doctor=$(onClient "$engine doctor 2>&1" || true)
  if printf '%s' "$doctor" | grep -q "^team     $TEAM — in step"; then
    pass "store reclone возвращает стор в работу"
  else
    flunk "после store reclone стор не в строю: $(printf '%s' "$doctor" | grep "^team" | head -n 1)"
  fi

  say ""
  say "Отзыв ключа"
  local key
  # Nothing left to send: the next ticks only fetch, and only a refused fetch can say the host turned the machine away
  onClient "$engine tick" >/dev/null 2>&1 || true
  key=$(memberKeys | tail -n 1)
  onHost "$ADMIN key revoke $key" >/dev/null
  sleep 3
  if onHost "sudo grep -q '$key' /home/vmgit/.ssh/authorized_keys"; then
    flunk "ключ $key остался у vmgit после отзыва"
  else
    pass "после отзыва ключа у vmgit нет"
  fi
  for _ in $(seq "$FAILING_TICKS"); do onClient "$engine tick" >/dev/null 2>&1 || true; done
  doctor=$(onClient "$engine doctor 2>&1" || true)
  if printf '%s' "$doctor" | grep -q "^team     $TEAM — in step"; then
    flunk "машине нечего отправлять, хост её не пускает, а doctor пишет «$TEAM — in step»"
  else
    pass "отказ хоста виден в doctor, даже когда машине нечего отправлять"
  fi
  startSession "$(newSessionId)"
  for _ in $(seq "$FAILING_TICKS"); do onClient "$engine tick" >/dev/null 2>&1 || true; done
  doctor=$(onClient "$engine doctor 2>&1" || true)
  if printf '%s' "$doctor" | grep -Eq "^team     $TEAM — [0-9]+ failing run"; then
    pass "отказ при отправке виден в doctor"
  else
    flunk "отказ при отправке не виден в doctor: $(printf '%s' "$doctor" | grep "^team" | head -n 1)"
  fi

  summary
}

down() {
  for name in "$CLIENT_VM" "$HOST_VM"; do
    if vmExists "$name"; then
      limactl delete --force "$name" >>"$LAB/limactl.log" 2>&1
      say "$name снесена"
    fi
  done
  removeAlias
  rm -f "$LAB/known_hosts" "$LAB/domain" "$LAB/grant.json"
  say "Алиас $ALIAS убран из ~/.ssh/config; образы и кэш сборки остались в /Volumes/Storage/Caches"
}

case "${1:-}" in
  up) up ;;
  check) check ;;
  down) down ;;
  -h | --help | "") usage ;;
  *) fail "неизвестная команда $1 (--help покажет список)" ;;
esac
