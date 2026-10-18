# Performance-Backlog (Stand 18.10.2026)

Was aus der Performance-Durchsicht vom Oktober 2026 noch offen ist. Umgesetzt ist der Rest in den Commits `3b95481..0fb3220`.
Zu jedem Punkt steht hier, was heute passiert, was sich ändern würde, wie aufwendig das ist und warum er zurückgestellt wurde.

Zum Entscheiden ein `x` ins Kästchen setzen. Für Fragen oder Ergänzungen gibt es die Zeile „Input“.

**Aufwand:** klein = unter einer Stunde · mittel = einige Stunden · groß = ein Tag oder mehr

---

## Brauchen eine Entscheidung

### 1. Indexieren liest die ganze RAW-Datei
**Heute:** Für die Metadaten (`raw::read_meta`) öffnet rawler die Datei mit `MAP_POPULATE`, also werden 30–60 MB je Bild gelesen, obwohl ein paar Kilobyte reichen würden. Auf USB-Laufwerken und Netzfreigaben bestimmt das die Dauer des Imports.
**Ziel:** Nur die TIFF/EXIF-Struktur wird gelesen.
**Wege:** rawler forken bzw. per `[patch.crates-io]` einen Konstruktor ohne populate ergänzen, einen Upstream-PR stellen oder einen eigenen Metadaten-Leser wie `raw_preview.rs` schreiben.
**Zurückgestellt, weil:** Jeder Weg ist eine Abhängigkeits- bzw. Wartungsentscheidung.
**Aufwand:** mittel (Patch) bis groß (eigener Leser)
**Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### 2. Export in reduzierter Auflösung
**Heute:** Jeder Export durchläuft die Pipeline in voller Auflösung, auch für ein JPEG mit 2048 px.
**Ziel:** Bei starker Verkleinerung wird zuerst auf etwa die doppelte Zielgröße reduziert und erst dann entwickelt.
**Zurückgestellt, weil:** Schärfen, Klarheit und andere lokale Operatoren wirken dann sichtbar anders. Das ist ein Qualitätskompromiss.
**Aufwand:** mittel
**Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### 3. Speichern nach jeder Regler-Geste im Hintergrund
**Heute:** Nach jeder Geste werden Sidecar, XMP (falls eingeschaltet) und Katalog im UI-Thread geschrieben, mit fsync (`commit_and_save`).
**Ziel:** Ein Schreib-Worker übernimmt das, bei schnellen Folgen zusammengefasst.
**Zurückgestellt, weil:** Die Reihenfolge der Schreibvorgänge bei Rückgängig, Bildwechsel und Absturz muss garantiert bleiben.
**Aufwand:** mittel
**Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

---

## Größere Umbauten

### 4. Ordnerinhalt im Hintergrund laden
**Heute:** `reload_entries` liest im UI-Thread den Ordner, alle Sidecars und den Katalog und sortiert dann. Das passiert nach dem Indexieren, Umbenennen und bei Varianten. Bei großen Ordnern ruckelt die Oberfläche.
**Ziel:** Ein Worker lädt, die Liste wird per Event übernommen. Der Hot-Folder fügt neue Aufnahmen schon jetzt nur hinzu.
**Aufwand:** groß
**Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### 5. Dunst entfernen ganz auf der Grafikkarte
**Heute:** Für das Quantil der Lichtfarbe werden Samples von der Grafikkarte gelesen, bei 24 MP rund 41 MB, und auf der CPU sortiert. Die Rücklesungen sind schon gebündelt.
**Ziel:** Ein Histogramm auf der GPU, ohne die Samples zu lesen.
**Aufwand:** mittel
**Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

### 6. Indexieren in Transaktionen
**Heute:** Jede Datei wird einzeln committet. Mit `synchronous=NORMAL` kostet das kaum noch ein fsync.
**Ziel:** Ein Commit alle 50–100 Dateien.
**Zurückgestellt, weil:** Eine Transaktion über das Lesen der RAW-Dateien hinweg hielte die Schreibsperre und würde das Bewerten in der Oberfläche blockieren.
**Aufwand:** klein
**Entscheidung:** [ ] umsetzen  [ ] nicht umsetzen  [ ] Klärung erforderlich
**Input:**

---

## Klein oder bewusst gelassen

| # | Thema | Warum offen |
|---|---|---|
| 7 | Personen-Panel: Gesichter der gewählten Person werden pro Frame geklont, dazu Schlüssel und Zählungen pro Frame | Liegt im µs-Bereich; `person_faces_ui` braucht `&mut self` |
| 8 | `commit_and_save` vergleicht die Einstellungen in jedem Frame | Bewusst so: Regler ändern die Werte direkt, ein Versions-Check verlöre Rückgängig-Schritte |
| 9 | KI-Runtime kopiert Ein- und Ausgaben pro Lauf | Neben der Inferenz vernachlässigbar |
| 10 | Gesichtsanalyse liest pro Bild alle Embeddings (Personen-Mittelwerte) | Index und HashMap sind erledigt; inkrementell pro Job wäre der nächste Schritt |
| 11 | Thumbnails werden nicht verkleinert dekodiert (DCT-Skalierung) | Mit zune-jpeg nicht möglich, jpeg-decoder wäre eine neue Abhängigkeit; die Wahl der kleinsten ausreichenden Vorschau ist umgesetzt |

---

## Bekannt

`pipeline::gpu::tests::rendering_matches_cpu` scheitert unter llvmpipe im Fall „Geometrie“ (max 86), und zwar schon vor der Durchsicht. Weil der Test beim ersten Fehler abbricht, laufen die Fälle danach (Ebenen, Klonen …) nicht mit. Probeweise einzeln geprüft stimmen sie mit max. 1–2 überein.
