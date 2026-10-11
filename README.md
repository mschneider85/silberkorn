# Silberkorn

RAW-Entwickler und Bildverwaltung in Rust.

## Starten

```bash
cargo run --release -p sk-app -- /pfad/zu/raws
```

Der erste Build dauert einige Minuten (u. a. wegen der ONNX-Inferenz-Bibliothek tract).
Ohne Pfad wird der zuletzt geöffnete Ordner verwendet. Falls wgpu (Vulkan) nicht startet,
fällt die App automatisch auf OpenGL zurück; erzwingen mit `SILBERKORN_RENDERER=glow`.

### Windows

Aus WSL/Linux für Windows bauen (einmalig `rustup target add x86_64-pc-windows-gnu` und mingw-w64,
z. B. `sudo apt install gcc-mingw-w64-x86-64`):

```bash
cargo build --release -p sk-app --target x86_64-pc-windows-gnu
```

Ergebnis: `target/x86_64-pc-windows-gnu/release/silberkorn.exe` – eine einzelne Datei ohne
zusätzliche DLLs. Unter Windows nativ geht auch `cargo build --release -p sk-app` mit der MSVC-Toolchain.
Einstellungen und Stile liegen dann in `%APPDATA%\silberkorn`, Katalog, Modelle, Lensfun-Profile und
Thumbnails in `%LOCALAPPDATA%\silberkorn`.

Kommandozeilen-Entwicklung (ohne GUI):

```bash
SK_ADJ='{"exposure":0.5,"shadows":40}' cargo run --release -p sk-core --example develop -- bild.ARW ausgabe/
```

## Aufbau

| Crate | Inhalt |
|---|---|
| `sk-core` | RAW-Dekodierung (rawler), Farbmathematik, Entwicklungs-Pipeline, Sidecars, Export |
| `sk-ai`   | Masken aus Bildinhalt: Motiv (U²-Net via tract/ONNX), Himmel, Magic Brush, Kantenverfeinerung |
| `sk-app`  | egui/wgpu-Oberfläche, Hintergrund-Worker |

Pipeline (Arbeitsfarbraum: lineares Rec.2020):

1. Geometrie: Zuschnitt, Geraderichten, Spiegeln, Drehen, Keystone, Objektivkorrektur
   (Verzeichnung, CA, Vignettierung – manuell oder per Lensfun-Profil)
2. Weißabgleich mit Highlight-Rekonstruktion, Kameramatrix
3. Rauschreduzierung (Luminanz: Guided Filter, Farbe: Chroma-Glättung)
4. Belichtung, lokales HDR, Klarheit/Struktur, Kontrast, Basiskurve
5. Farbe in Oklab: Color Editor (8 Farbbereiche), Color Balance, Sättigung/Dynamik, Schwarzweiß mit Tonung
6. Helligkeit, Levels, Kurven
7. Schärfen, Vignette, Körnung
8. Ausgabefarbraum (sRGB, Adobe RGB, Display P3, ProPhoto) mit eingebettetem ICC-Profil

Die frühen Stufen werden zwischengespeichert, sodass Ton- und Farbregler flüssig bleiben. Beim
Hineinzoomen rendert ein Hintergrund-Worker den sichtbaren Ausschnitt in voller Auflösung.

`SK_TRACE=1` gibt die Zeiten der einzelnen Stufen aus; `cargo run --release -p sk-core --example bench -- <raw>`
misst die Pipeline.

### Ebenen & Masken

Lokale Anpassungen (Tab „Ebenen“) wirken als Deltas auf die Hintergrund-Einstellungen und werden pro
Pixel mit Maske × Deckkraft gemischt; überlappende Ebenen addieren sich. Masken bestehen aus
Pinselstrichen (Größe, Härte, Fluss, Radierer), linearen und radialen Verläufen, „Füllen“ und
„Invertieren“. Sie werden in Quellkoordinaten gespeichert und bleiben deshalb nach Zuschnitt,
Drehen oder Objektivkorrektur am Bildinhalt.

„Reparieren“ übernimmt Struktur von einer automatisch gewählten Quelle und gleicht Farbe/Helligkeit an
die Umgebung an; „Klonen“ kopiert 1:1. Quelle und Ziel lassen sich verschieben.

### Auswahl aus Bildinhalt

- **Motiv auswählen:** neuronales Netz U²-Net-p (Apache-2.0, 4,6 MB). Das Modell wird im Tab „Ebenen“
  auf Wunsch nach `~/.local/share/silberkorn/models/` geladen (oder `SILBERKORN_MODEL=<pfad>`).
- **Himmel auswählen:** helle, strukturarme, blaue oder neutrale Bereiche, die mit dem oberen Rand verbunden sind.
- **Magic Brush (J):** Klick wählt zusammenhängende, farbähnliche Bereiche (Toleranz einstellbar).
- **Luma Range** und **Farbbereich** (mit Pipette) verfeinern jede Maske.
- **Style Brushes:** Pinsel-Presets (Aufhellen, Abdunkeln, Wärmer, Haut glätten, Dunst entfernen, …).

Weitere Funktionen: **Dunst entfernen** (Dark-Channel-Prior, global und pro Ebene), **Speed Edit**
(Taste halten + Mausrad), **geteilte Vorher/Nachher-Ansicht** und **mehrere Ausgaberezepte**, die
in einem Durchgang verarbeitet werden (das RAW wird pro Bild nur einmal entwickelt).

### Objektivprofile

Lensfun-XML-Dateien werden aus `~/.local/share/silberkorn/lensfun`, `~/.local/share/lensfun` und
`/usr/share/lensfun` gelesen. Im Tab „Objektiv“ lässt sich die Datenbank auch herunterladen
(Quelle: lensfun auf GitHub, Lizenz CC BY-SA 3.0; sie wird nicht mit Silberkorn ausgeliefert).

Einstellungen werden nicht-destruktiv als `<Bild>.sk.json` neben dem RAW gespeichert.

## Bibliothek

Silberkorn arbeitet wahlweise mit dem **Katalog** oder mit **Sessions**.

### Sessions

Eine Session ist ein in sich geschlossener Projektordner mit eigener Datenbank:

```
Hochzeit Müller/
  Hochzeit Müller.sksession   Session-Datenbank (Alben, Favoriten, Index)
  Capture/                    „Aufnahmen“ – neue Bilder
  Selects/                    „Auswahl“ – die Auswahl
  Output/                     „Ausgabe“ – verarbeitete Dateien
  Trash/                      „Papierkorb“ – bis er geleert wird
  Silberkorn/Cache/           Vorschaubilder der Session
```

- **Datei → Neue Session** (Strg+N) legt den Ordner samt Unterordnern an, **Session öffnen**
  (Strg+Umschalt+O) öffnet eine `.sksession`-Datei; zuletzt verwendete Sessions stehen im
  Menü, und die zuletzt offene Session wird beim Start wieder geöffnet. Eine Session lässt sich
  auch als Programmargument übergeben (`silberkorn "Hochzeit Müller"`).
- **Importieren** kopiert RAWs samt Sidecars in „Aufnahmen“ (die Originale bleiben liegen).
- **Strg+J** verschiebt die Auswahl nach „Auswahl“, **Strg+Entf** in den Papierkorb; per
  Rechtsklick auf ein Vorschaubild oder im Menü „Bild“ geht es auch nach „Aufnahmen“, in
  einen Favoriten oder einen beliebigen Ordner. Sidecars wandern mit, bei Namensgleichheit
  wird `-2`, `-3` … angehängt.
- **Session-Favoriten:** häufig genutzte Ordner, gespeichert in der Session.
- **Verarbeiten** schreibt relative Ausgabeordner in den Session-Ordner, das Standardrezept
  also nach `Output/`.
- **Papierkorb leeren** löscht die Bilder darin endgültig (mit Rückfrage).
- **Session schließen** kehrt zum Katalog und zum zuvor geöffneten Ordner zurück.
- **Portabel:** Die Session-Datenbank speichert Pfade relativ zum Session-Ordner. Eine Session
  lässt sich daher als Ganzes verschieben, umbenennen oder auf einen anderen Rechner kopieren
  (auch zwischen Windows und Linux), ohne dass Alben, Favoriten oder der Index verloren gehen.
  Nur Favoriten außerhalb des Session-Ordners bleiben absolute Pfade.
- **Vorschaubilder** einer Session liegen in `Silberkorn/Cache/Thumbnails` im Session-Ordner
  und wandern mit; Bilder außerhalb von Sessions nutzen den Cache unter `~/.cache/silberkorn`.

Alben, Smart-Alben, Keywords und Suche funktionieren in einer Session genauso wie im Katalog,
beziehen sich aber nur auf die Bilder der Session.

### Katalog

- **Ordner** werden wie eine Session direkt durchsucht und im Hintergrund indexiert.
- **Katalog** (SQLite, `~/.local/share/silberkorn/catalog.sqlite`, überschreibbar mit
  `SILBERKORN_CATALOG`): „Ordner importieren“ nimmt einen Ordnerbaum auf; danach stehen
  „Alle Bilder“, „Letzter Import“, Katalogordner und Alben zur Verfügung.
- **Alben** (manuell) und **Smart-Alben** (gespeicherter Filter), Rechtsklick für Umbenennen/Löschen.
- **Filter & Suche:** Freitext (Name, Kamera, Objektiv, Keyword), Mindestbewertung, Farb-Label,
  Keyword; Sortierung nach Name, Aufnahmedatum oder Bewertung.
- **Keywords** im Tab „Metadaten“ (mit Vorschlägen aus dem Katalog); Bewertung, Label und
  Keywords wirken auf die ganze Auswahl.
- **Stile:** Anpassungen als Preset speichern (`~/.config/silberkorn/styles`) und auf die Auswahl anwenden.
- **Thumbnail-Cache** unter `~/.cache/silberkorn/thumbs`.

Sidecars bleiben die maßgebliche Quelle; der Katalog ist ein Index und kann jederzeit neu aufgebaut werden.

Daten aus der Zeit als „Capture Room“ werden übernommen: Ordner `capture-room` in `~/.config`,
`~/.local/share` und `~/.cache` werden beim ersten Start nach `silberkorn` verschoben, und alte
`<Bild>.crs.json`-Sidecars werden gelesen und beim nächsten Speichern durch `.sk.json` ersetzt.

## Tastenkürzel

| Taste | Funktion |
|---|---|
| ← / → | vorheriges / nächstes Bild |
| 0–5 | Bewertung |
| 6 / 7 / 8 / 9 | Farb-Label Rot / Gelb / Grün / Blau |
| G | Raster ↔ Viewer |
| Strg+Z / Strg+Y | Rückgängig / Wiederholen |
| Strg+Umschalt+C / V | Anpassungen kopieren / auf Auswahl anwenden |
| Strg+D | Auswahl verarbeiten (Export) |
| Strg+N / Strg+Umschalt+O | neue Session / Session öffnen |
| Strg+J / Strg+Entf | nach „Auswahl“ / in den Papierkorb verschieben (Session) |
| Strg+A | alle auswählen · Strg/Umschalt+Klick: Mehrfachauswahl |
| Strg+[ / Strg+] | drehen |
| Y | Vorher/Nachher |
| E | Belichtungswarnung |
| W | Weißabgleich-Pipette |
| Z / Doppelklick | Einpassen ↔ 100 % · Mausrad: zoomen · Ziehen: verschieben |
| C / Enter / Esc | Zuschnitt-Werkzeug / übernehmen / beenden |
| B / N / L / R | Pinsel / Radierer / linearer / radialer Verlauf |
| H / S | Reparieren / Klonen · Entf: Punkt löschen |
| M | Maske anzeigen · [ / ]: Pinselgröße · mittlere Maustaste: verschieben |
| J | Magic Brush |
| U | geteilte Vorher/Nachher-Ansicht |
| Q / A / D / F / V / T + Mausrad | Speed Edit: Belichtung / Kontrast / Sättigung / Lichter / Tiefen / Temperatur (auf der aktiven Ebene, sonst global) |

Doppelklick auf einen Reglernamen setzt den Regler zurück.

## Roadmap

- **Phase 1 (fertig):** RAW laden/anzeigen, Weißabgleich, Belichtung, HDR, Levels, Kurven, Sättigung/Dynamik, Drehen, Bewertung/Farb-Labels, Kopieren/Anwenden, Undo, Export JPEG/TIFF/PNG
- **Phase 2 (fertig):** Katalog (SQLite), Thumbnail-Cache, Alben/Smart-Alben, Keywords, Suche, Sortierung, Stile, Rasteransicht
- **Phase 3 (fertig):** Zuschnitt/Geraderichten/Keystone, Schärfen, Rauschreduzierung, Objektivkorrektur (manuell + Lensfun), Color Editor, Color Balance, Schwarzweiß, Klarheit/Struktur, lokales HDR, Körnung/Vignette, Auto-Anpassungen, echtes 100 % (Kachel-Rendering), Highlight-Rekonstruktion, Ausgabe-Farbräume mit ICC
- **Phase 4 (fertig):** Ebenen und Masken (Pinsel, Radierer, linearer/radialer Verlauf, Deckkraft), Reparieren/Klonen
- **Phase 5 (fertig):** Luma Range, Farbbereichsmasken, Style Brushes, KI-Motivmaske (ONNX), Himmelmaske, Magic Brush, Dunst entfernen, Speed Edit, Vergleichsansicht, mehrere Ausgaberezepte
