#!/usr/bin/env python3
"""One-time Amazon login for the Alexa shopping list import of SpesaApp.

Amazon has no public API for Alexa lists any more (closed on 1 July 2024).
This script uses the library of Home Assistant's "Alexa Devices" integration
(aioamazondevices) to register SpesaApp as an Alexa app, and saves the session
to a JSON file. The SpesaApp server (`spesa --alexa <file>`) then renews it by
itself.

The login happens in a real Chrome window: there you handle whatever Amazon
asks (password, captcha, "verify your identity" by email/SMS, 2FA code), as on
any web site. The script only waits for the final authorization code.

Requirements: Python >= 3.13 and Google Chrome (or set CHROME=/path/to/chrome).

    python3.13 -m venv .venv-alexa
    .venv-alexa/bin/pip install aioamazondevices
    .venv-alexa/bin/python tools/alexa_login.py alexa.json

The file contains the keys of your Amazon session: keep it private.
"""

import asyncio
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path
from urllib.parse import parse_qs, urlparse

from aiohttp import ClientSession

from aioamazondevices.api import AmazonEchoApi
from aioamazondevices.structures import AmazonListType, AmazonSaveDataConfig

# Amazon redirects here at the end of the login, with the authorization code.
RETURN_URL = "https://www.amazon.com/ap/maplanding"
CODE_PARAM = "openid.oa2.authorization_code"
DEBUG_PORT = 9339
LOGIN_TIMEOUT = 15 * 60  # seconds


def find_chrome() -> str:
    """Path of Google Chrome / Chromium."""
    candidates = [
        os.environ.get("CHROME", ""),
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        shutil.which("google-chrome") or "",
        shutil.which("chromium") or "",
        shutil.which("chromium-browser") or "",
    ]
    for path in candidates:
        if path and Path(path).exists():
            return path
    sys.exit("Chrome non trovato: installalo o imposta CHROME=/percorso/di/chrome")


async def browser_login(
    start_url: str, cookies: dict[str, str], *, headless: bool = False
) -> str:
    """Open `start_url` in Chrome and return the authorization code.

    `cookies` are set on .amazon.com before loading the page (same as the app).
    """
    profile = tempfile.mkdtemp(prefix="spesa-alexa-chrome-")
    args = [
        find_chrome(),
        f"--user-data-dir={profile}",
        f"--remote-debugging-port={DEBUG_PORT}",
        "--no-first-run",
        "--no-default-browser-check",
        "--window-size=500,800",
    ]
    if headless:
        args.append("--headless=new")
    chrome = subprocess.Popen(
        [*args, "about:blank"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL
    )
    try:
        async with ClientSession() as http:
            page = None
            for _ in range(100):
                try:
                    async with http.get(f"http://127.0.0.1:{DEBUG_PORT}/json") as r:
                        targets = await r.json()
                    page = next((t for t in targets if t["type"] == "page"), None)
                    if page:
                        break
                except OSError:
                    pass
                await asyncio.sleep(0.2)
            if not page:
                sys.exit("Impossibile controllare Chrome")

            async with http.ws_connect(
                page["webSocketDebuggerUrl"], max_msg_size=0
            ) as ws:
                ids = iter(range(1, 1_000_000))

                async def send(method: str, **params) -> None:
                    await ws.send_json(
                        {"id": next(ids), "method": method, "params": params}
                    )

                await send("Network.enable")
                await send("Page.enable")
                for name, value in cookies.items():
                    await send(
                        "Network.setCookie",
                        name=name,
                        value=value,
                        domain=".amazon.com",
                        path="/",
                        secure=True,
                    )
                await send("Page.navigate", url=start_url)

                async def wait_code() -> str:
                    async for msg in ws:
                        event = msg.json()
                        url = ""
                        if event.get("method") == "Network.requestWillBeSent":
                            url = event["params"]["request"]["url"]
                        elif event.get("method") == "Page.frameNavigated":
                            url = event["params"]["frame"]["url"]
                        if url.startswith(RETURN_URL) and CODE_PARAM in url:
                            return parse_qs(urlparse(url).query)[CODE_PARAM][0]
                    sys.exit("Chrome è stato chiuso prima della fine del login")

                return await asyncio.wait_for(wait_code(), LOGIN_TIMEOUT)
    finally:
        chrome.terminate()
        shutil.rmtree(profile, ignore_errors=True)


async def login(out: Path) -> None:
    async with ClientSession() as session:
        # Email and password are typed in the browser, not here.
        api = AmazonEchoApi(
            session,
            "",
            "",
            save_data=AmazonSaveDataConfig(path=Path(tempfile.mkdtemp())),
        )
        lg = api.login
        state = lg._session_state_data  # noqa: SLF001
        verifier = lg._create_code_verifier()  # noqa: SLF001
        url = lg._build_oauth_url(verifier, lg._build_client_id(), "it_IT")  # noqa: SLF001

        print("Si apre Chrome: fai il login ad Amazon e completa le verifiche richieste.")
        code = await browser_login(
            str(url),
            lg._http_wrapper.cookies,  # noqa: SLF001
        )
        print("Login completato, registro SpesaApp come app Alexa...")

        # Same steps as AmazonLogin.login_mode_interactive after the OTP page.
        data = await lg._register_device(  # noqa: SLF001
            {
                "authorization_code": code,
                "code_verifier": verifier,
                "domain": state.domain,
            }
        )
        state.login_stored_data = data
        await lg._domain_refresh_auth_cookies()  # noqa: SLF001
        await lg._register_device_capabilities()  # noqa: SLF001
        await lg.obtain_account_customer_id()
        data.update({"site": state.retail_site_url.human_repr().rstrip("/")})

        # Readable only by the owner: it contains the session keys.
        fd = os.open(out, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
        with os.fdopen(fd, "w") as f:
            json.dump(data, f, indent=2)
        print(f"Login riuscito ({data.get('site')}), salvato in {out}.")

        # Check that the shopping list is reachable (the library refreshes the
        # lists only together with the devices: call its list handler directly).
        await api._todo_handler.update_lists()  # noqa: SLF001
        shop = [lst for lst in api.todo_lists if lst.list_type == AmazonListType.SHOP]
        if not shop:
            sys.exit("Attenzione: la lista della spesa di Alexa non è stata trovata.")
        items = await api.get_todo_list_items(shop[0].id)
        print(f"Lista della spesa di Alexa raggiungibile: {len(items)} elementi.")
        print(f"Avvia il server con: spesa --alexa {out} ...")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit(f"Uso: {sys.argv[0]} <file di output, es. alexa.json>")
    asyncio.run(login(Path(sys.argv[1])))
