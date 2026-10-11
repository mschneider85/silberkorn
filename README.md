# Capture Room

RAW-Entwickler und Bildverwaltung in Rust.

## Starten

```bash
cargo run --release -p cr-app -- /pfad/zu/raws
```

Ohne Pfad wird der zuletzt geöffnete Ordner verwendet. Falls wgpu (Vulkan) nicht startet,
fällt die App automatisch auf OpenGL zurück; erzwingen mit `CAPTURE_ROOM_RENDERER=glow`.

Kommandozeilen-Entwicklung (ohne GUI):

```bash
CR_ADJ='{"exposure":0.5,"shadows":40}' cargo run --release -p cr-core --example develop -- bild.ARW ausgabe/
```

## Aufbau

| Crate | Inhalt |
|---|---|
| `cr-core` | RAW-Dekodierung (rawler), Farbmathematik, Entwicklungs-Pipeline, Sidecars, Export |
| `cr-app`  | egui/wgpu-Oberfläche, Hintergrund-Worker |

Pipeline: Kamera-RGB → Weißabgleich (Kelvin/Tint) → Kameramatrix → lineares sRGB →
Belichtung, HDR, Kontrast → Basiskurve (Film) → Sättigung/Dynamik → sRGB → Helligkeit, Levels, Kurven.

Einstellungen werden nicht-destruktiv als `<Bild>.crs.json` neben dem RAW gespeichert.

## Bibliothek

- **Ordner** werden wie eine Session direkt durchsucht und im Hintergrund indexiert.
- **Katalog** (SQLite, `~/.local/share/capture-room/catalog.sqlite`, überschreibbar mit
  `CAPTURE_ROOM_CATALOG`): „Ordner importieren“ nimmt einen Ordnerbaum auf; danach stehen
  „Alle Bilder“, „Letzter Import“, Katalogordner und Alben zur Verfügung.
- **Alben** (manuell) und **Smart-Alben** (gespeicherter Filter), Rechtsklick für Umbenennen/Löschen.
- **Filter & Suche:** Freitext (Name, Kamera, Objektiv, Keyword), Mindestbewertung, Farb-Label,
  Keyword; Sortierung nach Name, Aufnahmedatum oder Bewertung.
- **Keywords** im Tab „Metadaten“ (mit Vorschlägen aus dem Katalog); Bewertung, Label und
  Keywords wirken auf die ganze Auswahl.
- **Stile:** Anpassungen als Preset speichern (`~/.config/capture-room/styles`) und auf die Auswahl anwenden.
- **Thumbnail-Cache** unter `~/.cache/capture-room/thumbs`.

Sidecars bleiben die maßgebliche Quelle; der Katalog ist ein Index und kann jederzeit neu aufgebaut werden.

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
| Strg+A | alle auswählen · Strg/Umschalt+Klick: Mehrfachauswahl |
| Strg+[ / Strg+] | drehen |
| Y | Vorher/Nachher |
| E | Belichtungswarnung |
| W | Weißabgleich-Pipette |
| Z / Doppelklick | Einpassen ↔ 100 % · Mausrad: zoomen · Ziehen: verschieben |

Doppelklick auf einen Reglernamen setzt den Regler zurück.

## Roadmap

- **Phase 1 (fertig):** RAW laden/anzeigen, Weißabgleich, Belichtung, HDR, Levels, Kurven, Sättigung/Dynamik, Drehen, Bewertung/Farb-Labels, Kopieren/Anwenden, Undo, Export JPEG/TIFF/PNG
- **Phase 2 (fertig):** Katalog (SQLite), Thumbnail-Cache, Alben/Smart-Alben, Keywords, Suche, Sortierung, Stile, Rasteransicht
- **Phase 3:** Zuschnitt/Geraderichten, Schärfen, Rauschreduzierung, Objektivkorrektur (Lensfun), Color Editor, Klarheit/Struktur, echtes 100 % (Kachel-Rendering), Highlight-Rekonstruktion, DCP/ICC-Profile
- **Phase 4:** Ebenen und Masken (Pinsel, Verlauf), Reparieren/Klonen
- **Phase 5:** Luma Range, Farbbereichsmasken, Style Brushes, KI-Masken (ONNX)
