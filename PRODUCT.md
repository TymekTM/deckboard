# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Tymek (autor, główny użytkownik): zaawansowany użytkownik Windows, streamer/tinkerer,
prowadzi Pulpit jako codzienny driver na własnym komputerze (autostart, zasobnik
systemowy). Druga grupa: posiadacze tabletów z Androidem, którzy chcą tablicy
przycisków do sterowania PC przez Wi-Fi/LAN — w tym użytkownicy starego Deckboarda,
którzy migrują bez zmiany klienta.

## Product Purpose

Pulpit zamienia dowolny tablet w tablicę przycisków do sterowania PC: edytor tablic
(okno desktopowe), tryb dotykowy, serwer sieciowy na porcie 8500, karty dwustanowe
odzwierciedlające żywy stan systemu (głośność, mute, urządzenia audio, postępy
agentów AI). Sukces = pełna parzystość zachowań z oryginalnym Deckboardem
(milestone M3) i codzienna niezawodność jako aplikacji rezydującej w zasobniku.

## Positioning

Natywny Rust-owy następca Deckboarda 3.x, który zachowuje cały ekosystem: stockowy
klient Android łączy się bez konfiguracji, plansze `.boardjson` round-trip bez
zmian, oryginalne rozszerzenia `.asar` działają w osadzonym silniku JS — a pod spodem
jest natywny silnik WASAPI (bez PowerShell), protokół v2 (WebSocket, parowanie QR,
tokeny per urządzenie) i jeden katalog danych `~/pulpitApp`.

## Operating Context

- Codzienna praca: edytor tablic w oknie Tauri 2 + Vue 3, tryb dotykowy na tym samym
  oknie (hotkey, domyślnie Ctrl+Alt+D), zamykanie do zasobnika, autostart HKCU.
- Tablety łączą się przez LAN: stockowy klient Deckboard (legacy socket.io v2) lub
  nowy natywny klient (protokół v2, kody parowania jednorazowe).
- Katalog danych `~/pulpitApp`: `database.db`, `settings.json`, `editor.json`,
  `devices.json`, `extensions/`, `assets/`, `logs/` (rotacja dzienna).
- Integracje: rozszerzenia Deckboard, natywne Discord/Voicemeeter, katalogi AI dev
  (OpenRouter, Anthropic, Codex, Claude, local sums) — docs/aidev.md.
- Weryfikacja zmian: `cargo test --workspace`, `cargo clippy`, `npx vite build`
  w apps/desktop; Android `apps/mobile` testami Gradle.

## Capabilities and Constraints

- Edytor: CRUD tablic i kafli, drag/resize, dual-state, widoczna siatka, live preview
  drugiego stanu, import/eksport `.boardjson` (format Deckboard).
- Tryb dotykowy: lokalne wykonywanie, slider drag, flip stanów na żywo.
- Silnik komend: głośność/mute/urządzenia (WASAPI), makra klawiszowe, pisanie tekstu,
  zrzuty ekranu, lokalne audio, multi-akcje, przełączanie tablic.
- Dwa protokoły na jednym porcie 8500; `settings.json` zachowuje nazwy kluczy
  oryginału dla kompatybilności (w widocznych na przewie stringach stara nazwa
  `deckboard` jest kontraktowa).
- Obecny UI: dark charcoal rail + jasny sidebar + canvas w kolorze tablicy, akcent
  teal #1abc9c, Roboto, jasny system modali — dziedzictwo wyglądu Deckboarda.
- Ustawienia istnieją dziś tylko jako popover w railu (hotkey touch mode, autostart,
  wersja); reszta konfiguracji żyje w plikach JSON bez UI.
- Zdecydowane (2026-09-28, rozmowa o ustawieniach): ustawienia jako pełnoekranowy
  overlay w oknie edytora z własną nawigacją; zakres v1 = rdzeń (Ogólne, Serwer,
  Tablety/parowanie); świat wizualny ustawień może odejść od dziedzictwa Deckboarda.

## Brand Commitments

- Nazwa: Pulpit (rebrand 2026-09-24, ADR-011); stara nazwa `deckboard` zostaje
  wyłącznie w kontraktowych stringach przewu (klucze ustawień, id rozszerzeń).
- Licencja MIT, autor Tymoteusz "TymekTM" Bielski; projekt niezależny od Deckboard.

## Evidence on Hand

- README.md, ROADMAP.md, docs/protocol-v2.md, docs/decisions.md, docs/aidev.md.
- Działający edytor w apps/desktop (App.vue, GridEditor, EditTileModal, BoardModal)
  jako dowód obecnego świata wizualnego.
- Brak: logo (rail używa ikony Font Awesome), materiałów marketingowych, zrzutów
  referencyjnych oryginału. Nie wymyślać ich bez zgody.

## Product Principles

1. Tablet jest powierzchnią, PC jest silnikiem — każdy widok musi działać w duosie
   edytor + tablet na tym samym stanie.
2. Kompatybilność to funkcja, nie dług — kontrakty przewu są nienaruszalne.
3. Natywnie zamiast przez shelle — WASAPI, nie PowerShell; Rust, nie skrypty.
4. Codzienny driver: autostart, zasobnik, logi — niezawodność ponad efekt.
5. Użytkownik posiada swoje pliki — jeden katalog danych, jawne formaty.

## Accessibility & Inclusion

- Tryb dotykowy używany na tablecie: duże cele dotyku, wysoki kontrast na kolorowym
  tle tablicy; reszta UI desktopowa — standardy klawiatury i focus-visible obowiązują.
