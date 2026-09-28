---
version: 1
slug: "apps-desktop-src-components-settingsoverlay-vue"
primary_target: "apps/desktop/src/components/SettingsOverlay.vue"
related_targets: ["apps/desktop/src/App.vue"]
---

# Surface brief: Ustawienia (SettingsOverlay)

Scope: desktop editor (apps/desktop) - pelnoekranowy overlay ustawien; mode Operate.
Audience: autor-codzienny driver + tablety; rzadkie, zadaniowe wejscia.

## Job
v1 sekcje: Ogolne (autostart, zamkniecie-do-zasobnika jako fakt, hotkey), Serwer (status, port, klienci), Tablety (parowanie QR legacy, kod v2 jednorazowy). Zrodlo prawdy: istniejace komendy Tauri; port read-only (brak komendy backendu), zarzadzanie urzadzeniami v2 = follow-up.

## Direction contract
THESIS: Ustawienia jak sciana tresci, nie formularz: kafel jest zywym kontenerem stanu (status, QR, kod parowania), a fokus jest jedynym mechanizmem odsaniajacym sterowanie; reszta sciany przygasa. Odmawia domyslnego "sidebar + wiersze formularzy".

OWN-WORLD: Glebokie neutralne tlo, kafle o ton jasniejsze z 1px linia, tekst z dwoch stopni szarosci, jeden akcent teal (#1abc9c) wylacznie na fokusu i akcjach glownych; Roboto (fontsource) + mono tabularne dla wartosci (port, IP, hotkey, kod); promien kafla 12px, jeden miekki cien z offsetem na kafle w focu, deklaracja elevacji raz.

STORY: Otwieram ustawienia z railu; widze trzy rzedy zywych kafli stanu; tap powieksza kafel i odsania akcje (przelacznik autostartu, edycja hotkey z walidacja, generowanie kodu z odliczaniem); reszta gaśnie; ESC wraca do edytora.

FIRST VIEWPORT: Belka "Ustawienia" + ESC; trzy rzedy kafli (Ogolne: Uruchamianie, Hotkey; Serwer: Status, Port; Tablety: Stock client QR, v2 kod); pierwszy kafel w focu; tresc przewijana gdy przekracza okno; zywe dane z Tauri od pierwszej sekundy.

FORM: Sciana kafli z fokusem - challenger pop-culture-shelf-streaming-title-card-wall (karta "SCIANA") z reki edc9113e; wybrany przez uzytkownika ponad kierunek przydzielony; code-led.

FINISH: unreviewed and undocumented is unfinished; this build ends with the finish review, the verdict, DESIGN.md, and every shipping raster carrying its provenance.

## Open decisions
Zmiana portu i lista urzadzen v2 po stronie backendu (poza v1 UI); brak zmian w kontraktach wire; touch mode nietkniety.
