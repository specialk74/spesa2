#!/usr/bin/env bash
# Deploy di SpesaApp sul Raspberry Pi: compila, fa il backup, copia, sostituisce,
# riavvia e verifica. La procedura è descritta in README.md ("Deploy sul Raspberry Pi").
#
# Uso: tools/deploy-rpi.sh [--css] [--bot] [--no-build]
#   (nessuna opzione)  server `spesa` + frontend `site`, poi riavvio dei servizi
#   --css              solo il frontend `site` (es. modifiche a style/main.css): niente riavvio
#   --bot              anche `telegram_bot` (lo lancia cron: niente riavvio)
#   --no-build         usa quello che è già compilato in target/
#
# Configurazione: deploy.env nella cartella del progetto (escluso da git),
# vedi deploy.env.example.

set -euo pipefail

cd "$(dirname "$0")/.."

usage() { sed -n '2,13p' "$0" | sed 's/^# \{0,1\}//'; exit "${1:-0}"; }

CSS_ONLY=0 BOT=0 BUILD=1
for arg in "$@"; do
    case "$arg" in
        --css) CSS_ONLY=1 ;;
        --bot) BOT=1 ;;
        --no-build) BUILD=0 ;;
        -h|--help) usage ;;
        *) echo "Opzione sconosciuta: $arg" >&2; usage 1 ;;
    esac
done
if [[ $CSS_ONLY == 1 && $BOT == 1 ]]; then
    echo "--css e --bot non vanno insieme" >&2; exit 1
fi

[[ -f deploy.env ]] || { echo "Manca deploy.env: copia deploy.env.example e completalo" >&2; exit 1; }
# shellcheck source=/dev/null
source deploy.env
: "${PI_HOST:?manca in deploy.env}" "${PI_SERVICES:?manca in deploy.env}" "${PI_PORTS:?manca in deploy.env}"
PI_DIR=${PI_DIR:-spesa-rs}
BUILDER=${BUILDER:-zigbuild}

TARGET=armv7-unknown-linux-musleabihf
BIN_DIR=target/$TARGET/release
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)
STAMP=$(date +%Y%m%d-%H%M%S)

step() { printf '\n\033[1m==> %s\033[0m\n' "$*"; }
# Runs the script read from stdin on the Pi with the given arguments. ssh joins its
# arguments into one command line, so each one is quoted (e.g. "spesa spesa2").
remote() {
    ssh -o BatchMode=yes "$PI_HOST" "bash -s -- $(printf '%q ' "$@")" 2> >(grep -v '^\*\*' >&2)
}

if [[ -n $(git status --porcelain) ]]; then
    echo "Attenzione: ci sono modifiche non committate, vanno sul Pi così come sono."
fi

# --- 1. Compilazione ----------------------------------------------------------
if [[ $BUILD == 1 ]]; then
    step "Compilo il frontend (cargo leptos build --release)"
    cargo leptos build --release
    if [[ $CSS_ONLY == 0 ]]; then
        step "Compilo per il Pi ($BUILDER, $TARGET)"
        case "$BUILDER" in
            zigbuild) cargo zigbuild --release --features ssr --bins --target "$TARGET" ;;
            cross) cross build --release --features ssr --bins --target "$TARGET" ;;
            *) echo "BUILDER sconosciuto: $BUILDER (zigbuild o cross)" >&2; exit 1 ;;
        esac
    fi
fi

[[ -f target/site/pkg/spesa.wasm ]] || { echo "Manca target/site/pkg/spesa.wasm" >&2; exit 1; }
if [[ $CSS_ONLY == 0 ]]; then
    [[ -f $BIN_DIR/spesa ]] || { echo "Manca $BIN_DIR/spesa" >&2; exit 1; }
    # without LEPTOS_OUTPUT_NAME (see .cargo/config.toml) the pages would ask for spesa_bg.wasm
    if ! grep -aq "$VERSION" "$BIN_DIR/spesa"; then
        echo "$BIN_DIR/spesa non contiene la versione $VERSION: è vecchio? (togli --no-build)" >&2; exit 1
    fi
fi
[[ $BOT == 0 || -f $BIN_DIR/telegram_bot ]] || { echo "Manca $BIN_DIR/telegram_bot" >&2; exit 1; }

# --- 2. Backup sul Pi ---------------------------------------------------------
step "Backup sul Pi ($PI_HOST:$PI_DIR/backup, *-prima-$STAMP)"
remote "$PI_DIR" "$STAMP" "$CSS_ONLY" "$BOT" <<'EOF'
set -euo pipefail
cd "$1"; mkdir -p backup
cp -rp site "backup/site-prima-$2"
test -f "backup/site-prima-$2/pkg/spesa.wasm"
echo "  backup/site-prima-$2"
if [[ $3 == 0 ]]; then
    cp -p spesa "backup/spesa-prima-$2" && test -f "backup/spesa-prima-$2"
    echo "  backup/spesa-prima-$2"
fi
if [[ $4 == 1 ]]; then
    cp -p telegram_bot "backup/telegram_bot-prima-$2" && test -f "backup/telegram_bot-prima-$2"
    echo "  backup/telegram_bot-prima-$2"
fi
rm -rf site.new spesa.new telegram_bot.new
EOF

# --- 3. Copia con nomi temporanei ---------------------------------------------
step "Copio i file nuovi (*.new)"
scp -q -o BatchMode=yes -r target/site "$PI_HOST:$PI_DIR/site.new"
[[ $CSS_ONLY == 1 ]] || scp -q -o BatchMode=yes "$BIN_DIR/spesa" "$PI_HOST:$PI_DIR/spesa.new"
[[ $BOT == 0 ]] || scp -q -o BatchMode=yes "$BIN_DIR/telegram_bot" "$PI_HOST:$PI_DIR/telegram_bot.new"

# --- 4. Sostituzione e riavvio ------------------------------------------------
step "Sostituisco i file$([[ $CSS_ONLY == 1 ]] && echo '' || echo " e riavvio: $PI_SERVICES")"
remote "$PI_DIR" "$CSS_ONLY" "$BOT" "$PI_SERVICES" <<'EOF'
set -euo pipefail
cd "$1"
test -f site.new/pkg/spesa.wasm
[[ $2 == 1 ]] || { test -s spesa.new && mv spesa.new spesa; }
[[ $3 == 0 ]] || { test -s telegram_bot.new && mv telegram_bot.new telegram_bot; }
mv site site.old && mv site.new site && rm -rf site.old
if [[ $2 == 0 ]]; then
    # shellcheck disable=SC2086
    sudo -n systemctl restart $4
    sleep 3
fi
EOF

# --- 5. Verifica --------------------------------------------------------------
step "Verifico"
CSS_MD5=$(md5 -q target/site/pkg/spesa.css 2>/dev/null || md5sum target/site/pkg/spesa.css | cut -d' ' -f1)
if remote "$PI_SERVICES" "$PI_PORTS" "$VERSION" "$CSS_MD5" <<'EOF'
ok=1
fail() { echo "  ERRORE: $*"; ok=0; }
for s in $1; do
    state=$(systemctl is-active "$s" || true)
    [[ $state == active ]] && echo "  servizio $s: attivo" || fail "servizio $s: $state"
done
for p in $2; do
    u=http://127.0.0.1:$p
    for page in spesa spesa2; do
        code=$(curl -s -o /dev/null -w '%{http_code}' "$u/$page")
        [[ $code == 200 ]] || fail "$p/$page risponde $code"
    done
    html=$(curl -s "$u/spesa")
    grep -q 'href="/pkg/spesa.wasm"' <<<"$html" || fail "$p: la pagina non chiede /pkg/spesa.wasm (LEPTOS_OUTPUT_NAME?)"
    grep -q "SpesaApp - v <!>$3<" <<<"$html" || fail "$p: la barra non mostra la versione $3"
    [[ $(curl -s "$u/pkg/spesa.css" | md5sum | cut -d' ' -f1) == "$4" ]] || fail "$p: spesa.css diverso da quello compilato"
    lm=$(curl -sI "$u/pkg/spesa.wasm" | tr -d '\r' | sed -n 's/^[Ll]ast-[Mm]odified: //p')
    code=$(curl -s -o /dev/null -w '%{http_code}' -H "If-Modified-Since: $lm" "$u/pkg/spesa.wasm")
    [[ $code == 304 ]] || fail "$p: il WASM in cache non risponde 304 ($code)"
    [[ $ok == 1 ]] && echo "  porta $p: pagine 200, spesa.wasm, v $3, CSS aggiornato, 304"
done
[[ $ok == 1 ]]
EOF
then
    step "Fatto: v$VERSION sul Pi. Dal telefono basta ricaricare la pagina."
else
    step "Verifica FALLITA. Per tornare indietro, sul Pi:"
    echo "  cd $PI_DIR"
    [[ $CSS_ONLY == 1 ]] || echo "  cp backup/spesa-prima-$STAMP spesa.new && mv spesa.new spesa"
    [[ $BOT == 0 ]] || echo "  cp backup/telegram_bot-prima-$STAMP telegram_bot.new && mv telegram_bot.new telegram_bot"
    echo "  rm -rf site && cp -r backup/site-prima-$STAMP site"
    [[ $CSS_ONLY == 1 ]] || echo "  sudo systemctl restart $PI_SERVICES"
    exit 1
fi
