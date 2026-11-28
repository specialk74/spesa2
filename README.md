# SpesaApp (Rust + Leptos)

Port in Rust di [specialk74/Spesa](https://github.com/specialk74/Spesa): lista della spesa web
con notifica Telegram quando la spesa è finita.

Tutto è in Rust, anche l'interfaccia: le pagine sono componenti [Leptos](https://leptos.dev)
generati sul server (SSR) e resi interattivi nel browser tramite WebAssembly.
Niente jQuery né JavaScript scritto a mano; da fuori arriva solo il CSS di Bootstrap.

## Struttura

| File                     | Contenuto |
|--------------------------|-----------|
| `src/model.rs`           | tipi e logica condivisi server/browser: categorie, colori, suggerimenti |
| `src/app.rs`             | interfaccia: pagine, form, autocompletamento |
| `src/api.rs`             | server function chiamate dall'interfaccia |
| `src/db.rs`              | accesso SQLite (solo server) |
| `src/icons.rs`           | icone disegnate in SVG (niente file immagine) |
| `src/alexa.rs`           | importazione dalla lista della spesa di Alexa |
| `tools/alexa_login.py`   | login Amazon (una volta sola) per l'importazione da Alexa |
| `src/main.rs`            | server web (axum) |
| `src/bin/init_db.rs`     | crea il database da `lista.txt` |
| `src/bin/telegram_bot.rs`| notifica Telegram |
| `style/main.css`         | stile (unico file, sopra Bootstrap) |

Il database è lo stesso della versione Python: il `database.db` esistente funziona così com'è.

## Requisiti

```sh
rustup target add wasm32-unknown-unknown
cargo install cargo-leptos --locked
```

## Sviluppo

```sh
cargo leptos watch -- --path ~/spesa-data     # ricompila e ricarica la pagina a ogni modifica
cargo test --features ssr                     # test
```

Poi apri http://localhost:9080/spesa.

## Build e avvio

```sh
cargo leptos build --release
./target/release/spesa --port 9080 --path ~/spesa-data
```

- `/spesa`: prodotti da rimettere in lista (quelli comprati più di una volta), più il form per aggiungere.
  In cima, **"Da ricomprare"** propone i prodotti che è ora di ricomprare in base a quanto spesso li
  prendi: per ogni prodotto calcola l'intervallo tipico tra due acquisti (mediana degli ultimi 5) e lo
  propone dal 90% di quell'intervallo, con l'indicazione "ogni settimana · ultima volta 8 giorni fa".
  La freccia lo mette in lista; la **×** ("non ora") lo nasconde per un altro intervallo. Servono
  almeno due acquisti registrati con l'ora per prodotto
- `/spesa2`: la lista della spesa, raggruppata e colorata per categoria. In fondo, **"Tolti di
  recente"** mostra gli ultimi 20 prodotti tolti, dal più recente, con l'ora ("oggi 10:32").
  La freccia rimette il prodotto in lista se è stato tolto per sbaglio
- il titolo nella barra in alto passa da una pagina all'altra

### Aggiornamento automatico

Le pagine aperte si aggiornano da sole, entro circa mezzo secondo, quando il database cambia,
chiunque lo modifichi: un altro telefono, l'app Python, il bot, `sqlite3` a mano, oppure un nuovo
`database.db` copiato sopra quello vecchio.

- **Server:** una connessione dedicata controlla `PRAGMA data_version` ogni 500 ms
  (`db::watch_changes`) e avvisa i browser con Server-Sent Events su `/api/events`.
- **Browser:** a ogni avviso ricarica i dati, senza perdere ciò che si sta scrivendo.
  Se la connessione cade (server riavviato, telefono in standby), il browser si riconnette
  da solo e ricarica i dati.

### Lista della spesa di Alexa

Con `--alexa` il server controlla la lista della spesa di Alexa ogni 30 secondi (`--alexa-interval`):
ogni prodotto trovato viene messo nella lista di SpesaApp e cancellato da quella di Alexa.
Quindi "Alexa, aggiungi il latte alla lista della spesa" finisce qui.

- Un prodotto già esistente viene riconosciuto anche se il nome ha maiuscole e minuscole diverse
  ("latte soia" → "Latte soia") e mantiene la sua categoria. Un prodotto nuovo va in categoria 0
  (Varie): la correggi dalla lista.
- Amazon ha chiuso l'API ufficiale delle liste il 1° luglio 2024. SpesaApp usa le stesse API interne
  dell'integrazione "Alexa Devices" di Home Assistant: potrebbero cambiare senza preavviso.

**Login (una volta sola, sul Mac).** Serve Python 3.13 o più recente e Google Chrome:

```sh
python3.13 -m venv .venv-alexa
.venv-alexa/bin/pip install aioamazondevices
.venv-alexa/bin/python tools/alexa_login.py alexa.json
```

Si apre una finestra di Chrome con la pagina di accesso di Amazon. Fai il login lì e completa
quello che Amazon chiede (captcha, verifica dell'identità via email o SMS, codice di verifica in
due passaggi). Quando hai finito la finestra si chiude da sola e lo script salva `alexa.json`.

Per controllare che funzioni, senza modificare nulla:

```sh
./target/release/spesa --alexa alexa.json --alexa-check    # mostra la lista di Alexa ed esce
```

Poi:

```sh
./target/release/spesa --port 9080 --path ~/spesa-data --alexa alexa.json
```

`alexa.json` contiene le chiavi della sessione Amazon: è leggibile solo dal proprietario e non va
messo nel repository (è in `.gitignore`). Il server rinnova da solo i cookie scaduti e li salva nel
file. Se il login viene revocato (cambio password, "esci da tutti i dispositivi"), il log mostra
`login Alexa non valido`: rifai il login.

Il server serve anche i file del frontend (`pkg/`). Li cerca, in quest'ordine:
in `--site <dir>`, poi nella cartella `site` accanto all'eseguibile, poi in `target/site`.

Gli altri due programmi:

```sh
cargo build --release --features ssr --bins

# crea database.db da lista.txt (CANCELLA le tabelle esistenti!)
./target/release/init_db --db database.db --lista lista.txt

# da lanciare periodicamente (cron): invia "Spesa finita..." quando la lista si svuota
./target/release/telegram_bot --path <cartella-dati>
```

`telegram_bot` legge `database.db`, `last_number.txt` e `config.ini` nella cartella indicata:

```ini
[DEFAULT]
TELEGRAM_TOKEN = 123456:ABC...
TELEGRAM_CHAT_ID = -100123456
```

## Deploy sul Raspberry Pi

Su un Raspberry Pi a 32 bit (armv7), soprattutto con un sistema operativo datato, la scelta più
sicura è un binario statico musl. Il frontend (WASM) non dipende dall'architettura.
Sostituisci `<utente>@<host>` e le cartelle con quelli del tuo Pi.

### In breve: `tools/deploy-rpi.sh`

Lo script fa tutti i passi descritti sotto: compila, fa il backup sul Pi, copia, sostituisce,
riavvia e verifica. Se la verifica fallisce, stampa i comandi per tornare indietro.

```sh
cp deploy.env.example deploy.env     # una volta sola: indirizzo del Pi, servizi, porte
tools/deploy-rpi.sh                  # server + frontend, con riavvio
tools/deploy-rpi.sh --css            # solo il frontend (es. style/main.css), senza riavvio
tools/deploy-rpi.sh --bot            # anche telegram_bot
```

`deploy.env` è escluso da git. Serve l'accesso ssh con chiave e `sudo` senza password per
`systemctl` sul Pi.

### Cosa serve sul Mac

```sh
rustup target add armv7-unknown-linux-musleabihf
cargo install cargo-zigbuild --locked && brew install zig    # oppure `cross`, che richiede Docker
```

### 1. Compilare

```sh
cargo leptos build --release            # frontend: target/site (JS, WASM, CSS)
cargo zigbuild --release --features ssr --bins --target armv7-unknown-linux-musleabihf
# con cross: cross build --release --features ssr --bins --target armv7-unknown-linux-musleabihf
```

I binari per il Pi finiscono in `target/armv7-unknown-linux-musleabihf/release/`
(`spesa`, `telegram_bot`, `init_db`).

`.cargo/config.toml` imposta `LEPTOS_OUTPUT_NAME = "spesa"`: cargo-leptos lo fa da solo, ma
`zigbuild`/`cross` no. Senza, il server fa chiedere alle pagine `pkg/spesa_bg.wasm`, che non
esiste: le pagine si vedono, ma i bottoni non fanno niente (funzionano solo i link della barra).

### 2. Cosa copiare

| Cosa è cambiato | Da copiare | Riavvio |
|---|---|---|
| solo `style/main.css` | `target/site` | no: i file sono letti a ogni richiesta |
| codice Rust, dipendenze o `version` in `Cargo.toml` | `spesa` **e** `target/site` insieme | sì |
| `src/bin/telegram_bot.rs` (o `db.rs`, `model.rs`) | anche `telegram_bot` | no: lo lancia cron |

Server e WASM vanno aggiornati **insieme**: se la pagina generata dal server e il WASM non
combaciano (anche solo il numero di versione), la pagina si blocca.

### 3. Backup, copia e sostituzione

Prima si fa il backup, poi si copiano i file nuovi con un nome temporaneo e solo alla fine si
sostituiscono quelli vecchi, così il servizio non vede mai file a metà.

```sh
T=$(date +%Y%m%d-%H%M%S)
ssh <utente>@<host> "cd spesa-rs && mkdir -p backup &&
  cp -p spesa backup/spesa-prima-$T && cp -rp site backup/site-prima-$T"

scp target/armv7-unknown-linux-musleabihf/release/spesa <utente>@<host>:spesa-rs/spesa.new
scp -r target/site <utente>@<host>:spesa-rs/site.new
# solo se è cambiato il bot:
# scp target/armv7-unknown-linux-musleabihf/release/telegram_bot <utente>@<host>:spesa-rs/telegram_bot.new

ssh <utente>@<host> "cd spesa-rs &&
  mv spesa.new spesa &&
  mv site site.old && mv site.new site && rm -rf site.old &&
  sudo systemctl restart spesa"
```

La prima installazione: `ssh <utente>@<host> mkdir -p spesa-rs`, poi copia `spesa`,
`telegram_bot`, `target/site` (come `spesa-rs/site`) e, se usi Alexa, `alexa.json`. Avvio:

```sh
~/spesa-rs/spesa --port 9080 --path <cartella-dati> --site ~/spesa-rs/site --alexa ~/spesa-rs/alexa.json
```

Per tenerlo sempre attivo conviene un servizio systemd (nell'esempio sopra si chiama `spesa`)
con questa riga come `ExecStart`.

### 4. Verificare

Sul Pi:

```sh
systemctl is-active spesa                                     # active
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:9080/spesa2     # 200
curl -s http://127.0.0.1:9080/spesa | grep -o 'href="/pkg/spesa.wasm"'   # NON spesa_bg.wasm
curl -s http://127.0.0.1:9080/spesa | grep -o 'SpesaApp - v <!>[^<]*'   # versione nuova
sudo journalctl -u spesa -n 20 --no-pager
```

Dal telefono basta ricaricare la pagina: il server manda `Cache-Control: no-cache`, quindi il
browser controlla ogni volta se i file sono cambiati (se no riceve un "304 Not Modified" vuoto,
senza riscaricare il WASM).

### Tornare indietro

```sh
cd ~/spesa-rs
cp backup/spesa-prima-<data> spesa.new && mv spesa.new spesa
rm -rf site && cp -r backup/site-prima-<data> site
sudo systemctl restart spesa
```

## Differenze rispetto all'originale

- **Interfaccia**: un solo insieme di componenti al posto dei due template quasi identici.
  Il CSS è un solo file, i colori per gruppo sono calcolati in Rust (prima c'erano 21 classi CSS).
- **Autocompletamento** scritto in Rust, usabile con tastiera, mouse e touch.
  I suggerimenti per i prodotti mostrano anche la categoria (es. `66 · Freddi`).
  Ogni campo categoria propone le categorie di riferimento; sulla lista, scegliere una categoria la salva subito.
- **Nessun ricaricamento di pagina**: aggiungere, prendere e cambiare categoria aggiornano la pagina
  sul posto, e il passaggio tra le due pagine avviene nel browser.
- I campi categoria accettano solo numeri: un valore non valido viene evidenziato e non viene inviato.
- I nomi dei prodotti vengono normalizzati anche quando li rinomini (apostrofi tipografici, spazi
  iniziali e finali).
- La tabella `history` viene creata automaticamente e conserva gli ultimi 400 giorni (prima: le
  ultime 100 voci), per conoscere anche le abitudini mensili. All'avvio il server le aggiunge la colonna `taken_at` (ora in cui il
  prodotto è stato tolto), riempita da un trigger SQLite: funziona anche con l'app Python.
  Le voci registrate prima non hanno l'ora.
- Tra un gruppo di categorie e l'altro c'è uno spazio (nel template Jinja originale non funzionava).
- Le icone PNG sono sostituite da SVG generati in Rust (`src/icons.rs`).
- Rimossi l'endpoint `/autocomplete` (non era usato dalle pagine) e il bundle JS di Bootstrap
  (non serviva).
- **Aggiornamento automatico** delle pagine quando il database cambia (vedi sopra).
- `/` reindirizza a `/spesa`.
