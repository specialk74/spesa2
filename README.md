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

- `/spesa`: prodotti da rimettere in lista (quelli comprati più di una volta), più il form per aggiungere
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

```sh
cargo leptos build --release                    # genera target/site
cargo install cross                             # richiede Docker
cross build --release --features ssr --bins --target armv7-unknown-linux-musleabihf

ssh <utente>@<host> mkdir -p spesa-rs
scp target/armv7-unknown-linux-musleabihf/release/{spesa,telegram_bot} <utente>@<host>:spesa-rs/
scp -r target/site <utente>@<host>:spesa-rs/site
scp alexa.json <utente>@<host>:spesa-rs/       # se usi Alexa
# sul Pi (database.db nella cartella indicata da --path):
~/spesa-rs/spesa --port 9080 --path <cartella-dati> --alexa ~/spesa-rs/alexa.json
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
- La tabella `history` viene creata automaticamente e mantiene le ultime 100 voci, senza
  `DELETE ... LIMIT`. All'avvio il server le aggiunge la colonna `taken_at` (ora in cui il
  prodotto è stato tolto), riempita da un trigger SQLite: funziona anche con l'app Python.
  Le voci registrate prima non hanno l'ora.
- Tra un gruppo di categorie e l'altro c'è uno spazio (nel template Jinja originale non funzionava).
- Le icone PNG sono sostituite da SVG generati in Rust (`src/icons.rs`).
- Rimossi l'endpoint `/autocomplete` (non era usato dalle pagine) e il bundle JS di Bootstrap
  (non serviva).
- **Aggiornamento automatico** delle pagine quando il database cambia (vedi sopra).
- `/` reindirizza a `/spesa`.
