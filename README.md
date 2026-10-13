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
| `sk-ai`   | Neuronale Netze (tract/ONNX Runtime): Masken aus Bildinhalt, Tiefenkarte, Entrauschen, Entfernen, Vergrößern, Gesichter |
| `sk-app`  | egui/wgpu-Oberfläche, Hintergrund-Worker |

Pipeline (Arbeitsfarbraum: lineares Rec.2020):

1. Geometrie: Zuschnitt, Geraderichten, Spiegeln, Drehen, Keystone, Objektivkorrektur
   (Verzeichnung, CA, Vignettierung – manuell, aus den Kameradaten oder per Lensfun-Profil)
2. Weißabgleich mit Highlight-Rekonstruktion, Kameramatrix
3. Rauschreduzierung (Luminanz: Guided Filter, Farbe: Chroma-Glättung)
4. Belichtung, lokales HDR, Klarheit/Struktur, Kontrast, Basiskurve
5. Farbe in Oklab: Color Editor (8 Farbbereiche), Color Balance, Sättigung/Dynamik, Schwarzweiß mit Tonung
6. Helligkeit, Levels, Kurven
7. Schärfen, Vignette, Körnung
8. Ausgabefarbraum (sRGB, Adobe RGB, Display P3, ProPhoto) mit eingebettetem ICC-Profil

Die frühen Stufen werden zwischengespeichert, sodass Ton- und Farbregler flüssig bleiben. Beim
Hineinzoomen rendert ein Hintergrund-Worker den sichtbaren Ausschnitt in voller Auflösung.

Die Pipeline läuft auf der Grafikkarte (wgpu-Compute-Shader in `sk-core/src/gpu/shaders`, über
Vulkan, DirectX 12 oder Metal) und fällt ohne geeignete Karte oder bei einem Fehler auf den Prozessor
zurück. Der Prozessor-Pfad bleibt die Referenz: `pipeline::gpu`-Tests vergleichen beide Wege (höchstens
1/255 Abweichung). Exporte großer Bilder werden in Kacheln gerechnet. Heilen/Klonen und Entfernen
werden beim Resampling auf dem Prozessor angewendet, der Softproof (Little CMS) ebenfalls.
Antwortet die Grafikkarte nicht (Zeitlimit 60 s), geht sie verloren oder liefert sie ein
unvollständiges Bild, wird sie bis zum Neustart nicht mehr verwendet und der Prozessor übernimmt;
der Dialog „Grafikkarte …“ nennt den Grund. Puffer werden zwischen Berechnungen wiederverwendet.
Abschalten im Dialog „Grafikkarte …“ oder mit `SILBERKORN_GPU=0`; `SILBERKORN_GPU=software` erlaubt
Software-Adapter (unter WSL zusätzlich `GALLIUM_DRIVER=llvmpipe`) für die App selbst; die
Vergleichstests holen sich ohne Grafikkarte von sich aus einen Software-Adapter.
`cargo run --release -p sk-core --example gpu_info` zeigt die gefundenen Adapter.

Bayer-Sensoren werden mit AHD entwickelt (jedes Pixel wird waagerecht und senkrecht interpoliert,
genommen wird die gleichmäßigere Richtung; keine Reißverschluss-Kanten an feinen Linien),
X-Trans-Sensoren (Fujifilm) mit dem Markesteijn-Verfahren (drei Durchgänge für
volle Auflösung und Export, einer für Vorschauen); Vorschauen fassen wie bei Bayer-Sensoren Blöcke
des Mosaiks zusammen (3×3 statt 2×2) und brauchen daher kein Demosaicing.

Ein Fehler in einem Hintergrund-Thread (etwa bei einer beschädigten RAW-Datei) beendet den Thread
nicht: der betroffene Auftrag schlägt fehl, die Statusleiste meldet ihn, Details stehen in
`~/.local/share/silberkorn/panic.log`.

`SK_TRACE=1` gibt die Zeiten der einzelnen Stufen aus; `cargo run --release -p sk-core --example bench -- <raw>`
misst die Pipeline auf Prozessor und Grafikkarte.

### Ebenen & Masken

Lokale Anpassungen (Tab „Ebenen“) wirken als Deltas auf die Hintergrund-Einstellungen und werden pro
Pixel mit Maske × Deckkraft gemischt; überlappende Ebenen addieren sich. Masken bestehen aus
Pinselstrichen, linearen und radialen Verläufen, „Füllen“ und „Invertieren“.

**Pinsel** (Pinsel, Radierer, Entfernen; Reparieren/Klonen ohne Fluss und Auto-Maske) haben jeweils
eigene, gespeicherte Einstellungen: **Größe**, **Härte** (weiche bis harte Kante), **Deckkraft** (das
Maximum eines Strichs) und **Fluss** (wie viel jeder Pinselabdruck beiträgt; innerhalb eines Strichs
baut sich die Deckung auf). **Automatisch maskieren** beschränkt den Strich auf Bereiche, die der
Stelle unter der Pinselmitte in Farbe und Helligkeit ähneln, sodass Kanten erhalten bleiben. Striche
werden dicht aufgezeichnet und als Catmull-Rom-Kurve mit Abdrücken im Abstand von 15 % des
Radius gerastert; Masken haben 3072 Pixel an der langen Kante. Beim Malen wird nur der aktuelle
Strich neu gerastert. Striche aus älteren Sidecars sehen aus wie zuvor. Sie werden in Quellkoordinaten gespeichert und bleiben deshalb nach Zuschnitt,
Drehen oder Objektivkorrektur am Bildinhalt.

„Reparieren“ und „Klonen“ werden wie ein Pinsel über die Stelle gemalt (ein Klick ergibt einen runden
Punkt). „Reparieren“ übernimmt Struktur von einer automatisch gewählten Quelle und gleicht Farbe/Helligkeit an
die Umgebung an; „Klonen“ kopiert 1:1. Mit aktivem Werkzeug zeigt der Viewer alle Bereiche mit Pins: der
weiße verschiebt den Bereich, der gelbe (beim ausgewählten) die Quelle. Im Abschnitt „Reparieren & Klonen“
stehen alle Bereiche in einer Liste; dort lassen sich Art, Deckkraft und Weichheit ändern, eine neue Quelle
suchen oder Bereiche löschen.

**Staub und Flecken finden** (dort, oder über die Befehlssuche) macht Sensorstaub und Hautunreinheiten zu
Reparieren-Stellen: gesucht werden kleine, runde Stellen, die rundum dunkler als ihre Umgebung sind, auf ruhigen
Flächen (Himmel, Wände, Haut). Auf dem Logarithmus der Helligkeit misst eine Differenz zweier Weichzeichnungen
in vier Größen, wie viel dunkler eine Stelle als ihr Ring ist; das muss die übliche Antwort der Umgebung
(Struktur, Rauschen) deutlich übertreffen, und der Ring muss in allen zwölf Richtungen heller sein, sonst ist es
eine Kante oder Linie. Die **Empfindlichkeit** senkt beide Schwellen. Die Suche läuft auf der Vorschau im
Hintergrund (etwa 0,2 s), bereits reparierte Stellen werden übersprungen, und danach zeigt das
Reparieren-Werkzeug alle Stellen zum Prüfen. Kommandozeile: `cargo run --release -p sk-core --example spots -- <raw> <bild.png> [empfindlichkeit]`

### Auswahl aus Bildinhalt

- **Motiv auswählen:** neuronales Netz U²-Net-p (Apache-2.0, 4,6 MB). Das Modell wird im Tab „Ebenen“
  auf Wunsch nach `~/.local/share/silberkorn/models/` geladen (oder `SILBERKORN_MODEL=<pfad>`).
- **Himmel auswählen:** helle, strukturarme, blaue oder neutrale Bereiche, die mit dem oberen Rand verbunden sind.
- **Magic Brush (J):** über den Bereich malen; gewählt wird, was zusammenhängt und einer der überstrichenen Farben ähnelt (Größe und Toleranz einstellbar).
- **Objekt (Umschalt+J):** auf ein Objekt klicken, es wird ausgewählt; weitere Klicks verfeinern, Alt+Klick
  nimmt Teile aus, „Nächstes Objekt“ wählt ein weiteres dazu. MobileSAM (Apache-2.0; Bild-Encoder und
  Masken-Decoder von Segment Anything als ONNX, 45 MB, auf Wunsch geladen). Der Encoder analysiert das Bild
  einmal, sobald das Werkzeug gewählt wird (etwa 4 s auf dem Prozessor); jeder Klick braucht dann nur noch
  den Decoder (etwa 0,2 s). tract rechnet nur dessen 256er-Maske, Silberkorn skaliert sie selbst hoch und
  verfeinert die Kanten am Bild. Jeder Klick ist ein Rückgängig-Schritt.
- **Luma Range** und **Farbbereich** (mit Pipette) verfeinern jede Maske.
- **Style Brushes:** Pinsel-Presets (Aufhellen, Abdunkeln, Wärmer, Haut glätten, Dunst entfernen, …).

### KI-Werkzeuge: Entrauschen und Entfernen

Die neuronalen Netze werden nicht mitgeliefert, sondern beim ersten Gebrauch auf Wunsch nach
`~/.local/share/silberkorn/models/` geladen (`SILBERKORN_MODELS=<ordner>` überschreibt den Ort).
Jede Datei wird gegen Größe und SHA-256 geprüft (Hugging-Face-Links auf einen festen Stand
festgelegt); ein abgebrochener Download wird beim nächsten Versuch fortgesetzt.

**KI-Beschleunigung** (Bearbeiten → „Grafikkarte …“, Abschnitt „KI“, Windows): Die Netze rechnen dann über
ONNX Runtime mit DirectML auf der Grafikkarte (jede DirectX-12-Karte). ONNX Runtime wird nicht
gelinkt, sondern zur Laufzeit geladen: Auf Wunsch lädt Silberkorn `onnxruntime.dll` (1.24.4) und
`DirectML.dll` (1.15.4) aus Microsofts NuGet-Paketen nach `%LOCALAPPDATA%\silberkorn\gpu`, und zwar
per Teil-Download nur diese beiden Dateien (etwa 28 MB statt über 200 MB). Die EXE bleibt eine
einzelne Datei, der Cross-Build aus WSL funktioniert weiter. Steht DirectML nicht zur Verfügung,
rechnet ONNX Runtime auf dem Prozessor (etwa doppelt so schnell wie ohne); ohne ONNX Runtime oder
bei Fehlern übernimmt tract. Zum Testen auf anderen Systemen: `SILBERKORN_ORT=<libonnxruntime.so>`.

Alles, was länger dauert (KI, Zusammenfügen, Verarbeiten, Indexieren, Downloads), zeigt die
Statusleiste mit Fortschritt an; Verarbeiten, Zusammenfügen und Entrauschen lassen sich dort mit ✕
abbrechen, und beim Beenden fragt Silberkorn nach, solange sie laufen; worauf man im Viewer wartet (Maske, Entfernen, Angleichen),
erscheint zusätzlich oben im Bild.

- **KI-Entrauschen** (Tab „Details“, für mehrere Bilder im Bild-Menü): RawNIND (UtNet2, Benoit
  Brummer, UCLouvain) in der Fassung von darktable (darktable-ai 5.6.0). Die Gewichte stehen unter
  **GPL-3.0**; sie werden nicht mitgeliefert, sondern erst geladen, nachdem man den Lizenzhinweis
  bestätigt hat (etwa 57 MB, per Teil-Download nur die beiden ONNX-Dateien aus dem Archiv).
  Bei Bayer-Sensoren bekommt das Netz das Mosaik selbst (R, G, G, B, schwarzwertbereinigt, nicht
  weißabgeglichen) und liefert sauberes Kamera-RGB in voller Auflösung, ersetzt also auch das
  Demosaicing; X-Trans und andere Sensoren laufen demosaict durch ein zweites Netz für lineares
  Rec.2020. Kacheln zu 512 mit 64 Überlappung, die Helligkeit jeder Kachel wird an die Eingabe
  angeglichen; übersteuerte Lichter bleiben unverändert.
  Das Ergebnis wird je Bild einmal im Hintergrund berechnet (das Bild im Viewer zuerst; 24 MP etwa
  30 s auf 12 Kernen, X-Trans deutlich länger) und als komprimierte Halbfloat-Datei in
  `~/.cache/silberkorn/denoise/` gehalten (höchstens 20 GB, die ältesten fallen heraus;
  `SILBERKORN_DENOISE_CACHE` überschreibt den Ort). **Stärke** mischt es vor allen anderen Stufen
  in die RAW-Daten: nicht-destruktiv, rückgängig zu machen, gilt für Vorschau, Export und
  Thumbnails. Fehlt es beim Export, wird es dort berechnet – nie wird stillschweigend ohne
  exportiert. Kommandozeile: `cargo run --release -p sk-ai --example denoise -- <raw> <ordner> [stärke] [--linear]`
- **Entfernen** (X, Tab „Ebenen & Retusche“): Über Störendes malen; beim Loslassen füllt MI-GAN
  (Picsart, MIT, 30 MB) die Stelle passend zur Umgebung. Gefüllt wird in voller Auflösung, bei
  größeren Markierungen in mehreren 512er-Fenstern nacheinander; erst sehr große Flächen werden
  verkleinert berechnet. Das Ergebnis ist ein kleiner Flicken mit weicher Kante im Sidecar (Feld
  `removals`): Er wird vor Reparieren/Klonen eingesetzt, bleibt am Bildinhalt wie die Masken und
  ist ein Rückgängig-Schritt. „Letzten zurücknehmen“ und „Alle“ stellen das Original wieder her.
- **Objektivunschärfe** (Tab „Details“): zeichnet Vorder- und
  Hintergrund weich wie eine offene Blende. Wie nah jede Stelle ist, schätzt Depth Anything V2
  Small (Lihe Yang et al.; das kleine Modell steht unter Apache-2.0, die größeren nicht; 99 MB,
  ONNX von onnx-community) einmal je Bild auf der Vorschau (etwa 7 s auf dem Prozessor). Die
  Tiefenkarte (relative Nähe 0 fern … 1 nah, 1536 px, Kanten am Bild verfeinert) liegt in
  `~/.cache/silberkorn/depth/` (`SILBERKORN_DEPTH_CACHE` überschreibt den Ort, höchstens 2 GB).
  **Fokus** per Pipette (Klick auf die Stelle, die scharf bleiben soll) oder Regler; beim
  Einschalten liegt er auf dem nächsten Zehntel des Bildes, meist dem Motiv. **Schärfebereich**:
  wie weit davor und dahinter es scharf bleibt; **Lichter**: helle Stellen werden in der
  Unschärfe zu leuchtenden Scheiben. Die Unschärfe wächst mit dem Tiefenabstand zum Fokus (die
  Netzausgabe ist inverse Tiefe, zu der der Zerstreuungskreis proportional ist), bei Stärke 100
  bis auf 2,5 % der langen Kante. Sie wird wie das Entrauschen vor allen anderen Stufen auf die
  linearen Daten angewendet, gilt also für Vorschau, Thumbnails und Export: Der Hintergrund
  sammelt nur von Nachbarn, die mindestens so unscharf sind (kein Hof um das scharfe Motiv);
  unscharfer Vordergrund wird in Ebenen nach Radius über das Dahinterliegende gelegt und greift
  über seine Kante hinaus. Übergangswerte an harten Tiefenkanten springen auf die nahe oder
  ferne Seite, sonst bliebe dort eine scharfe Naht. Große Radien werden auf verkleinerten Kopien
  gerechnet: 24 MP etwa 1,2 s. Fehlt die Tiefenkarte beim Export, wird sie dort berechnet.
  Kommandozeile: `cargo run --release -p sk-ai --example depth -- <raw> <ordner> [stärke] [fokus] [lichter]`
- **Tiefe auswählen** (Tab „Ebenen“, Auswahl aus Bildinhalt): eine Maske aus derselben
  Tiefenkarte, anfangs das nächste Fünftel des Bildes; **Nah**, **Fern** und **Weichheit** stellen
  den Bereich danach ein (gespeichert mit der Maske, rückgängig zu machen).

### Gesichter und Sichtung

Tab **„Bibliothek“ → Gesichter & Sichtung → „Ansicht analysieren“** untersucht die Bilder der
Ansicht im Hintergrund (auf dem Kamera-Vorschaubild, daher schnell) und speichert das Ergebnis
im Katalog bzw. in der Session:

- **Gesichter** findet YuNet (OpenCV Zoo, MIT); Profile werden erkannt, aber nicht weiter bewertet.
- **Geschlossene Augen:** open-closed-eye-0001 (Intel, Apache-2.0) bewertet beide Augen; ein
  Gesicht gilt als „Augen zu“, wenn beide geschlossen sind. Im Browser erscheint dann ein
  Augen-Symbol oben rechts.
- **Schärfe:** Verhältnis der feinsten zu gröberen Details auf den Gesichtern (ohne Gesichter auf
  der schärfsten Bildregion), unabhängig von Belichtung und Kontrast. Unter 0,22 gilt ein Bild als
  **unscharf** (Tropfen-Symbol).
- **Filter:** „Augen geschlossen“, „Unscharf“, „Mit Gesichtern“, „Nicht analysiert“; sie lassen sich
  auch als Smart-Album speichern.
- **Personen:** SFace (OpenCV Zoo, Apache-2.0) berechnet für jedes Gesicht einen Merkmalsvektor;
  ähnliche Gesichter werden derselben Person zugeordnet. Die Liste zeigt benannte Personen und
  solche auf mindestens zwei Bildern; ein Klick zeigt ihre Bilder, ein Doppelklick benennt sie.
  Der Name wird als Keyword der Bilder gesetzt (auch im XMP, wenn aktiviert); eine Person, die
  bereits so heißt, wird zusammengeführt.
- **Stapel → „Schärfstes Bild nach oben“** macht in jedem ausgewählten Stapel das schärfste Bild
  zum obersten.

Die drei Modelle (zusammen etwa 40 MB) werden beim ersten Analysieren geladen.
Kommandozeile: `cargo run --release -p sk-ai --example faces -- <bild>...`

### Vergleichen und Angleichen

- **Vergleichsansicht:** Sind mehrere Bilder ausgewählt (Strg/Umschalt+Klick), zeigt der Viewer sie
  nebeneinander (bis zu 12). Mausrad und Ziehen zoomen bzw. verschieben alle Bilder gemeinsam,
  Doppelklick wechselt zwischen Einpassen und Vergrößerung. Beim Hineinzoomen wird der sichtbare
  Ausschnitt jedes Bildes aus der vollen Auflösung nachgeladen. Ein Klick macht ein Bild zum
  bearbeiteten (blau umrandet), ohne die Auswahl zu ändern. Abschalten: Ansicht → „Auswahl
  nebeneinander vergleichen“.
- **Referenzbild** (Umschalt+Enter oder Rechtsklick in der Vergleichsansicht): Es bleibt links neben
  dem aktuellen Bild stehen, auch beim Durchblättern.
- **An Referenz angleichen** (Menü „Bild“): Belichtung und/oder Weißabgleich der Auswahl werden an das
  Referenzbild angepasst. Die Belichtung bringt die mittlere Helligkeit auf die der Referenz, der
  Weißabgleich übernimmt deren Abweichung von einer neutralen Szene. Eine bewusst warme Referenz
  macht die anderen Bilder also gleich warm, auch wenn sie bei anderem Licht entstanden sind.
- **Fokusmaske** (K oder Ansicht → „Fokusmaske“): Scharfe Bereiche werden grün markiert, auch in der
  Vergleichsansicht, um schnell das schärfste Bild einer Serie zu finden. Gemessen wird die Energie der
  feinsten Details, die bei Unschärfe zuerst verschwinden; hineingezoomt in voller Auflösung. Die Empfindlichkeit lässt sich im Menü
  „Ansicht“ einstellen.
- **Hauttöne** (Tab „Farbe“): Mit der Pipette einen Hautton aufnehmen. „Gleichmäßigkeit“ zieht
  Farbton, Sättigung und Helligkeit der ähnlichen Farben zu diesem Ton hin (Flecken, Rötungen),
  „Hautton verändern“ verschiebt den ganzen Bereich.

Weitere Funktionen: **Dunst entfernen** (Dark-Channel-Prior, global und pro Ebene), **Speed Edit**
(Taste halten + Mausrad), **Vorher/Nachher** geteilt mit verschiebbarem Trenner (U) oder nebeneinander
mit gemeinsamem Zoom und Ausschnitt (Umschalt+U), und **mehrere Ausgaberezepte**, die in einem Durchgang
verarbeitet werden (das RAW wird pro Bild nur einmal entwickelt).

### HDR und Panorama zusammenfügen

**Bild → Zusammenfügen** (mindestens zwei Bilder ausgewählt) erzeugt ein neues
**lineares DNG** neben den Originalen (`…-HDR.dng`, `…-Pano.dng`): demosaicte Kameradaten als
16-Bit-Gleitkomma, verlustfrei komprimiert. Weißabgleich, Farbe und Tonwerte bleiben daher voll
bearbeitbar, und auch Lightroom und Capture One öffnen die Datei. Beides läuft im Hintergrund
(Fortschritt in der Statusleiste); das Ergebnis wird danach ausgewählt.

- **HDR:** Die Belichtungsunterschiede werden aus den Bilddaten gemessen (EXIF als Rückfall),
  aus der Hand fotografierte Reihen per Median-Threshold-Bitmaps ausgerichtet. Pro Pixel zählt
  jede Belichtung nach Belichtungsgüte und -zeit, übersteuerte Werte gar nicht. **Geister
  entfernen** (Aus/Niedrig/Mittel/Hoch) nimmt bewegte Motive aus der mittleren Belichtung. Lichter
  über dem Weißpunkt bleiben erhalten und lassen sich mit „Lichter“ und „Weiß“ zurückholen.
- **Panorama:** Harris-Ecken mit normierten Bildausschnitten, Zuordnung zwischen allen Paaren,
  RANSAC-Homographien, Brennweite aus den Homographien, Rotationen über einen Spannbaum und
  Bündelausgleich (Levenberg-Marquardt), Horizontausgleich. Projektion **sphärisch**,
  **zylindrisch** oder **perspektivisch**; Helligkeitsausgleich, weiche Übergänge im linearen Licht,
  auf Wunsch automatischer Beschnitt auf das größte Rechteck ohne leere Ränder. Die
  Objektivkorrektur der Einzelbilder (aus ihrem Sidecar) wird vorher angewendet.

Kommandozeile: `cargo run --release -p sk-core --example merge -- hdr|pano <raw>...`

### Automatisch geraderichten

Silberkorn sucht gerade Kanten im Bild (Linienerkennung nach dem
LSD-Prinzip) und bestimmt Drehung und Keystone so, dass sie waagerecht bzw. senkrecht stehen.
Bewertet wird jede Lösung über die echte Geometrie-Kette, einschließlich Objektivkorrektur.

- **Auto** (Umschalt+A, Bild → „Automatisch geraderichten“): Drehung und stürzende Linien.
- **Nur Drehung:** Horizont und Kanten waagerecht, Keystone bleibt unverändert.
- **Voll:** zusätzlich horizontaler Keystone, etwa für eine schräg fotografierte Fassade.

Waagerechte zählen bis 8° Abweichung, Senkrechte bis 15°; deutlich schräge Kanten wie Dächer
oder Wege bleiben unberücksichtigt. Keystone wird nur gesetzt, wenn mehrere passende Kanten über
das Bild verteilt sind. Die Buttons stehen im Tab „Objektiv“ unter „Keystone“; das Ergebnis ist ein
Rückgängig-Schritt und lässt sich mit den Reglern nachjustieren.

### Viewer auf dem Zweitmonitor

**Ansicht → „Viewer in eigenem Fenster“** (Umschalt+F11) öffnet den Viewer als eigenes Fenster,
etwa für einen zweiten Bildschirm. Das Hauptfenster zeigt dann
das Raster zum Sichten und Auswählen, die Werkzeuge bleiben links. Im Viewer-Fenster
funktionieren Zoom, Vergleichsansicht, Werkzeuge und alle Tastenkürzel; **F11** schaltet dort auf
Vollbild, Esc zurück. Position, Größe und Vollbild werden gemerkt, beim nächsten Start öffnet sich
das Fenster wieder.

### Lupe

Maustaste auf dem eingepassten Bild gedrückt halten: Ein Ausschnitt unter dem Cursor erscheint in
100 % (ein Bildpixel pro Bildschirmpixel). Gezeigt werden die
Kacheln in voller Auflösung, solange sie noch berechnet werden die vergrößerte Vorschau.

### Verlauf und Schnappschüsse

Der Tab **„Verlauf“** listet alle Bearbeitungsschritte des Bildes, benannt nach dem, was sich
geändert hat („Belichtung 0,8“, „Keystone, Geraderichten“). Ein Klick springt zu diesem Stand,
spätere Schritte bleiben erreichbar, bis eine neue Änderung sie verwirft. Der Verlauf gilt, solange
das Bild geöffnet ist.

**Schnappschüsse** sichern einen Stand unter einem Namen, dauerhaft im Sidecar (je Bild bzw.
Variante). Klick wendet ihn an (ein Rückgängig-Schritt), Doppelklick benennt um.

### Umbenennen

**Bild → „Umbenennen …“** (Strg+Umschalt+R) benennt die ausgewählten Dateien nach einer Vorlage um,
mit Vorschau. Platzhalter: `{Name}`, `{Zähler}` (`{Zähler:3}` dreistellig, Startwert wählbar),
`{Datum}`, `{Zeit}`, `{Jahr}`, `{Monat}`, `{Tag}`, `{Kamera}`, `{Objektiv}`, `{Session}`,
`{Variante}`. Sidecar, Varianten, Schnappschüsse, Katalogeinträge und Vorschaubilder wandern mit;
ist ein Name vergeben, wird „-2“ usw. angehängt. Dieselben Platzhalter gibt es im Ausgaberezept
als **Dateiname** der verarbeiteten Bilder (der Zähler zählt dort die Bilder eines Durchgangs).

### Hilfslinien und Overlay

- **Hilfslinien** (O wechselt, oder Ansicht → „Hilfslinien“): Drittel, goldener Schnitt, Raster mit
  annähernd quadratischen Feldern, Diagonalen.
- **Overlay** (Umschalt+O, Ansicht → „Overlay wählen …“): ein eigenes Layout-PNG, z. B. Titelseite
  oder Beschnittzugabe, wird eingepasst über das Bild gelegt; Deckkraft im Menü einstellbar.

### Wasserzeichen und Metadaten (Ausgaberezept)

- **Wasserzeichen:** Text (weiß oder schwarz) oder Logo (PNG mit Transparenz), in neun Positionen,
  Größe und Randabstand in % der kurzen Bildkante, Deckkraft einstellbar. Die Proof-Ansicht (P)
  zeigt es so, wie es geschrieben wird.
- **Formate:** JPEG, TIFF (8/16 Bit), PNG, WebP (verlustbehaftet, libwebp) und AVIF (10 Bit,
  4:4:4, rav1e). AVIF kennzeichnet den Farbraum ohne ICC-Profil und gibt es deshalb nur in sRGB
  und Display P3; ein 24-MP-Bild braucht rund 12 s. Die Proof-Ansicht zeigt bei WebP die
  komprimierten Pixel, bei AVIF die Pixel vor der Kompression (mit der echten Dateigröße).
- **KI-Vergrößern** (wie „Super Resolution“): verdoppelt oder vervierfacht die Kantenlänge des
  fertig entwickelten Bildes mit einem neuronalen Netz, bevor es gespeichert wird; eine lange
  Kante verkleinert danach wieder (so lassen sich z. B. genau 8000 px erreichen). Das Netz ist
  RealPLKSR (MIT, ~30 MB je Faktor, in der Fassung von darktable-ai); es erfindet keine Details
  und wird erst bei Bedarf im Rezept geladen. Höchstens 250 MP Ergebnis. Auf dem Prozessor
  langsam (eine 512er-Kachel etwa 1,5 min, ein 24-MP-Bild eine halbe Stunde und mehr), mit
  Grafikkarte (Windows) deutlich schneller. Die Proof-Ansicht zeigt das Bild ohne Vergrößerung.
- **Metadaten:** JPEG, PNG, WebP und AVIF erhalten EXIF mit Kamera, Objektiv, Belichtung und
  Aufnahmedatum (abschaltbar) sowie Fotograf und Copyright. GPS-Daten werden nie übernommen.
  TIFF-Dateien erhalten keine Metadaten.

### Softproof

**Umschalt+P** (Ansicht → „Softproof“) zeigt das Bild so, wie es in einem anderen Farbraum oder
auf einem Drucker herauskommt. Eingestellt wird im Tab **„Ausgabe“ → Softproof**:

- **Profil:** sRGB, Adobe RGB, Display P3 oder ein ICC-Profil (Drucker/Papier). Gefunden werden
  Profile in `~/.config/silberkorn/profiles`, `/usr/share/color/icc`, `~/.local/share/icc`
  bzw. unter Windows im Farbprofil-Ordner des Systems; weitere über „ICC-Profil laden …“.
- **Priorität** (wahrnehmungsorientiert, relativ/absolut farbmetrisch, Sättigung),
  **Tiefenkompensierung** und **Papier und Druckfarbe simulieren** (Papierton und echtes Schwarz).
- **Farbumfang-Warnung:** Farben, die das Profil nicht wiedergeben kann, erscheinen magenta.

Die Umrechnung übernimmt Little CMS (statisch eingebunden). Der Softproof betrifft nur die
Anzeige, nicht den Export.

### Objektivkorrektur aus den Kameradaten

Viele Kameras speichern die Korrektur für das angesetzte Objektiv im RAW. Silberkorn liest sie und
wendet sie bei neuen Bildern automatisch an (Tab „Objektiv“, „Korrektur aus den Kameradaten“;
Einstellungen von vor dieser Funktion behalten ihr Aussehen):

- **Sony ARW:** Verzeichnung, Farbsäume und Vignettierung (Tags 0x7032/0x7035/0x7037).
- **Fujifilm RAF:** Verzeichnung, Farbsäume und Vignettierung (Tags 0xF00B/0xF00F/0xF010). Der
  1,25×-Ausschnitt (Sportsucher, elektronischer Verschluss) wird noch nicht berücksichtigt.
- **Panasonic RW2:** Verzeichnung (Tag 0x0119); Vignettierung korrigiert die Kamera schon im RAW.
- **DNG:** WarpRectilinear und FixVignetteRadial (OpcodeList3) sowie die GainMaps der OpcodeList2
  (Helligkeitsausgleich von Smartphones). Die GainMaps gehören zu den Rohdaten und werden immer
  angewendet. Tangentiale Verzeichnung wird ignoriert.

Die Formate der Hersteller sind nicht dokumentiert; Silberkorn folgt dem Reverse Engineering von
darktable und ExifTool (eigene Umsetzung). Alle Modelle werden in eine gemeinsame Form gebracht
(gerade Polynome im Radius je Farbkanal) und laufen über dieselbe Geometrie wie lensfun-Profile;
Kameradaten und lensfun-Profil schließen sich gegenseitig aus.

### Objektivprofile

Lensfun-XML-Dateien werden aus `~/.local/share/silberkorn/lensfun`, `~/.local/share/lensfun` und
`/usr/share/lensfun` gelesen. Im Tab „Objektiv“ lässt sich die Datenbank auch herunterladen
(Quelle: lensfun auf GitHub, Lizenz CC BY-SA 3.0; sie wird nicht mit Silberkorn ausgeliefert).

Einstellungen werden nicht-destruktiv als `<Bild>.sk.json` neben dem RAW gespeichert.
Sidecars, XMP-Dateien, Stile und Einstellungen werden atomar geschrieben (erst vollständig in eine
eigene temporäre Datei, dann ersetzt), sodass ein Absturz oder eine volle Platte nie eine halbe
Datei hinterlässt. Ein Sidecar oder Katalog, den eine neuere Silberkorn-Version geschrieben hat,
wird gelesen, aber nicht überschrieben.

### Prozessversionen

Jede Bearbeitung merkt sich die Version des Renderers
(`process` in den Anpassungen). Bearbeitungen aus älteren Versionen sehen deshalb unverändert aus,
neue und zurückgesetzte Bilder nutzen die aktuelle. **Bild → „Auf aktuelle Prozessversion heben“**
stellt die Auswahl um; Stile und eingefügte Anpassungen übernehmen die Version des Zielbilds.

- **1:** ursprünglicher Renderer.
- **2:** Farben außerhalb des Ausgabefarbraums werden zu ihrer eigenen Helligkeit hin entsättigt,
  statt jeden Kanal abzuschneiden. Der Farbton bleibt so erhalten (gesättigtes Blau kippt nicht
  mehr ins Violette, Neon und LEDs nicht ins Gelbe).
- **3:** Die Tonkurve wird auf den hellsten und den dunkelsten Farbkanal angewendet, der mittlere
  folgt (wie die Referenz-Tonkurve der DNG-Spezifikation). Mehr Kontrast verschiebt so keine
  Farbtöne mehr: Haut wird nicht orange, Himmel nicht cyan. Lichter/Schatten arbeiten auf einem
  Median der Helligkeit (0,15 % der langen Kante), sodass Äste, Linien und feine Texturen ihren
  Kontrast behalten. Die Farbmatrix folgt dem Licht: Hat die Kamera auch eine Matrix für
  Glühlampenlicht, wird zwischen Tageslicht und Glühlampe nach der Farbtemperatur interpoliert.

### Varianten

Ein RAW kann mehrere Bearbeitungen
haben, z. B. Farbe und Schwarzweiß. **F2** legt eine neue Variante mit Standardeinstellungen an,
**F3** klont die aktuelle; beides auch im Menü „Bild“ und per Rechtsklick auf ein Vorschaubild.
Varianten erscheinen im Browser als eigene Bilder („IMG_0001.ARW (2)“) mit eigener Bewertung,
eigenem Label und eigenen Keywords, lassen sich vergleichen, in Alben legen und verarbeiten
(Ausgabe als `IMG_0001-v2.jpg`).

Gespeichert werden sie im Sidecar der RAW-Datei (Feld `variants`). Beim Verschieben oder Löschen
der Datei wandern sie also mit. Intern heißt eine Variante wie die Datei mit angehängtem `#2`, `#3` …
Die erste Variante gehört zur Datei selbst. Weitere lassen sich über „Variante löschen“ (mit Rückfrage)
wieder entfernen; Strg+Entf auf ausgewählten weiteren Varianten tut dasselbe, statt die ganze Datei in
den Papierkorb zu legen. Ein beschädigtes Sidecar wird vor dem Überschreiben als `.sk.json.bak`
gesichert.

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
  Ebenso die bildschirmgroßen **Vorschauen** des Viewers (`Silberkorn/Cache/Previews` bzw.
  `~/.cache/silberkorn/previews`): Ein schon einmal angesehenes Bild erscheint beim Durchblättern
  sofort, die Nachbarbilder werden im Hintergrund vorbereitet. Fehlende Vorschauen eines Ordners
  oder einer Session entstehen im Hintergrund, sobald die App eine Weile nichts zu tun hat
  (Fortschritt rechts in der Statusleiste); die Thumbnails zeigen dann ebenfalls die eigene
  Entwicklung statt des Kamera-JPEGs.

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
- **Eingebettete Vorschauen** werden direkt gelesen: nur die TIFF-Struktur und das JPEG, nicht die
  ganze RAW-Datei (ARW, NEF, CR2, DNG, ORF, RW2, PEF, RAF). Über Netzlaufwerke oder USB ein
  Vielfaches schneller; andere Formate (z. B. CR3) wie bisher über die volle Datei.
- **Thumbnail-Cache** unter `~/.cache/silberkorn/thumbs` (Vorschauen in `previews`). Zusammen
  höchstens 4 GB (`SILBERKORN_CACHE_MB` ändert das); beim Start werden die am längsten nicht
  genutzten entfernt. Session-Caches sind davon ausgenommen. Sichtbare Vorschaubilder werden vor
  denen berechnet, an denen man schon vorbeigescrollt ist.

Sidecars bleiben die maßgebliche Quelle; der Katalog ist ein Index und kann jederzeit neu aufgebaut werden.

### Stapel

Zusammengehörige Bilder (Serie, Belichtungsreihe, Varianten eines Motivs) zeigt
der Browser als ein Vorschaubild, das oberste Bild des Stapels. Die Zahl oben links nennt die
Bilder im Stapel; ein Klick darauf (oder **Umschalt+G**) klappt ihn auf und wieder zu.

- **Strg+G** bildet aus der Auswahl einen Stapel, das Bild im Viewer wird das oberste.
  **Strg+Umschalt+G** löst die Stapel der Auswahl auf. Im Menü **Bild → Stapel** (auch per
  Rechtsklick) außerdem: aus dem Stapel entfernen, als oberstes Bild festlegen, alle auf-/zuklappen.
- **Automatisch nach Aufnahmezeit …** stapelt die Auswahl (oder alle Bilder der Ansicht): Eine
  Pause von mehr als der eingestellten Zeit (Standard 2 s) beginnt einen neuen Stapel. Bilder, die
  schon gestapelt sind, bleiben in ihrem Stapel.
- Die Zugehörigkeit steht im Sidecar jeder RAW-Datei (Feld `stack`); Varianten gehören zum Stapel
  ihrer Datei. Bewertung, Verschieben usw. betreffen nur die ausgewählten Bilder.

### XMP (Lightroom, Bridge, Capture One)

Bewertung, Farb-Label und Keywords werden mit XMP-Sidecars ausgetauscht (`IMG_0001.xmp`, auch
darktables `IMG_0001.ARW.xmp`).

- **Lesen:** Ändert ein anderes Programm die XMP-Datei, übernimmt Silberkorn deren Werte beim
  nächsten Laden. Gilt die eigene Änderung als neuer, bleibt sie. **Bild → XMP → „Metadaten aus
  XMP lesen“** übernimmt die XMP-Werte der Auswahl in jedem Fall.
- **Schreiben:** **„Metadaten in XMP schreiben“** für die Auswahl, oder automatisch bei jeder
  Änderung („Änderungen automatisch in XMP schreiben“, standardmäßig aus). Nur diese drei
  Eigenschaften werden ersetzt; Entwicklungseinstellungen anderer Programme und alles Übrige
  in der Datei bleiben erhalten. Label-Namen werden englisch geschrieben („Red“), deutsche
  Lightroom-Namen („Rot“) werden gelesen.
- XMP-Dateien wandern beim Verschieben, Umbenennen und Importieren mit. Varianten haben eigene
  Werte, die nicht ins XMP gehen.

Daten aus der Zeit als „Capture Room“ werden übernommen: Ordner `capture-room` in `~/.config`,
`~/.local/share` und `~/.cache` werden beim ersten Start nach `silberkorn` verschoben, und alte
`<Bild>.crs.json`-Sidecars werden gelesen und beim nächsten Speichern durch `.sk.json` ersetzt.

## Tastenkürzel

| Taste | Funktion |
|---|---|
| Strg+K | Befehlspalette: jede Funktion, jedes Werkzeug und jeden Tool-Abschnitt per Suche finden (auch über das Suchfeld in der Titelleiste) |
| Strg+1 … Strg+0 | Werkzeug-Tabs in der Reihenfolge der Tab-Leiste |
| Strg+Mausrad | Vorschaugröße im Raster (auch Regler in der Statusleiste) |
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
| Umschalt+P | Softproof |
| W | Weißabgleich-Pipette |
| Z / Doppelklick | Einpassen ↔ 100 % · Mausrad: zoomen · Ziehen: verschieben |
| C / Enter / Esc | Zuschnitt-Werkzeug / übernehmen / beenden |
| B / N / L / R | Pinsel / Radierer / linearer / radialer Verlauf |
| H / S | Reparieren / Klonen · Entf: Punkt löschen |
| X | Entfernen (KI) |
| M | Maske anzeigen · [ / ]: Pinselgröße · mittlere Maustaste: verschieben |
| J | Magic Brush |
| U / Umschalt+U | Vorher/Nachher geteilt / nebeneinander |
| O / Umschalt+O | Hilfslinien wechseln / eigenes Overlay |
| Umschalt+A | automatisch geraderichten |
| Strg+Umschalt+R | Auswahl umbenennen |
| Umschalt+F11 | Viewer in eigenem Fenster (Zweitmonitor) |
| Maustaste halten | Lupe (100 %) im eingepassten Bild |
| Umschalt+Enter | Referenzbild festlegen / entfernen |
| K | Fokusmaske |
| F2 / F3 | neue Variante / Variante klonen |
| Strg+G / Strg+Umschalt+G | Stapel bilden / auflösen |
| Umschalt+G | Stapel auf-/zuklappen |
| Q / A / D / F / V / T + Mausrad | Speed Edit: Belichtung / Kontrast / Sättigung / Lichter / Tiefen / Temperatur (auf der aktiven Ebene, sonst global) |

Doppelklick auf einen Reglernamen setzt den Regler zurück.

## Tests

```bash
cargo t        # schnell: alles außer den langsamen Tests (etwa 1–2 Minuten)
cargo t-slow   # nur die langsamen (volle Auflösung, Export, Vergleichsansicht …)
cargo t-all    # alles
```

Die Aliase stehen in `.cargo/config.toml`. Langsame Tests tragen `#[ignore = "langsam: …"]`.
Die App-Tests nutzen die Beispiel-RAWs in `samples/` und werden ohne sie übersprungen. Sie arbeiten
mit verkleinerten Kopien (2400 × 1600, lineare DNGs mit denselben Aufnahmedaten), die beim ersten
Lauf in `target/test-samples/` entstehen; nur Tests, die 24 Megapixel brauchen, nehmen die
Originale. Nur die
Vergleichstests Grafikkarte/Prozessor (`pipeline::gpu`) rechnen auf der Grafikkarte, ohne eine
echte über einen Software-Adapter; alle anderen Tests rechnen auf dem Prozessor, der viel schneller
ist als ein Software-Adapter. `sk-core` wird auch in Debug-Builds optimiert übersetzt (`Cargo.toml`),
weil die Tests echte RAWs durch die Pipeline schicken. Der Test gegen den echten NuGet-Server läuft
nur mit `SK_NETWORK_TESTS=1 cargo t-all`.

## Bildnachweis

Startbildschirm: Ansel Adams, *The Tetons and the Snake River* (1942), Grand-Teton-Nationalpark.
Als Werk eines Angestellten der US-Bundesregierung gemeinfrei
([Wikimedia Commons](https://commons.wikimedia.org/wiki/File:Adams_The_Tetons_and_the_Snake_River.jpg),
Original im US-Nationalarchiv); für den Startbildschirm beschnitten und verkleinert.

## Roadmap

- **Phase 1 (fertig):** RAW laden/anzeigen, Weißabgleich, Belichtung, HDR, Levels, Kurven, Sättigung/Dynamik, Drehen, Bewertung/Farb-Labels, Kopieren/Anwenden, Undo, Export JPEG/TIFF/PNG
- **Phase 2 (fertig):** Katalog (SQLite), Thumbnail-Cache, Alben/Smart-Alben, Keywords, Suche, Sortierung, Stile, Rasteransicht
- **Phase 3 (fertig):** Zuschnitt/Geraderichten/Keystone, Schärfen, Rauschreduzierung, Objektivkorrektur (manuell + Lensfun), Color Editor, Color Balance, Schwarzweiß, Klarheit/Struktur, lokales HDR, Körnung/Vignette, Auto-Anpassungen, echtes 100 % (Kachel-Rendering), Highlight-Rekonstruktion, Ausgabe-Farbräume mit ICC
- **Phase 4 (fertig):** Ebenen und Masken (Pinsel, Radierer, linearer/radialer Verlauf, Deckkraft), Reparieren/Klonen
- **Phase 5 (fertig):** Luma Range, Farbbereichsmasken, Style Brushes, KI-Motivmaske (ONNX), Himmelmaske, Magic Brush, Dunst entfernen, Speed Edit, Vergleichsansicht, mehrere Ausgaberezepte
- **Phase 6 (fertig):** Mehrfachansicht mit Referenzbild und synchronem Zoom, Angleichen von Belichtung/Weißabgleich an eine Referenz, Hauttöne vereinheitlichen
- **Phase 7 (fertig):** Varianten (virtuelle Kopien), Fokusmaske
- **Phase 8 (fertig):** Vorher/Nachher nebeneinander, Wasserzeichen (Text/Logo), EXIF und Copyright im Export, Hilfslinien und Layout-Overlay
- **Phase 9 (fertig):** automatisches Geraderichten (Drehung, vertikaler/horizontaler Keystone)
- **Phase 10 (fertig):** Lupe, Verlauf und Schnappschüsse, Umbenennen mit Platzhaltern (auch im Ausgaberezept)
- **Phase 11 (fertig):** XMP-Austausch, Stapel, Viewer auf dem Zweitmonitor, Softproof, HDR und Panorama, KI-Entrauschen, Entfernen, Gesichter und Sichtung
