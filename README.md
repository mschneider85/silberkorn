# Silberkorn

RAW-Entwickler und Bildverwaltung in Rust. Lizenz: [MIT](LICENSE).

## Herunterladen

**[Silberkorn für Windows herunterladen](https://github.com/mschneider85/silberkorn/releases/latest/download/silberkorn-setup-x64.exe)** (Windows 10 und 11, 64 Bit)

Webseite: <https://mschneider85.github.io/silberkorn/> · Alle Versionen: [Releases](https://github.com/mschneider85/silberkorn/releases)

## Selbst bauen

```bash
cargo run --release -p sk-app -- /pfad/zu/raws
```

Der erste Build dauert einige Minuten (u. a. wegen der ONNX-Inferenz-Bibliothek tract).
Ohne Pfad wird der zuletzt geöffnete Ordner verwendet. Falls wgpu (Vulkan) nicht startet,
fällt die App automatisch auf OpenGL zurück; erzwingen mit `SILBERKORN_RENDERER=glow`.

### Windows

Die Releases baut GitHub Actions unter Windows mit der MSVC-Toolchain (`.github/workflows/release.yml`).
Lokal geht dasselbe Ziel aus WSL/Linux mit [cargo-xwin](https://github.com/rust-cross/cargo-xwin), das
die Windows-SDK- und CRT-Dateien von Microsoft beim ersten Bauen lädt. Einmalig einrichten:

```bash
rustup target add x86_64-pc-windows-msvc
cargo install --locked cargo-xwin
sudo apt install clang lld
# Die Werkzeuge unter den Namen, die cargo-xwin erwartet. lld-link ruft Rusts eigenes lld auf:
# das lld 10 von Ubuntu 20.04 ist für die aktuelle CRT zu alt.
mkdir -p ~/.local/xwin-bin
ln -sf "$(command -v clang)" ~/.local/xwin-bin/clang-cl
ln -sf /usr/lib/llvm-10/bin/llvm-lib /usr/lib/llvm-10/bin/llvm-rc ~/.local/xwin-bin/
lld="$(rustc --print sysroot)/lib/rustlib/x86_64-unknown-linux-gnu/bin/rust-lld"
printf '#!/bin/sh\ncase "$1" in -flavor) exec "%s" "$@";; *) exec "%s" -flavor link "$@";; esac\n' "$lld" "$lld" > ~/.local/xwin-bin/lld-link
chmod +x ~/.local/xwin-bin/lld-link
```

Bauen (`CL` braucht clang-cl für die SSE4.1-/AVX2-Teile von libwebp; solche lokalen Builds laufen
deshalb nur auf Prozessoren mit AVX2, die Releases überall):

```bash
PATH=~/.local/xwin-bin:$PATH CL="-msse4.1 -mavx2" cargo xwin build --release -p sk-app --target x86_64-pc-windows-msvc
```

Ergebnis: `target/x86_64-pc-windows-msvc/release/silberkorn.exe`, eine einzelne Datei: die C-Laufzeit ist
eingebunden (`crt-static` in `.cargo/config.toml`, auch für die Releases), sonst bräuchte sie
`VCRUNTIME140.dll`, die einem frischen Windows fehlt. Deren `exp2f`/`log2f` sind langsam; in Pixel-Schleifen
deshalb `color::exp2`/`color::log2` nehmen. Unter Windows nativ geht `cargo build --release -p sk-app`.
Zum Messen von Geschwindigkeit immer diesen Weg nehmen: mingw-Builds (`x86_64-pc-windows-gnu`) nutzen die
alte msvcrt, deren Mathe-Funktionen 4–8× langsamer sind als in den Releases.

Einstellungen und Stile liegen unter Windows in `%APPDATA%\silberkorn`, Katalog, Modelle, Lensfun-Profile und
Thumbnails in `%LOCALAPPDATA%\silberkorn`.

Kleinere Datei (z. B. zum Weitergeben): `--profile dist` statt `--release` baut mit LTO, Ergebnis unter
`dist/` statt `release/`. Die `.exe` schrumpft so um etwa ein Viertel (von rund 110 auf rund 80 MB), ein
Build dauert aber etwa eine Viertelstunde statt gut einer Minute. Zusätzlich `strip = true` im Profil
entfernt die Funktionsnamen und spart weitere 10–25 MB; dann fehlen die Namen aber in den Backtraces
des Absturzprotokolls.

### Releases und Updates

Die Versionsnummer steht nur in `Cargo.toml` unter `[workspace.package]`. Ein neues Release:

```bash
# Version in Cargo.toml erhöhen, z. B. auf 0.2.0, dann:
cargo check -p sk-app        # übernimmt die Nummer in Cargo.lock
git commit -am "Version 0.2.0"
git tag v0.2.0
git push origin main v0.2.0
```

Der Tag startet `.github/workflows/release.yml`: GitHub Actions baut auf Windows mit `--profile dist`,
erzeugt mit Inno Setup den Installer (`installer/silberkorn.iss`) und veröffentlicht
`silberkorn-setup-x64.exe` und die einzelne `silberkorn-windows-x64.exe`, jeweils mit SHA-256-Datei, als
Release. Die Notizen entstehen aus den Commits seit dem letzten Tag (auf der Release-Seite nachträglich
bearbeitbar). Passt der Tag nicht zur Version in `Cargo.toml`, bricht der Workflow ab.

Der Installer installiert nur für den angemeldeten Benutzer nach `%LOCALAPPDATA%\Programs\Silberkorn`
(Startmenü-Eintrag, Deinstallation über die Windows-Einstellungen) und braucht deshalb nie
Administratorrechte. Beim Deinstallieren fragt er, ob auch Einstellungen, Stile, Katalog, KI-Modelle
und Vorschauen gelöscht werden sollen (Standard: behalten). Fotos und Sidecars bleiben immer erhalten.

Die App fragt beim Start höchstens einmal am Tag `api.github.com` nach dem neuesten Release
(nicht in Debug-Builds; abschaltbar unter Hilfe → „Nach Updates suchen …“). Ist eines neuer, erscheint
unten rechts „Update x.y.z“. Unter Windows lädt „Jetzt aktualisieren“ den Installer und prüft die
Prüfsumme; „Jetzt neu starten“ schließt die App, der Installer läuft still durch (nur ein
Fortschrittsfenster) und startet die neue Version. Wer nicht neu startet, bekommt das Update beim
Beenden. Auch eine einzeln gestartete `.exe` wird so zur installierten Version. Zum Testen:
`SILBERKORN_UPDATE_REPO=<owner/repo>` fragt ein anderes Repository, `SILBERKORN_DIALOG=update`
öffnet den Dialog beim Start.

Die Dateien sind nicht signiert: Beim ersten Ausführen des heruntergeladenen Installers warnt Windows
SmartScreen („Unbekannter Herausgeber“ → „Weitere Informationen“ → „Trotzdem ausführen“). Updates aus der
App heraus sind davon nicht betroffen.

Kommandozeilen-Entwicklung (ohne GUI):

```bash
SK_ADJ='{"exposure":0.5,"shadows":40}' cargo run --release -p sk-core --example develop -- bild.ARW ausgabe/
```

## Aufbau

| Crate | Inhalt |
|---|---|
| `sk-core` | RAW-Dekodierung (rawler), Farbmathematik, Entwicklungs-Pipeline, Sidecars, Export |
| `sk-ai`   | Neuronale Netze (tract/ONNX Runtime): Masken aus Bildinhalt, Tiefenkarte, Entrauschen, Entfernen, Gesichter |
| `sk-app`  | egui/wgpu-Oberfläche, Hintergrund-Worker |

Bereiche für Spezialfunktionen starten zugeklappt und bleiben offen, sobald man sie einmal öffnet:
Gesichter & Sichtung, Hauttöne, Schwarzweiß, Neu ausleuchten, Objektivunschärfe,
Atmosphäre, Leuchten, Softproof, Wasserzeichen, Druckvorschau.

Pipeline (Arbeitsfarbraum: lineares Rec.2020):

1. Geometrie: Zuschnitt, Geraderichten, Spiegeln, Drehen, Perspektive, Objektivkorrektur
   (Verzeichnung, CA, Vignettierung – manuell, aus den Kameradaten oder per Lensfun-Profil;
   Fisheye-Entzerrung)
2. Weißabgleich mit Highlight-Rekonstruktion, Kameramatrix
3. Rauschreduzierung (Luminanz: Guided Filter, Farbe: Chroma-Glättung)
4. Belichtung, lokales HDR, Klarheit/Struktur, Kontrast, Basiskurve
5. Farbe in Oklab: Farbeditor (8 Farbbereiche und bis zu 8 eigene), Farbbalance, Sättigung/Dynamik, Schwarzweiß mit Tonung
6. Helligkeit, Tonwerte, Kurven (RGB, Rot, Grün, Blau, danach die Luma-Kurve: nur die Helligkeit folgt ihr, die Farben werden mitskaliert, Farbton und Sättigung bleiben)
7. Schärfen, Leuchten, Vignette, Körnung
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
`~/.local/share/silberkorn/panic.log`. Ein Fehler beim Zeichnen der Oberfläche kostet nur diesen
Frame (Bearbeitungen werden sofort gespeichert). RAW-Dateien werden vor rawler geprüft
(`raw_check`): Verweise im Kreis, riesige Einträge und unplausible Bildgrößen, an denen rawler
unabfangbar abbräche, gelten als beschädigt.

**Nur eine Instanz** (`sk-app/src/single_instance.rs`): Zwei Silberkorn würden sich Sidecars, Katalog
und Einstellungen überschreiben. Die erste sperrt `silberkorn.lock` im Datenordner (das System gibt
die Sperre frei, wenn der Prozess endet, auch nach einem Absturz) und wartet auf einem lokalen Port;
Port und ein Zufallsschlüssel stehen in `silberkorn.port`. Ein weiterer Start – etwa ein Doppelklick
auf eine `.sksession` – reicht seinen Ordner oder seine Session an die laufende weiter, die sie öffnet
und nach vorn kommt, und beendet sich. Antwortet die laufende nicht binnen 5 s, startet er doch. Der
KI-Prozess und `SILBERKORN_SCREENSHOT` sind ausgenommen; beim Neustart nach Verlust der Grafikkarte
gibt der alte Prozess die Sperre vorher ab.

`cargo run --release -p sk-core --example bench -- <raw>` misst die Pipeline auf Prozessor und Grafikkarte.

### Ebenen & Masken

Lokale Anpassungen (Tab „Ebenen“) wirken als Deltas auf die Hintergrund-Einstellungen und werden pro
Pixel mit Maske × Deckkraft gemischt; überlappende Ebenen addieren sich. Masken bestehen aus
Pinselstrichen, linearen und radialen Verläufen, „Füllen“ und „Invertieren“.

**Ebenenmodus:** Ist eine Ebene gewählt, ändern die Regler in Belichtung, Farbe und Details diese Ebene
statt des ganzen Bildes: Belichtung, Kontrast, Helligkeit, Lichter, Tiefen, Weiß, Schwarz, Temperatur und
Tönung (relativ zum Hintergrund), Sättigung, Dynamik, die Bänder des Farbeditors, die Farbbalance,
Klarheit, Struktur und Dunst entfernen, außerdem die Gradationskurve (alle Kanäle und Luma). Die Kurve
einer Ebene wirkt auf das Ergebnis des Hintergrunds und wird mit Maske × Deckkraft eingeblendet.
Was nur aufs ganze Bild wirkt, ist im Ebenenmodus deaktiviert: die Werkzeuge Tonwerte, Schärfen,
Rauschreduzierung usw. (zugeklappt, „nur ganzes Bild“), die Tabs Objektiv, Effekte, Retusche und Stile sowie Zuschneiden, Drehen, Weißabgleich-Pipette, Automatisch
anpassen und Einfügen. Ein blauer Punkt in der Tab-Leiste markiert die Tabs, die auf die Ebene wirken.
Woran man gerade arbeitet, zeigt:
- eine Leiste oben in Belichtung, Farbe und Details: „Hintergrund“ oder blau „Ebene „…““, mit
  Auswahlmenü, Maskenanzeige und ✕ zurück zum Hintergrund; „Neue Ebene“ im Menü führt gleich zur
  Maske im Tab „Ebenen“, denn ohne Maske wirkt die Ebene noch nicht; fährt man mit der Maus
  darüber, wird die Maske eingeblendet,
- ein blauer Strich an den Werkzeugen und ein Schild „Ebene „…““ oben im Bild.

Den Ebenenmodus verlässt man mit ✕ in der Leiste, „Hintergrund“ in der Ebenenliste oder Alt+↓.
Alt-Klick auf das Auge einer Ebene zeigt nur sie (wie in Photoshop), ein zweiter wieder alle.
Im Tab „Ebenen“ fasst „Anpassungen“ zusammen, was die Ebene ändert; ein Klick springt zum Werkzeug.
Alt+↑ / Alt+↓ wählen die Ebene darüber bzw. darunter (unter der untersten liegt der Hintergrund).

**Verläufe** lassen sich nachträglich ändern. Mit gewähltem Werkzeug (L oder R) erscheinen
drei Linien bzw. Ellipsen: volle Wirkung, Mitte (50 %) und Ende des Übergangs.
- **Linear:** Mittellinie oder Mittelpunkt verschieben den Verlauf, die äußeren (gestrichelten) Linien
  machen den Übergang breiter oder schmaler, der runde Griff dreht ihn.
- **Radial:** Der Mittelpunkt verschiebt, die mittlere Ellipse skaliert, ihre vier eckigen Griffe ändern
  Breite bzw. Höhe, innere und äußere Ellipse die weiche Kante, der runde Griff dreht die Ellipse.
- Ziehen an einer freien Stelle legt einen neuen Verlauf an; **Entf** löscht den gewählten.

**Pinsel** (Pinsel, Radierer, Entfernen; Reparieren/Klonen ohne Fluss und „Kanten beachten“) haben jeweils
eigene, gespeicherte Einstellungen: **Größe**, **Härte** (weiche bis harte Kante), **Deckkraft** (das
Maximum eines Strichs) und **Fluss** (wie viel jeder Pinselabdruck beiträgt; innerhalb eines Strichs
baut sich die Deckung auf). **Kanten beachten** beschränkt den Strich auf Bereiche, die der
Stelle unter der Pinselmitte in Farbe und Helligkeit ähneln, sodass Kanten erhalten bleiben; verglichen
wird in der Auflösung der Maske (3072 Pixel), **Toleranz** bestimmt, wie verschieden es sein darf (50
wie früher; jeder Schritt von 50 halbiert bzw. vervierfacht den erlaubten Abstand bis 0 bzw. 100).
Mit gehaltenem **Alt** radiert der Pinsel (mit den Einstellungen des Radierers). Striche
werden dicht aufgezeichnet und als Catmull-Rom-Kurve mit Abdrücken im Abstand von 15 % des
Radius gerastert; Masken haben 3072 Pixel an der langen Kante. Beim Malen wird nur der aktuelle
Strich neu gerastert. Striche aus älteren Sidecars sehen aus wie zuvor. Sie werden in Quellkoordinaten gespeichert und bleiben deshalb nach Zuschnitt,
Drehen oder Objektivkorrektur am Bildinhalt.

„Reparieren“ und „Klonen“ (Tab „Retusche“) werden wie ein Pinsel über die Stelle gemalt (ein Klick ergibt einen runden
Punkt). „Reparieren“ übernimmt Struktur von einer automatisch gewählten Quelle und gleicht Farbe/Helligkeit an
die Umgebung an; „Klonen“ kopiert 1:1. Mit aktivem Werkzeug zeigt das Bild alle Bereiche mit Pins: der
weiße verschiebt den Bereich, der gelbe (beim ausgewählten) die Quelle. Im Abschnitt „Retuschen“
stehen alle Bereiche in einer Liste wie die Ebenen (Art, Name, Deckkraft), zusammen mit den entfernten; dort lassen sich Art, Deckkraft und
Weichheit ändern, eine neue Quelle suchen oder Bereiche löschen (↺ im Kopf löscht alle) (was eine Art nicht hat, ist ausgegraut). Einen gewählten Bereich erweitert man, indem man mit Umschalt dazumalt; mit Alt
nimmt man einen Teil weg (oder „Malen: Erweitern / Verkleinern“ im Abschnitt). Ein **Alt-Klick** (ohne zu
ziehen) legt die Quelle fest wie beim Kopierstempel in Photoshop: beim gewählten Bereich dort hin, ohne
gewählten Bereich für den nächsten (ein Fadenkreuz zeigt sie bis dahin). Die Striche gehören zum Bereich:
gleiche Größe, Weichheit und Quelle, und sie wandern mit, wenn man ihn verschiebt.

**Staub und Flecken finden** (Tab „Retusche“, oder über die Befehlssuche) macht Sensorstaub und Hautunreinheiten zu
Reparieren-Stellen: gesucht werden kleine, runde Stellen, die rundum dunkler als ihre Umgebung sind, auf ruhigen
Flächen (Himmel, Wände, Haut). Auf dem Logarithmus der Helligkeit misst eine Differenz zweier Weichzeichnungen
in vier Größen, wie viel dunkler eine Stelle als ihr Ring ist; das muss die übliche Antwort der Umgebung
(Struktur, Rauschen) deutlich übertreffen, und der Ring muss in allen zwölf Richtungen heller sein, sonst ist es
eine Kante oder Linie. Die **Empfindlichkeit** senkt beide Schwellen. Die Suche läuft auf der Vorschau im
Hintergrund (etwa 0,2 s), bereits reparierte Stellen werden übersprungen, und danach zeigt das
Reparieren-Werkzeug alle Stellen zum Prüfen. Kommandozeile: `cargo run --release -p sk-core --example spots -- <raw> <bild.png> [empfindlichkeit]`

### Auswahl aus Bildinhalt

- **Motiv auswählen:** neuronales Netz BiRefNet lite (MIT, 224 MB, Export von onnx-community). Es sieht das
  ganze Bild auf 1024 × 1024 gestreckt und trennt auch Haare, Fell und dünne Teile; die Maske hat 1024 Pixel
  an der langen Seite (andere Masken 768). Es rechnet immer auf dem Prozessor (unter Windows 10–30 s mit
  ONNX Runtime, etwa 45 s mit tract): Auf einer GTX 1650 Ti mit 4 GB rechnete es Minuten, bis Windows die
  Grafikkarte zurücksetzte. Damit man nicht wartet: Zeigt der Tab „Ebenen“ ein Bild, rechnet BiRefNet
  still im Hintergrund in einem eigenen Thread (Objekt, Himmel und Zauberpinsel warten nicht darauf), und die
  Masken der letzten acht Bilder bleiben im Speicher. Kommt der Klick vorher, setzt U²-Net-p sofort eine grobe
  Maske; die genaue ersetzt sie, sobald sie fertig ist, auch im Verlauf (kein eigener Rückgängig-Schritt; wurde
  die Maske inzwischen geändert, bleibt die Änderung). Automatisch anpassen und Look angleichen nehmen weiter das schnelle
  U²-Net-p (Apache-2.0, 4,6 MB; `SILBERKORN_MODEL=<pfad>`). Beide werden beim Programmstart still nach
  `~/.local/share/silberkorn/models/` geladen.
- **Himmel auswählen:** helle, strukturarme, blaue oder neutrale Bereiche, die mit dem oberen Rand verbunden sind.
- **Zauberpinsel (J):** über den Bereich malen; gewählt wird, was zusammenhängt und einer der überstrichenen Farben ähnelt (Größe und Toleranz einstellbar).
- **Objekt (Umschalt+J):** auf ein Objekt klicken, es wird ausgewählt; weitere Klicks verfeinern, Alt+Klick
  nimmt Teile aus, Umschalt+Klick wählt ein weiteres Objekt dazu. MobileSAM (Apache-2.0; Bild-Encoder und
  Masken-Decoder von Segment Anything als ONNX, 45 MB, geladen beim ersten Wählen des Werkzeugs). Der Encoder analysiert das Bild
  einmal, sobald das Werkzeug gewählt wird (etwa 4 s auf dem Prozessor); jeder Klick braucht dann nur noch
  den Decoder (etwa 0,2 s). tract rechnet nur dessen 256er-Maske, Silberkorn skaliert sie selbst hoch und
  verfeinert die Kanten am Bild. Jeder Klick ist ein Rückgängig-Schritt.
- **Helligkeitsbereich** und **Farbbereich** (mit Pipette, unter „Maske“ → „Eingrenzen“) beschränken jede Maske.
  Der Helligkeitsbereich ist wie in Capture One ein Trapez über dem Histogramm: Zwischen den oberen
  Griffen wirkt die Ebene voll, zu den unteren läuft sie aus. Das Trapez lässt sich als Ganzes
  verschieben; die Werte stehen darunter (0–255).
- **Stilpinsel** (+ unter der Ebenenliste → „Neue Ebene mit Stilpinsel“): Stilpinsel (Aufhellen, Abdunkeln, Wärmer, Haut glätten, Dunst entfernen, …).
- **Neue Ebene mit Look** (+ unter der Ebenenliste): eine Ebene über das ganze Bild mit einem
  Filmlook oder eigenen Stil; ihre Deckkraft ist die Stärke, ihre Maske kann ihn eingrenzen (siehe
  „Stile“).

### KI-Werkzeuge: Raw-Details, Entrauschen und Entfernen

Die neuronalen Netze (außer dem von Raw-Details) werden nicht mitgeliefert, sondern nach `~/.local/share/silberkorn/models/`
geladen (`SILBERKORN_MODELS=<ordner>` überschreibt den Ort), und zwar still beim Programmstart, ohne
Nachfrage (zusammen etwa 710 MB, unter Windows samt KI-Beschleunigung). Die Modelldateien von
„Verwacklung entfernen“ (nur in 0.2.6) und deren Zwischenspeicher löscht der Start wieder. Der Startbildschirm zeigt den
Fortschritt, danach läuft der Download in der Statusleiste weiter; man kann sofort arbeiten. Wird eine
Funktion benutzt, bevor ihr Modell da ist (oder fehlte beim Start das Internet), sieht man den Fortschritt
am Werkzeug, danach läuft die Funktion von selbst weiter; ein fehlgeschlagener Download wird beim
nächsten Gebrauch wiederholt. Modellnamen und Lizenzen stehen unter Hilfe → „Lizenzen …“. Jede Datei wird gegen Größe und SHA-256 geprüft (Hugging-Face-Links auf einen festen Stand
festgelegt); ein abgebrochener Download wird beim nächsten Versuch fortgesetzt.

**Kennzeichnung:** KI-Funktionen tragen ein gelbes Funken-Symbol hinter ihrem Namen, in den
Werkzeugbereichen (Objektivunschärfe, Neu ausleuchten, Atmosphäre, Entfernen, Gesichter & Sichtung),
an Werkzeugen und Knöpfen (Raw-Details, Objekt, Motiv, Himmel, Tiefe, KI-Entrauschen), in den Menüs
und in der Befehlspalette.

**KI-Beschleunigung** (Bearbeiten → „Grafikkarte …“, Abschnitt „KI-Berechnung“, Windows): Die Netze rechnen dann über
ONNX Runtime mit DirectML auf der Grafikkarte (jede DirectX-12-Karte). ONNX Runtime wird nicht
gelinkt, sondern zur Laufzeit geladen: Beim ersten Start lädt Silberkorn `onnxruntime.dll` (1.24.4) und
`DirectML.dll` (1.15.4) aus Microsofts NuGet-Paketen nach `%LOCALAPPDATA%\silberkorn\gpu`, und zwar
per Teil-Download nur diese beiden Dateien (etwa 28 MB statt über 200 MB). Die EXE bleibt eine
einzelne Datei, der Cross-Build aus WSL funktioniert weiter. Steht DirectML nicht zur Verfügung,
rechnet ONNX Runtime auf dem Prozessor (etwa doppelt so schnell wie ohne); ohne ONNX Runtime oder
bei Fehlern übernimmt tract. Zum Testen auf anderen Systemen: `SILBERKORN_ORT=<libonnxruntime.so>`.
ONNX Runtime und DirectML laufen in einem eigenen Prozess (`silberkorn --ki-prozess`, gestartet beim
ersten Netz, Daten über stdin/stdout): Ein Absturz darin oder im Grafiktreiber beendet nur diesen.
Die geladenen Netze rechnen dann mit tract weiter, das nächste startet einen neuen Prozess; nach zwei
Abstürzen bleibt die Grafikkarte für KI bis zum nächsten Start aus. Setzt Windows die Grafikkarte zurück (DirectML meldet
`887A0006` o. ä.), rechnen alle Netze bis zum nächsten Start auf dem Prozessor. Antwortet der Prozess fünf Minuten
nicht, gilt er als abgestürzt. `cargo run --release -p sk-ai --example ki_prozess -- <silberkorn>`
vergleicht die Zeiten und beendet den Prozess zur Probe.

Alles, was länger dauert (KI, Zusammenfügen, Exportieren, Indexieren, Downloads), zeigt die
Statusleiste mit Fortschritt an; Export, Zusammenfügen und Entrauschen lassen sich dort mit ✕
abbrechen, und beim Beenden fragt Silberkorn nach, solange sie laufen; worauf man im Bild wartet (Maske, Entfernen, Angleichen),
erscheint zusätzlich oben im Bild.

- **Abschnitt „Verbessern“** (Tab „Details“, ganz oben; Entwickeln → „Verbessern“): Raw-Details und
  KI-Entrauschen, wie Lightroom „Verbessern“. Das Übertragen-Symbol im Kopf gilt für beide, die
  klassische Rauschreduzierung (Luminanz, Farbe) ist ein eigener Abschnitt darunter. Bei 100 %
  KI-Entrauschen ist Raw-Details gesperrt und wirkungslos: RawNIND setzt die Farben selbst neu zusammen.
  Raw-Details lässt Rauschen und Textur stehen (für rauscharme Bilder), KI-Entrauschen glättet.
- **Raw-Details**: Demosaicing durch
  DemosaicNet (Michaël Gharbi et al., „Deep Joint Demosaicking and Denoising“, 2016) statt AHD –
  feine Details mit weniger Farbsäumen, Treppen und Moiré, ähnlich Lightrooms „Raw-Details“. Die
  Gewichte stehen unter **MIT** und sind mitgeliefert (`crates/sk-ai/models/`, 2.3 MB, aus
  `bayer.pth` unverändert nach ONNX exportiert; Lizenztext unter Hilfe → „Lizenzen …“). Nur für
  Bayer-Sensoren; bei X-Trans, linearen DNGs und HDR ist der Regler gesperrt. Das Netz lernte auf
  weißabgeglichenen, gammakodierten Bildern: Das Mosaik wird vorher abgeglichen und kodiert (Gamma
  2.2), danach zurückgerechnet. Es hat keinen Rand: Kacheln zu 768 liefern 706 Pixel, sie schließen
  nahtlos aneinander an (mit der Phase des Bayer-Musters). Übersteuerte Lichter bleiben wie bei AHD.
  Berechnet, gehalten und gemischt wird wie beim KI-Entrauschen (24 MP etwa 50 s auf 12 Kernen ohne
  Grafikkarte; `~/.cache/silberkorn/raw-details/`, höchstens 10 GB, `SILBERKORN_DETAILS_CACHE`);
  **Stärke** mischt es in die AHD-Daten, das KI-Entrauschen kommt danach. Kommandozeile:
  `cargo run --release -p sk-ai --example raw_details -- <raw> <ordner> [x y]` (Ausschnitt mit AHD und Raw-Details).
- **KI-Entrauschen**: RawNIND (UtNet2, Benoit
  Brummer, UCLouvain) in der Fassung von darktable (darktable-ai 5.6.0). Die Gewichte stehen unter
  **GPL-3.0**; sie werden nicht mitgeliefert, sondern wie die anderen Modelle beim Start geladen (etwa 57 MB,
  per Teil-Download nur die beiden ONNX-Dateien aus dem Archiv).
  Bei Bayer-Sensoren bekommt das Netz das Mosaik selbst (R, G, G, B, schwarzwertbereinigt, nicht
  weißabgeglichen) und liefert sauberes Kamera-RGB in voller Auflösung, ersetzt also auch das
  Demosaicing; X-Trans und andere Sensoren laufen demosaict durch ein zweites Netz für lineares
  Rec.2020. Kacheln zu 512 mit 64 Überlappung, die Helligkeit jeder Kachel wird an die Eingabe
  angeglichen; übersteuerte Lichter bleiben unverändert.
  Das Ergebnis wird je Bild einmal im Hintergrund berechnet (das angezeigte Bild zuerst; 24 MP etwa
  30 s auf 12 Kernen, X-Trans deutlich länger) und als komprimierte Halbfloat-Datei in
  `~/.cache/silberkorn/denoise/` gehalten (höchstens 20 GB, die ältesten fallen heraus;
  `SILBERKORN_DENOISE_CACHE` überschreibt den Ort). **Stärke** mischt es vor allen anderen Stufen
  in die RAW-Daten: nicht-destruktiv, rückgängig zu machen, gilt für Vorschau, Export und
  Thumbnails. Fehlt es beim Export, wird es dort berechnet – nie wird stillschweigend ohne
  exportiert. Kommandozeile: `cargo run --release -p sk-ai --example denoise -- <raw> <ordner> [stärke] [--linear]`
- **Entfernen** (X, Tab „Retusche“): Über Störendes malen; beim Loslassen füllt LaMa (Roman Suvorov
  et al., Samsung AI Center, Apache-2.0; ONNX von Carve, 208 MB, auch mit tract lauffähig) die Stelle
  passend zur Umgebung; bis 0.2.14 war es MI-GAN. DirectML lehnte die Matrixprodukte seiner
  Fourier-Faltungen ab (eine 2-D-Matrix mal einen 5-D-Tensor, „Falscher Parameter“); Silberkorn lädt
  darum eine angepasste Datei aus einem eigenen Release, in der sie als gleichwertige 4-D-Produkte
  stehen (gleiche Ergebnisse, `crates/sk-ai/models/lama_dml.py`). Carves unveränderte Datei (bis 0.2.18)
  löscht der Start. Unter Windows rechnet es so auf der Grafikkarte: etwa 0.4 s pro 512er-Fenster auf
  einer GTX 1650 Ti (auf dem Prozessor 3.8 s, mit tract 10 s), gut 800 MB Grafikspeicher. Geladen wird
  das Netz (gut 10 s, auf der Grafikkarte samt erstem Lauf, der die Shader übersetzt) im Hintergrund,
  sobald das Werkzeug gewählt wird, nicht erst mit dem ersten Strich. Das Netz rechnet so verkleinert, dass die
  Markierung höchstens die Hälfte seines 512er-Fensters einnimmt (es sieht dann genug Umgebung,
  im Maßstab der Bilder, an denen es lernte); größere Flächen in mehreren Fenstern. Danach in voller
  Auflösung (sehr große Flächen etwas verkleinert): Helligkeit und Farbe werden am Rand gemessen und
  ins Innere verteilt (Push-Pull). Musste das Netz verkleinert rechnen (voll ab
  1.6-fach), kommt die feine Struktur – samt Korn – blockweise aus Stellen der Umgebung mit ähnlicher
  Farbe (Wald aus Wald, nicht aus Fels oder Wolke); wo keine passt, und bei kleineren Flächen, bleibt
  die Struktur des Netzes. Das Ergebnis ist ein kleiner Flicken mit weicher Kante im Sidecar (Feld
  `removals`): Er wird vor Reparieren/Klonen eingesetzt, bleibt am Bildinhalt wie die Masken und
  ist ein Rückgängig-Schritt. Die entfernten Bereiche stehen in der Liste „Retuschen“ und sind im Bild umrandet;
  ein Klick auf den Pin wählt einen. Er hat eine eigene Deckkraft, lässt sich einzeln löschen (Entf)
  und wie bei Reparieren/Klonen mit Umschalt erweitern oder mit Alt verkleinern: Dann wird nur dieser
  Bereich mit allen seinen Strichen neu gefüllt und behält Platz und Deckkraft. ↺ im Kopf der Retuschen
  stellt das Original wieder her.
- **Objektivunschärfe** (Tab „Effekte“): zeichnet Vorder- und
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
- **Tiefe auswählen** (Tab „Ebenen“, Maske → Automatisch auswählen): eine Maske aus derselben
  Tiefenkarte, anfangs das nächste Fünftel des Bildes. Den Bereich stellt danach ein Trapez über der
  Verteilung der Entfernungen ein, wie beim Helligkeitsbereich: von nah (links) bis fern (rechts), mit
  getrenntem Übergang nach vorne und hinten (gespeichert mit der Maske, rückgängig zu machen).
- **Neu ausleuchten** (Tab „Belichtung“): **Belichtung** und **Wärme** für
  Vorder- und Hintergrund getrennt, etwa um ein Motiv im Gegenlicht aufzuhellen. Die **Trennung**
  liegt über der Verteilung der Entfernungen (links nah, rechts fern): Die Linie ist die **Grenze**, die
  gestrichelten Linien sind Anfang und Ende des **Übergangs**, jede einzeln zu ziehen.
- **Atmosphäre** (Tab „Effekte“): Dunst, der mit der Entfernung dichter
  wird. **Reichweite** bestimmt, wie weit nach vorn er reicht, **Helligkeit** und **Wärme** färben ihn.

  Beide nutzen dieselbe Tiefenkarte wie die Objektivunschärfe und werden intern zu Ebenen mit
  Tiefenmasken (Belichtung/Wärme bzw. negatives „Dunst entfernen“), rechnen also auf Grafikkarte und
  Prozessor gleich und gelten für Vorschau, Thumbnails und Export. Bis die Tiefenkarte da ist, wirken
  sie nicht; der Export berechnet sie bei Bedarf.

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
  auch als Filteralbum speichern.
- **Personen:** SFace (OpenCV Zoo, Apache-2.0) berechnet für jedes Gesicht einen Merkmalsvektor;
  ähnliche Gesichter werden derselben Person zugeordnet. Die Liste zeigt benannte Personen und
  solche auf mindestens zwei Bildern, jede als Kachel mit ihrem Gesicht (dem größten, das von ihr
  gefunden wurde), Name und Bildanzahl. Ein Klick zeigt ihre Bilder, ein zweiter wieder alle, ein
  Doppelklick benennt sie. Wird dieselbe Person doppelt erkannt, eine Kachel auf die andere ziehen.
  Nach einer Rückfrage gehören alle Gesichter zur
  Zielperson; sie behält ihren Namen oder übernimmt den der anderen, und das Keyword folgt.
  Umgekehrt zeigt eine gewählte Person darunter alle ihre Gesichter einzeln. Gehören welche zu
  jemand anderem: auf die richtige Person ziehen (Strg+Klick wählt mehrere, die dann zusammen
  gezogen werden), oder auf die Kachel „Neue Person“, die beim Ziehen am Ende der Liste erscheint
  (die neue Person wird gleich benannt und bleibt auch mit einem Bild in der Liste). Bilder, die die Person nicht mehr zeigen, verlieren ihren Namen als Keyword. Wird ein
  Bild neu analysiert, ordnet die Erkennung seine Gesichter wieder selbst zu.
  Der Name wird als Keyword der Bilder gesetzt (auch im XMP, wenn aktiviert); eine Person, die
  bereits so heißt, wird zusammengeführt.
- **Stapel → „Schärfstes Bild nach oben“** macht in jedem ausgewählten Stapel das schärfste Bild
  zum obersten.

Die drei Modelle (zusammen etwa 40 MB) werden beim ersten Analysieren geladen.
Kommandozeile: `cargo run --release -p sk-ai --example faces -- <bild>...`

### Anpassungen kopieren und übertragen

Silberkorn kopiert und überträgt Werkzeuge statt immer den ganzen Satz:

- **Strg+Umschalt+C** kopiert die bearbeiteten Werkzeuge des Bildes, **Strg+Umschalt+V** wendet
  genau diese auf die Auswahl an; alles andere behalten die Bilder. Zuschnitt, Drehen & Spiegeln,
  Trapezkorrektur, Ebenen und Retusche gehören zum Bildinhalt und sind nur dabei, wenn man sie wählt.
- **Anpassungen auswählen und kopieren …** (Menü „Bearbeiten“ oder Rechtsklick auf das
  Kopieren-Symbol der Werkzeugleiste) zeigt alle Werkzeuge zum Ankreuzen; bearbeitete sind heller.
  „Kopieren“ legt die Wahl in die Zwischenablage, „Auf N Bilder anwenden“ überträgt sie gleich.
- **Übertragen-Symbol im Kopf eines Werkzeugs** (erscheint, wenn mehrere Bilder ausgewählt sind):
  überträgt nur dieses Werkzeug auf die anderen ausgewählten Bilder.

### Eigene Farbbereiche

Im Farbeditor (Tab „Farbe“) zeigt **„Eigene“** frei festgelegte Farbbereiche: **Aufnehmen** und auf eine Farbe im Bild klicken legt einen Bereich um ihren Farbton an
(bis zu acht). **Farbe**, **Breite** und **Weichheit** bestimmen, welche Farbtöne er erfasst; der
Streifen darüber zeigt es auf dem Farbkreis. **Farbton**, **Sättigung** und **Helligkeit** ändern
dann nur diese Farben. Gewählt wird nach der Farbe, wie sie in den Farbeditor kommt, also so, wie sie
aufgenommen wurde, auch wenn die acht festen Bereiche sie schon verschieben. Fast neutrale Farben
bleiben unberührt. Die Bereiche rechnen auf Grafikkarte und Prozessor gleich.

### Vergleichen und Angleichen

- **Vergleichsansicht:** Sind mehrere Bilder ausgewählt (Strg/Umschalt+Klick), zeigt das Einzelbild sie
  nebeneinander (bis zu 12). Mausrad und Ziehen zoomen bzw. verschieben alle Bilder gemeinsam,
  Doppelklick wechselt zwischen Einpassen und Vergrößerung. Beim Hineinzoomen wird der sichtbare
  Ausschnitt jedes Bildes aus der vollen Auflösung nachgeladen. Ein Klick macht ein Bild zum
  bearbeiteten (blau umrandet), ohne die Auswahl zu ändern. Abschalten: Ansicht → „Auswahl
  nebeneinander vergleichen“.
- **Referenzbild** (Umschalt+Enter oder Rechtsklick in der Vergleichsansicht): Es bleibt links neben
  dem aktuellen Bild stehen, auch beim Durchblättern.
- **Look angleichen** (Tab „Farbe“ oder Menü „Entwickeln“): Die Auswahl bekommt den Look einer oder
  mehrerer Vorlagen. „Vorlage hinzufügen“ bietet eine Bilddatei (JPEG, PNG, TIFF, WebP, etwa ein
  Filmstill; auch per Ziehen und Ablegen), das geöffnete Bild oder das Referenzbild der
  Vergleichsansicht an, Bilder der Bibliothek so, wie sie bearbeitet sind. Jede Vorlage erscheint als
  Kachel mit ihrer Ähnlichkeit zum aktuellen Bild. Verglichen wird, wie die Bilder
  aussehen, über Statistiken, die keine gleichen Motive brauchen:
  - Belichtung: mittlere Helligkeit.
  - Weißabgleich: Farbstich der fast neutralen Töne.
  - Tonwerte: Schwarz- und Weißpunkt, Helligkeit und Kontrast als Luma-Kurve.
  - Farben: Sättigung und, pro Band des Farbeditors, Farbton, Sättigung und Helligkeit.
  - Farbstimmung: Farbbalance in Schatten, Mitten und Lichtern (Split-Toning).

  Der Himmel zählt dabei nicht mit, er sagt mehr über das Wetter als über den Look. Das Hauptmotiv
  zählt dreifach, wenn das Motivmodell geladen ist.

  Die **Ähnlichkeit** vergleicht Bildaufbau (ein grobes Helligkeitsraster nach Rang, unabhängig von
  Belichtung und Kurven, tolerant gegen etwas anderen Ausschnitt und Zoom), Helligkeitsverteilung, Farben (mit herausgerechnetem Farbstich) und
  Himmelsanteil. Sie wirkt zweifach: Mehrere Vorlagen werden nach Ähnlichkeit gewichtet. Und ein ähnliches Bild, etwa aus derselben Serie, wird vollständig
  angeglichen. Bei einem anderen Motiv werden Belichtung, Tonwerte und Farben nur teilweise
  übernommen, damit z. B. ein Foto mit viel hellem Himmel nicht jedes Bild ausfrisst.

  Das Ergebnis sind normale Regler, die sich weiter bearbeiten lassen. Gemessen wird am klein
  gerenderten Bild, ein paar Runden je Stufe. Eine Schwarzweiß-Referenz (auch getönt, etwa Sepia)
  nimmt die Farbe ganz heraus, und die Farbbalance übernimmt die Tönung. Was angeglichen wird, lässt
  sich mit Schaltern wählen. Nach dem Angleichen lässt sich die Stärke (0–100 %) noch ändern. Darunter
  steht, was sich geändert hat und welche Referenz wie viel beigetragen hat.

  Ausprobieren ohne App: `cargo run --release -p sk-ai --example look -- aus.png bild.ARW referenz.jpg …`
- **Schärfemaske** (K oder Ansicht → „Schärfemaske“): Scharfe Bereiche werden grün markiert, auch in der
  Vergleichsansicht und auf den Vorschaubildern, um schnell das schärfste Bild einer Serie zu finden. Gemessen wird die Energie der
  feinsten Details, die bei Unschärfe zuerst verschwinden; hineingezoomt in voller Auflösung. Die Empfindlichkeit lässt sich im Menü
  „Ansicht“ einstellen.
- **Hauttöne** (Tab „Farbe“): Mit der Pipette einen Hautton aufnehmen. „Gleichmäßigkeit“ zieht
  Farbton, Sättigung und Helligkeit der ähnlichen Farben zu diesem Ton hin (Flecken, Rötungen),
  „Hautton verändern“ verschiebt den ganzen Bereich.

**Schärfen** (Tab „Details“): **Stärke**, **Radius** und **Schwelle** wie gewohnt (Unscharf maskieren;
der Radius ist das Sigma der Weichzeichnung, 0.2–1.5 – seine Säume reichen etwa dreimal so weit, 1.5
entspricht damit ungefähr dem Höchstwert anderer Programme; ältere Bearbeitungen mit mehr bleiben),
dazu **Details** wie in Lightroom (i-Symbol am Regler): Er bestimmt, *wie* geschärft wird, nicht wie
stark. 0 ist Unscharf maskieren, 100 ist Dekonvolution (`sk-core/src/deconvolution.rs`):
Richardson-Lucy auf der Helligkeit der linearen Daten, 8 Durchgänge, mit einem Gauß-Radius, der aus
den schärfsten waagerechten und senkrechten Kanten des Bildes gemessen wird (10–90-%-Anstieg,
schärfstes Viertel, 80 % davon, 0.5–1.6 px). Flächen ohne Detail und ausgebrannte Lichter bleiben
ausgespart, kein Pixel ändert sich um mehr als den Faktor 1.35 (gegen Säume). Dazwischen wird die
Stärke geteilt; bei Details 100 ist die Dekonvolution ab Stärke 120 voll, darüber kommt wieder
Unscharf maskieren dazu (`Detail::sharpen_split`). Radius und Schwelle sind gesperrt, solange kein
Unscharf maskieren wirkt. Die Dekonvolution wird je Bild einmal gerechnet (24 MP etwa 1 s), der Regler
mischt danach nur.

**Alt beim Ziehen eines Reglers** zeigt statt des Bildes, was er bewirkt, solange er gehalten wird
(`SliderView`), wie in Lightroom:
- Belichtung, Lichter, Weiß und Weißpunkt der Tonwerte: Schwarz, ausgefressene Stellen weiß bzw. in der
  Farbe der vollen Kanäle; Tiefen, Schwarz und Schwarzpunkt der Tonwerte: Weiß, abgesoffene Stellen
  schwarz bzw. farbig. Dieselben Grenzen wie die Belichtungswarnung (255 und 0).
- Schärfen: Stärke das Bild in Graustufen, Radius und Details nur den Schärfungsanteil als Relief auf
  Mittelgrau, Schwelle die Maske (weiß wird geschärft); am besten bei 100 %.
- Rauschreduzierung (Luminanz, Farbe): was entfernt wird, als Relief auf Mittelgrau. Körnchen und
  Farbflecken heißen, sie arbeitet richtig; erkennbare Umrisse, sie nimmt schon Detail mit.

**Leuchten** (Tab „Effekte“, Orton-Effekt): eine weichgezeichnete Kopie des Bildes wird, vor allem aus
ihren hellen Teilen, per „Negativ multiplizieren“ darübergelegt; Lichter strahlen in die Umgebung.
**Größe** bestimmt die Weichzeichnung (0,3 bis 3,3 % der langen Kante); gerechnet wird nach dem
Schärfen auf Grafikkarte und Prozessor gleich, in Kacheln mit passendem Rand.

Weitere Funktionen: **Dunst entfernen** (Dark-Channel-Prior, global und pro Ebene), **Schnellbearbeitung**
(Taste halten + Mausrad), **Vorher/Nachher** geteilt mit verschiebbarem Trenner (U) oder nebeneinander
mit gemeinsamem Zoom und Ausschnitt (Umschalt+U), und **mehrere Ausgaberezepte**, die in einem Durchgang
verarbeitet werden (das RAW wird pro Bild nur einmal entwickelt).

### HDR, Panorama, Focus Stacking und Langzeitbelichtung

**Entwickeln → Zusammenfügen** (mindestens zwei Bilder ausgewählt) erzeugt ein neues
**lineares DNG** neben den Originalen (`…-HDR.dng`, `…-Pano.dng`, `…-Fokus.dng`, `…-Langzeit.dng`): demosaicte Kameradaten als
16-Bit-Gleitkomma, verlustfrei komprimiert. Weißabgleich, Farbe und Tonwerte bleiben daher voll
bearbeitbar, und auch Lightroom und Capture One öffnen die Datei. Alles läuft im Hintergrund
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
- **Focus Stacking:** Eine Reihe mit wandernder Schärfeebene wird zu einem Bild, das überall scharf
  ist. Referenz ist das mittlere Bild; die anderen werden daran ausgerichtet (Harris-Ecken wie beim
  Panorama, auf leicht weichgezeichneten Bildern, damit scharfe und unscharfe Fassung derselben Stelle
  zusammenpassen; RANSAC, danach eine Ähnlichkeitstransformation über alle passenden Punkte). So
  werden die Größenänderung beim Fokussieren („Focus Breathing“) und kleine Verschiebungen aus der
  Hand ausgeglichen. Jedes Bild zählt pro Pixel mit dem Quadrat seiner örtlichen Schärfe
  (geglättetes Quadrat des Laplace-Operators der Helligkeit): Das schärfste Bild überwiegt, Flächen
  ohne Struktur mitteln alle Bilder und rauschen weniger. Die Bilder werden nacheinander addiert,
  der Speicherbedarf hängt also nicht von ihrer Zahl ab.
- **Langzeitbelichtung:** Eine Serie derselben Szene wird zu einem Bild, als wäre lange belichtet
  worden. Referenz ist die mittlere Aufnahme; die anderen werden wie beim Focus Stacking
  ausgerichtet (abschaltbar, vom Stativ unnötig) und, falls die Kamera sie anders belichtet hat, auf
  ihre Helligkeit gebracht. Drei Verfahren:
  **Mittelwert** (`…-Langzeit.dng`) wie eine lange Belichtung ohne ND-Filter: Wasser und Wolken
  werden weich, das Rauschen sinkt. **Median** (`…-Median.dng`) nimmt je Pixel die Aufnahme, die der
  Helligkeit nach in der Mitte liegt, als Ganzes: Was durchs Bild läuft (Passanten, Autos),
  verschwindet. **Hellste** (`…-Lichtspuren.dng`) nimmt je Pixel die hellste Aufnahme: Licht- und
  Sternspuren, Feuerwerk; hier ohne Ausrichten, sonst richten sich die Sterne aus. Mittelwert und
  Hellste addieren nacheinander. Der Median braucht alle Aufnahmen zugleich (16 Bit je Kanal) und
  nimmt höchstens so viele, gleichmäßig über die Serie verteilt, wie in 2 GB passen (bei 24 MP 14).

Kommandozeile: `cargo run --release -p sk-core --example merge -- hdr|pano <raw>...`

### Kamera-Kurve

In jeder RAW-Datei steckt das JPEG, das die Kamera selbst entwickelt hat. Neue Bilder starten mit
dessen Tonkurve (Tab „Farbe“ → Basismerkmale → Kurve „Kamera“): Helligkeit, Kontrast und
Lichter sehen aus wie in der Kamera, mit der Abstimmung des Herstellers und der beim Fotografieren
eingestellten Belichtungskorrektur. Die übrigen Regler setzen darauf auf.

- **Wie es funktioniert:** Die Helligkeitsverteilung des JPEGs wird pro Bild auf die RAW-Daten
  abgebildet. Der n-dunkelste RAW-Wert bekommt die Helligkeit des n-dunkelsten JPEG-Pixels. So
  braucht es keine pixelgenaue Deckung, auch wenn die Kamera Verzeichnung korrigiert oder das
  Vorschaubild schwarze Ränder hat. Ein Durchgang durch die echte Pipeline gleicht den Rest aus.
  Wie in der Kamera wirkt die Kurve auf jeden Farbkanal einzeln: Lichter laufen nach Weiß aus,
  Schatten werden satter.
- **Wann:** beim ersten Öffnen eines nie bearbeiteten Bildes (gleich gespeichert), beim Export
  und bei „Automatisch anpassen“ noch nie geöffneter Bilder sowie nach „Alle Anpassungen
  zurücksetzen“. Bereits bearbeitete Bilder behalten ihre Kurve. Eine andere Kurve lässt sich
  jederzeit wählen, auch als Teil eines Stils.
- **Ohne:** Zusammengefügte DNGs und Dateien ohne brauchbares eingebettetes JPEG bleiben bei
  „Film Standard“. Eingefügte Einstellungen behalten die Kurve des Zielbildes; beim Öffnen bekommt es
  seine eigene.

### Automatisch anpassen

**Automatisch anpassen** (Strg+L, Knopf im Tab „Belichtung“ oder Entwickeln → „Automatisch
anpassen“) stellt mehrere Werkzeuge in einem Rückgängig-Schritt ein:
Weißabgleich, Belichtung, Lichter, Schatten, Weiß, Schwarz, Kontrast und Dynamik, auf Wunsch auch
die Tonwerte. Was dabei gesetzt wird, legt das Menü neben dem Knopf fest. Sind mehrere Bilder
ausgewählt, werden alle angepasst, im Hintergrund. Darunter steht der Regler **Stärke**, aktiv nach
einer automatischen Anpassung des Bildes: Er blendet zwischen den Einstellungen vorher (0 %) und dem vollen Auto-Ergebnis (100 %)
über, den Weißabgleich in Mired.

**A im Werkzeugkopf:** Werkzeuge, die sich selbst einstellen können, tragen im Kopf ein **A**. Es
stellt nur dieses Werkzeug ein, die übrigen Regler bleiben, wie sie sind. Wie „Automatisch anpassen“
wirkt es auf alle ausgewählten Bilder, im Hintergrund, und jedes A ist auch ein Befehl (Befehlssuche,
eigenes Tastenkürzel). Auf einer Ebene ist es deaktiviert.

| Werkzeug | A stellt ein | Befehl |
|---|---|---|
| Weißabgleich | Weißabgleich aus den fast neutralen Stellen | Weißabgleich automatisch |
| Belichtung | Belichtung und Kontrast | Belichtung automatisch |
| Lichter & Tiefen | Lichter, Tiefen, Weiß und Schwarz | Lichter & Tiefen automatisch |
| Farbe | Dynamik | Dynamik automatisch |
| Tonwerte | Schwarz- und Weißpunkt für alle Kanäle (je höchstens 0.05 % abgeschnitten, Farben bleiben) | Tonwerte automatisch |
| Drehen & Spiegeln | Horizont und Kanten waagerecht | Geraderichten automatisch |
| Trapezkorrektur | Drehung und stürzende Linien (Umschalt+A) | Perspektive automatisch |
| Objektivkorrektur | Lensfun-Profil des Objektivs | Objektivkorrektur automatisch |
| Schärfen | Radius nach der Unschärfe des Objektivs, Stärke, Schwelle und Details nach dem Rauschen | Schärfen automatisch |
| Rauschreduzierung | Luminanz und Farbe nach dem Rauschen (weniger, wo KI-Entrauschen schon wirkt) | Rauschreduzierung automatisch |
| Verbessern | Raw-Details für rauscharme Bilder, KI-Entrauschen für verrauschte | Verbessern automatisch |

- **Schärfen, Rauschreduzierung, Verbessern:** Diese drei messen an der vollen Auflösung (das Laden
  dauert gut eine Sekunde je Bild; verkleinert würde das Rauschen weggemittelt). Das Rauschen wird in der
  Wurzel der Helligkeit gemessen, wo das Schrotrauschen des Sensors in hellen und dunklen Teilen etwa
  gleich ist: je Block von 32×32 Pixeln der Median eines Laplace-Operators, der glatte Verläufe
  aufhebt (nach Immerkær), und davon die ruhigsten Blöcke, damit Textur nicht als Rauschen zählt
  (`sk-core/src/auto_detail.rs`). An Bildern einer Sony-Vollformatkamera: etwa 0.7 bei ISO 100, 2 bei
  ISO 1000, 3 bei ISO 2500 (Tausendstel). Der Radius des Schärfens folgt der gemessenen Unschärfe des
  Objektivs (wie bei „Details“), Stärke und Details sinken, die Schwelle steigt mit dem Rauschen.
  KI-Entrauschen setzt erst ab etwa ISO 1000 ein (darunter lohnte die lange Berechnung nicht), voll
  bei doppelt so viel Rauschen; Raw-Details bleibt an, solange KI-Entrauschen unter 50 % liegt.
- **Helligkeit:** Wie hell das Bild werden soll, gibt das Kamera-JPEG vor (siehe oben). Auf dem
  Kamera-Look ändert Auto die Belichtung daher meist nur wenig und kümmert sich vor allem um Lichter,
  Schatten und Weißabgleich.
- **Tonwerte:** Die Regler werden nicht einzeln aus dem Histogramm abgeleitet, sondern gemeinsam
  optimiert. Eine kleine Fassung des Bildes läuft durch die echte Pipeline und wird bewertet:
  Helligkeit der Mitteltöne, ausgefressene und abgesoffene Flächen, Ausnutzung des Tonwertumfangs
  und Kontrast. Die Regler werden verändert, bis das Ergebnis nicht mehr besser wird, ohne einen
  davon zu übertreiben. Wie hell das Bild werden soll, gibt das in die RAW-Datei eingebettete
  Kamera-JPEG vor: Seine mittlere Helligkeit enthält die Abstimmung des Herstellers und die
  Belichtungskorrektur beim Fotografieren. Ein Wald bleibt so dunkler als ein Strandbild. Ohne
  eingebettetes JPEG (etwa bei zusammengefügten DNGs) dient die gemessene Kamerabelichtung als
  Anhalt.
- **Gesichter und Hauptmotiv:** Sind die KI-Modelle geladen (Gesichter, Motiv), zählen Gesichter
  und das Hauptmotiv mehr als der Rest. Gesichter sollen weder dunkel noch ausgefressen sein. Das ist
  ein Bereich und kein fester Zielwert, denn Hauttöne sind verschieden. Geladen wird dafür nichts:
  Ohne Modelle wiegt das ganze Bild gleich.
- **Weißabgleich:** Ausgangspunkt ist der Weißabgleich der Kamera. Korrigiert wird anhand der fast
  neutralen Stellen (Steine, Wasser, weiße Wände, Wolken), nicht nach dem Mittelwert des ganzen
  Bildes. So ziehen ein Wald oder eine Holzwand ihn nicht weg. Je weniger solche Stellen es gibt,
  desto mehr bleibt es beim Kamerawert. Warmes und kühles Licht behalten einen Teil ihrer Stimmung
  (ein Sonnenuntergang bleibt warm). Das A im Kopf von Weißabgleich (Tab „Farbe“) arbeitet genauso.

Kommandozeile (Vorher | Nachher nebeneinander): `cargo run --release -p sk-ai --example auto -- <Ordner> <raw>...`

### Trapezkorrektur und automatisches Geraderichten

Der Abschnitt **Trapezkorrektur** (Tab „Objektiv“) richtet stürzende Linien auf: mit den Reglern
**Vertikal** und **Horizontal** (je ±50, stärker verzerrt es das Bild zu sehr), mit gezogenen Linien
oder automatisch. **Stärke** (0–100 %) schwächt die Korrektur ab, etwa damit ein Gebäude oben noch
leicht zuläuft. Solange man an den Reglern zieht, liegt ein Raster über dem Bild, ebenso beim Regler „Geraderichten“.

**Linien ziehen:** legt zwei bzw. vier Linien ins Bild; im Menü daneben wählt man senkrechte Kanten,
waagerechte Kanten oder beides. Ihre Enden (oder die ganze Linie) auf Kanten ziehen, die senkrecht bzw.
waagerecht sein sollen; **Übernehmen** (Enter) berechnet Drehung und Trapezkorrektur so, dass die Linien
genau ausgerichtet sind, Esc bricht ab. Wer verschobene Linien liegen lässt und ein anderes Werkzeug wählt,
übernimmt sie ebenfalls. Eine gezogene Linie je Richtung dreht nur, zwei korrigieren auch die Perspektive.

**Automatisch:** Silberkorn sucht gerade Kanten im Bild (Linienerkennung nach dem
LSD-Prinzip) und bestimmt Drehung und Perspektive so, dass sie waagerecht bzw. senkrecht stehen.
Bewertet wird jede Lösung über die echte Geometrie-Kette, einschließlich Objektivkorrektur.

- **A** im Kopf der Trapezkorrektur (Umschalt+A, Entwickeln → „Perspektive automatisch“): Drehung und
  stürzende Linien.
- Der Knopf **Beide Richtungen** darunter korrigiert zusätzlich die horizontale Perspektive,
  etwa für eine schräg fotografierte Fassade.
- **A** im Kopf von Drehen & Spiegeln: nur Horizont und Kanten waagerecht, die Perspektive bleibt unverändert.

Alle drei wirken auf die ausgewählten Bilder.

Horizontale zählen bis 8° Abweichung, vertikale bis 15°; deutlich schräge Kanten wie Dächer
oder Wege bleiben unberücksichtigt. Die Perspektive wird nur korrigiert, wenn mehrere passende Kanten über
das Bild verteilt sind. Das Ergebnis ist ein
Rückgängig-Schritt und lässt sich mit den Reglern nachjustieren.

Wie beim Geraderichten zoomt die Perspektivkorrektur automatisch gerade so weit hinein, dass der ganze
Rahmen im Bild liegt: Es entstehen keine leeren Ränder, und ein eigener Zuschnitt liegt immer innerhalb.

### Bild auf dem Zweitmonitor

**Ansicht → „Bild in eigenem Fenster“** (Umschalt+F11) öffnet das Bild als eigenes Fenster,
etwa für einen zweiten Bildschirm. Das Hauptfenster zeigt dann
das Raster zum Sichten und Auswählen, die Werkzeuge bleiben links. Im Bild-Fenster
funktionieren Zoom, Vergleichsansicht, Werkzeuge und alle Tastenkürzel; **F11** schaltet dort auf
Vollbild, Esc zurück. Position, Größe und Vollbild werden gemerkt, beim nächsten Start öffnet sich
das Fenster wieder.

### Lupe

Maustaste auf dem eingepassten Bild gedrückt halten: Ein Ausschnitt unter dem Cursor erscheint in
100 % (ein Bildpixel pro Bildschirmpixel). Gezeigt werden die
Kacheln in voller Auflösung, solange sie noch berechnet werden die vergrößerte Vorschau.

### Histogramm

Über dem Bild zeigt das Histogramm die Farbe unter der Maus: je Kanal eine Linie und die Werte in
Prozent (die angezeigten sRGB-Werte). Über dem Histogramm erscheinen die Tonbereiche Schwarz, Tiefen,
Belichtung, Lichter und Weiß (0–10, 10–35, 35–65, 65–90, 90–100 %); **Ziehen** darin verstellt den
Regler des Bereichs (die ganze Breite: sein halber Bereich), auf der aktiven Ebene oder dem ganzen
Bild, ein Rückgängig-Schritt je Zug. Umgekehrt markiert ein Regler im Tab „Belichtung“ seinen Bereich,
solange die Maus darauf steht.

**Messanzeige** (Umschalt+H, Ansicht → „Messanzeige“ oder Rechtsklick darauf) zeigt an derselben
Stelle statt des Histogramms:

- **Wellenform:** für jede Spalte des Bildes, wie hell seine Pixel dort sind (unten Schwarz, oben
  Weiß), R, G und B übereinander; wo sie zusammenfallen, wird es weiß. Anders als im Histogramm
  bleibt jeder Tonwert an seiner Stelle im Bild: Der helle Himmel links steht auch links oben. Die
  Werte unter der Maus erscheinen als Linien.
- **Vektorskop:** die Farben nach Farbton (Winkel) und Sättigung (Abstand von der Mitte), auf der
  Cb/Cr-Ebene wie in der Videotechnik. Kästchen markieren Rot, Gelb, Grün, Cyan, Blau und Magenta
  bei 75 %. Die Linie zwischen Rot und Gelb ist die Hauttonlinie: Haut jeder Farbe liegt auf ihr, nur
  verschieden weit außen. Die Farbe unter der Maus ist ein weißer Punkt.

Gemessen wird das angezeigte Bild, und nur solange Wellenform oder Vektorskop zu sehen sind. Die
Wahl bleibt über Programmstarts.

### Ansicht: Bildinfo, Hintergrund, Weißabgleich-Pipette

- **Bildinfo** (I, Ansicht → „Bildinfo …“): oben links über dem Bild Dateiname und Belichtung (Zeit,
  Blende, ISO, Brennweite); ein zweites I zeigt dazu Kamera, Objektiv, Aufnahmedatum und Größe, ein
  drittes blendet aus. Bleibt über Programmstarts.
- **Hintergrund** (Rechtsklick neben das Bild oder Ansicht → „Hintergrund“): Dunkelgrau (Standard, im
  Vollbild schwarz), Schwarz, Mittelgrau (sRGB 119, 18 %) oder Weiß. Die Umgebung bestimmt mit, wie
  hell ein Bild wirkt; vor dem Drucken sind Mittelgrau oder Weiß ehrlicher. Die Druckvorschau zeigt
  weiter das Papier.
- **Weißabgleich-Pipette** (W): Beim Überfahren zeigt das Bild schon den Weißabgleich, den ein Klick an
  dieser Stelle ergäbe, und neben dem Cursor eine Lupe der Stelle (der Rahmen ist der gemittelte
  Bereich) mit Kelvin und Tönung – oder warum es dort nicht geht (zu dunkel, ausgebrannt).

### Zielkorrektur

Das Fadenkreuz neben den Kanälen der **Gradationskurve** bzw. neben „Farbton / Sättigung /
Helligkeit“ im **Farbeditor** schaltet sie ein (Esc beendet): Ziehen im Bild nach oben oder unten
ändert, was dort liegt, wo der Zug begann – die Kurve beim Ton der Stelle (ein Punkt in der Nähe wird
genommen, sonst einer gesetzt) bzw. Farbton, Sättigung oder Helligkeit der Farbbereiche nach dem Anteil,
den sie am Farbton der Stelle haben (`sk-app/src/target_tool.rs`). Auf der aktiven Ebene oder dem
ganzen Bild.

### Verlauf und Schnappschüsse

Der Tab **„Verlauf“** listet alle Bearbeitungsschritte des Bildes, benannt nach dem, was sich
geändert hat („Belichtung 0,8“, „Perspektive, Geraderichten“). Ein Klick springt zu diesem Stand,
spätere Schritte bleiben erreichbar, bis eine neue Änderung sie verwirft. Der Verlauf gilt, solange
das Bild geöffnet ist. Überfahren eines Schritts oder Schnappschusses zeigt ihn im Bild, ohne ihn
anzuwenden.

**Schnappschüsse** sichern einen Stand unter einem Namen, dauerhaft im Sidecar (je Bild bzw.
Variante). Sie erscheinen wie die Stile als kleine Vorschauen des Bildes, jede so, wie der Stand
gespeichert wurde (ein eigener Hintergrund-Thread rechnet sie); mit „Ansicht: Liste“ bei den Stilen
als Liste. Klick wendet ihn an (ein Rückgängig-Schritt), Doppelklick benennt um, Überfahren zeigt ihn
im Bild.

### Umbenennen

**Foto → „Umbenennen …“** (Strg+Umschalt+R) benennt die ausgewählten Dateien nach einer Vorlage um,
mit Vorschau. Platzhalter: `{Name}`, `{Zähler}` (`{Zähler:3}` dreistellig, Startwert wählbar),
`{Datum}`, `{Zeit}`, `{Jahr}`, `{Monat}`, `{Tag}`, `{Kamera}`, `{Objektiv}`, `{Session}`,
`{Variante}`. Sidecar, Varianten, Schnappschüsse, Katalogeinträge und Vorschaubilder wandern mit;
ist ein Name vergeben, wird „-2“ usw. angehängt. Dieselben Platzhalter gibt es im Ausgaberezept
als **Dateiname** der verarbeiteten Bilder (der Zähler zählt dort die Bilder eines Durchgangs).

### Hilfslinien

O wechselt (oder Ansicht → „Hilfslinien“): Drittel, goldener Schnitt, Raster mit annähernd
quadratischen Feldern, Diagonalen.

### Wasserzeichen und Metadaten (Ausgaberezept)

- **Wasserzeichen:** Text (weiß oder schwarz) oder Logo (PNG mit Transparenz), in neun Positionen,
  Größe und Randabstand in % der kurzen Bildkante, Deckkraft einstellbar. Die Ausgabevorschau (P)
  zeigt es so, wie es geschrieben wird.
- **Größe:** Originalgröße oder lange Kante, kurze Kante, Breite, Höhe (in Pixeln) bzw. Prozent;
  vergrößert wird nie. Alte Rezepte mit „lange Kante“ bleiben gültig.
- **Ausgabeschärfung:** Aus, „Für Bildschirm“ (dezent) oder „Für Druck“ (kräftiger): eine
  Unschärfemaske auf der Helligkeit nach dem Verkleinern, das Bilder weicher macht; unabhängig vom
  Schärfen in der Entwicklung. Die Ausgabevorschau zeigt sie.
- **Formate:** JPEG, TIFF (8/16 Bit), PNG, WebP (verlustbehaftet, libwebp) und AVIF (10 Bit,
  4:4:4, rav1e). AVIF kennzeichnet den Farbraum ohne ICC-Profil und gibt es deshalb nur in sRGB
  und Display P3; ein 24-MP-Bild braucht rund 12 s. Die Ausgabevorschau zeigt bei WebP die
  komprimierten Pixel, bei AVIF die Pixel vor der Kompression (mit der echten Dateigröße).
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

### Fisheye entzerren

Der Regler **Fisheye** (Tab „Objektiv“, Objektivkorrektur) rechnet ein Fisheye-Bild in ein normales
(rektilineares) Weitwinkelbild um: Gerade Linien, die das Fisheye krümmt, werden wieder gerade. Der Wert
ist der Bildwinkel des Fisheyes über die Diagonale – bei den meisten Vollformat-Fisheyes 180°, bei
Zirkular-Fisheyes eher 220°. Angenommen wird die übliche winkeltreue (äquidistante) Abbildung. Ein
rektilineares Bild kann nicht so weit reichen wie ein Fisheye; Silberkorn zoomt so weit hinein, dass
keine leeren Ränder entstehen (bis 80° von der Bildmitte). Geraderichten und Perspektive wirken danach
auf das entzerrte Bild.

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

**Farbsaum Lila/Grün** (Defringe) behandelt die longitudinale chromatische Aberration: lila und grüne
Säume an kontrastreichen Kanten, die auch in der Bildmitte auftreten und von keinem Profil erfasst
werden (typisch bei offener Blende, Makros und Gegenlicht). Wo die Helligkeit in der Umgebung stark
schwankt (Standardabweichung der wahrnehmungsgleichen Helligkeit in rund 3 Sensorpixeln), werden
Pixel mit lila bis blauem bzw. grünem Farbton zur Helligkeit hin entfärbt; Flächen gleicher Farbe
bleiben unberührt. Der Grünregler trifft auch Blattränder, deshalb sparsam einsetzen.

### Objektivprofile

Lensfun-XML-Dateien werden aus `~/.local/share/silberkorn/lensfun`, `~/.local/share/lensfun` und
`/usr/share/lensfun` gelesen – beim Programmstart im Hintergrund (rund 1100 Objektive in etwa
35 ms). Fehlt die Datenbank oder ist Silberkorns Kopie älter als 30 Tage, wird sie dabei von GitHub
geladen (lensfun, Lizenz CC BY-SA 3.0, etwa 4 MB; sie wird nicht mit Silberkorn ausgeliefert).
Ohne Netz bleibt es still bei der vorhandenen Kopie.

Ein neues, noch unbearbeitetes Bild bekommt das passende Profil automatisch,
wenn die Raw-Datei keine eigene Korrektur mitbringt; die Einstellung wird gleich gespeichert, damit
Export und Vorschaubilder sie auch sehen. Bereits bearbeitete Bilder bleiben unverändert. Im Tab
„Objektiv“ lässt sich das Profil entfernen und wieder anwenden, über das A im Kopf auch für alle
ausgewählten Bilder; „Zurücksetzen“ stellt den Zustand
eines neuen Bildes her. In der Oberfläche heißt es nur „Profil“; die Quelle steht im Info-Fenster
(„Über Silberkorn“).

Einstellungen werden nicht-destruktiv als `<Bild>.sk.json` neben dem RAW gespeichert.
Sidecars, XMP-Dateien, Stile und Einstellungen werden atomar geschrieben (erst vollständig in eine
eigene temporäre Datei, dann ersetzt), sodass ein Absturz oder eine volle Platte nie eine halbe
Datei hinterlässt. Ein Sidecar oder Katalog, den eine neuere Silberkorn-Version geschrieben hat,
wird gelesen, aber nicht überschrieben.

### Prozessversionen

Jede Bearbeitung merkt sich die Version des Renderers
(`process` in den Anpassungen). Bearbeitungen aus älteren Versionen sehen deshalb unverändert aus,
neue und zurückgesetzte Bilder nutzen die aktuelle. **Entwickeln → „Auf aktuelle Prozessversion heben“**
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
- **4:** Teilweise ausgebrannte Lichter werden rekonstruiert (`sk-core/src/highlights.rs`): Ist nur
  ein Kanal oder sind zwei übersteuert, schätzt Silberkorn sie aus den heilen Kanälen und dem
  Farbanteil der hellen, nicht übersteuerten Umgebung (auf einem groben Raster gesammelt und per
  Push-Pull in die ausgebrannten Stellen verteilt). Ein heller Himmel, dessen Grün übersteuert
  ist, bleibt so blau statt cyan; die Helligkeit steigt weiter an (Werte über 1, die der
  Lichter-Regler zurückholen kann). Nur wo alle Kanäle voll sind, wird wie bisher Weiß daraus.
  Etwa 0.1 s für 24 MP; immer aktiv, ohne Regler.
- **5:** Schärfen weichzeichnet für die Unschärfemaske mit einer echten Gauß-Kurve des Radius
  statt mit drei Kastenfiltern, die unterhalb von etwa 1.4 jeden Radius gleich machten: Der
  Radius wirkt jetzt über den ganzen Bereich, und wird er kleiner als ein Pixel der Darstellung
  (in der eingepassten Ansicht unterhalb von etwa 0.8), lässt die Wirkung allmählich nach, statt
  schlagartig zu verschwinden.

Unabhängig von der Prozessversion werden **Hotpixel** entfernt: Ein Fotosit des Bayer-Mosaiks,
das mehr als dreimal so hell ist wie alle acht gleichfarbigen Nachbarn und alle acht direkt
darum, bekommt den Median der gleichfarbigen Nachbarn (gleich nach dem Skalieren der RAW-Daten,
also auch für Vorschau, Raw-Details und KI-Entrauschen). Echte Lichtpunkte treffen durch das
Objektiv immer mehrere Fotosites und bleiben; in den Beispielfotos wird kein einziges ersetzt.

### Varianten

Ein RAW kann mehrere Bearbeitungen
haben, z. B. Farbe und Schwarzweiß. **F2** legt eine neue Variante mit Standardeinstellungen an,
**F3** klont die aktuelle; beides auch im Menü „Foto“ und per Rechtsklick auf ein Vorschaubild.
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
  Selects/                    „Selects“ – die ausgewählten Bilder
  Output/                     „Ausgabe“ – verarbeitete Dateien
  Trash/                      „Papierkorb“ – bis er geleert wird
  Silberkorn/Cache/           Vorschaubilder der Session
```

- **Datei → Neue Session** (Strg+N) legt den Ordner samt Unterordnern an, **Session öffnen**
  (Strg+Umschalt+O) öffnet eine `.sksession`-Datei; zuletzt verwendete Sessions stehen im
  Menü, und die zuletzt offene Session wird beim Start wieder geöffnet. Eine Session lässt sich
  auch als Programmargument übergeben (`silberkorn "Hochzeit Müller"`).
- **Importieren** kopiert RAWs samt Sidecars in „Aufnahmen“ (die Originale bleiben liegen).
- **Strg+J** verschiebt die Auswahl nach „Selects“, **Strg+Entf** (im Raster auch Entf) in den Papierkorb; per
  Rechtsklick auf ein Vorschaubild oder im Menü „Foto“ geht es auch nach „Aufnahmen“, in
  einen Favoriten oder einen beliebigen Ordner. Sidecars wandern mit, bei Namensgleichheit
  wird `-2`, `-3` … angehängt.
- **Session-Favoriten:** häufig genutzte Ordner, gespeichert in der Session.
- **Exportieren** schreibt relative Ausgabeordner in den Session-Ordner, das Standardrezept
  also nach `Output/`.
- **Papierkorb leeren** löscht die Bilder darin endgültig (mit Rückfrage).
- **Ordner überwachen** (in der Session-Spalte, für Ordner ohne Session unter „Ordner“, auch im Menü
  „Datei“): Neue Bilder im angezeigten Ordner, etwa aus der Kamerasoftware in „Aufnahmen“,
  erscheinen von selbst in der Übersicht; mit **Neue Aufnahme gleich öffnen** wird die neueste
  geöffnet. Gesehen wird einmal pro Sekunde, und nur, wenn sich im Ordner etwas geändert hat; eine
  Datei gilt als fertig, wenn ihre Größe zwischen zwei Blicken gleich bleibt, und die Übersicht wird
  erst aktualisiert, wenn keine Datei mehr wächst. Die Statusleiste zeigt „Überwacht: <Ordner>“.
  Einstellungen des aktuellen Bildes werden nicht übertragen.
- **Session schließen** kehrt zum Katalog und zum zuvor geöffneten Ordner zurück.
- **Portabel:** Die Session-Datenbank speichert Pfade relativ zum Session-Ordner. Eine Session
  lässt sich daher als Ganzes verschieben, umbenennen oder auf einen anderen Rechner kopieren
  (auch zwischen Windows und Linux), ohne dass Alben, Favoriten oder der Index verloren gehen.
  Nur Favoriten außerhalb des Session-Ordners bleiben absolute Pfade.
- **Vorschaubilder** einer Session liegen in `Silberkorn/Cache/Thumbnails` im Session-Ordner
  und wandern mit; Bilder außerhalb von Sessions nutzen den Cache unter `~/.cache/silberkorn`.
  Ebenso die bildschirmgroßen **Vorschauen** der Einzelbildansicht (`Silberkorn/Cache/Previews` bzw.
  `~/.cache/silberkorn/previews`): Ein schon einmal angesehenes Bild erscheint beim Durchblättern
  sofort, die Nachbarbilder werden im Hintergrund vorbereitet. Fehlende Vorschauen eines Ordners
  oder einer Session entstehen im Hintergrund, sobald die App eine Weile nichts zu tun hat
  (Fortschritt rechts in der Statusleiste); die Thumbnails zeigen dann ebenfalls die eigene
  Entwicklung statt des Kamera-JPEGs.

Alben, Filteralben, Keywords und Suche funktionieren in einer Session genauso wie im Katalog,
beziehen sich aber nur auf die Bilder der Session.

### Katalog

- **Ordner** werden wie eine Session direkt durchsucht und im Hintergrund indexiert.
- **Katalog** (SQLite, `~/.local/share/silberkorn/catalog.sqlite`, überschreibbar mit
  `SILBERKORN_CATALOG`): „Ordner importieren“ nimmt einen Ordnerbaum auf; danach stehen
  „Alle Bilder“, „Letzter Import“, Katalogordner und Alben zur Verfügung.
- **Alben** (manuell) und **Filteralben** (gespeicherter Filter), Rechtsklick für Umbenennen/Löschen.
- **Filter & Suche:** Freitext (Name, Kamera, Objektiv, Keyword), Mindestbewertung, Farb-Label,
  Keyword; Sortierung nach Name, Aufnahmedatum oder Bewertung.
- **Keywords** im Tab „Metadaten“ (mit Vorschlägen aus dem Katalog); Bewertung, Label und
  Keywords wirken auf die ganze Auswahl.
- **Stile:** Anpassungen als Stil speichern (`~/.config/silberkorn/styles`) und auf die Auswahl anwenden.
  Der Tab „Stile“ zeigt Filmsimulationen und eigene Stile als **Vorschauen**: jeder Stil auf das
  aktuelle Bild angewendet, im Hintergrund klein gerechnet und nach jeder abgeschlossenen Änderung neu.
  Klick wendet ihn auf die Auswahl an, der Zeiger zeigt ihn groß im Bild,
  Rechtsklick löscht einen eigenen Stil. „Liste“ schaltet auf die knappe Ansicht um (wird gemerkt).
  Beim Speichern („Enthält: …“) wählt man die Werkzeuge; vorgewählt sind die bearbeiteten. Ein Stil
  setzt nur seine Werkzeuge, alle anderen bleiben, wie sie sind, so lassen sich Stile übereinander
  anwenden (etwa ein Farblook und danach ein Körnungs-Stil). Stile von vor dieser Funktion setzen
  weiterhin alles außer Zuschnitt, Drehen, Perspektive, Ebenen und Retusche.
  Ist eine **Ebene gewählt**, geht ein Stil oder Look auf diese Ebene (nur im aktuellen Bild, auch
  die Vorschau beim Überfahren): als Änderung des Hintergrunds, deren Stärke die Deckkraft der Ebene
  ist und die ihre Maske eingrenzt. Schwarzweiß wird dort zu Sättigung −100 mit der
  Farbempfindlichkeit als Helligkeit je Farbband und der Tonung als Farbbalance; eine andere
  Basiskurve als „Film Standard“ wird zu einer Gradationskurve, die denselben Unterschied macht.
  So sieht jeder der elf Filmlooks auf einer Ebene über das ganze Bild aus wie auf dem Bild selbst,
  bis auf die Körnung (neun gleich, Velvia und Tri-X im Mittel unter drei von 255 Stufen anders). Was
  eine Ebene nicht kann (Körnung, Tonwerte, Weißabgleich, eigene Farbbereiche …), nennt die
  Statusleiste; das bleibt dem Hintergrund vorbehalten, wie in Capture One.
- **Eingebettete Vorschauen** werden direkt gelesen: nur die TIFF-Struktur und das JPEG, nicht die
  ganze RAW-Datei (ARW, NEF, CR2, DNG, ORF, RW2, PEF, RAF). Über Netzlaufwerke oder USB ein
  Vielfaches schneller; andere Formate (z. B. CR3) wie bisher über die volle Datei.
- **Thumbnail-Cache** unter `~/.cache/silberkorn/thumbs` (Vorschauen in `previews`). Zusammen
  höchstens 4 GB (`SILBERKORN_CACHE_MB` ändert das); beim Start werden die am längsten nicht
  genutzten entfernt. Session-Caches sind davon ausgenommen. Sichtbare Vorschaubilder werden vor
  denen berechnet, an denen man schon vorbeigescrollt ist.
- **Speicherplatz** (Bearbeiten → „Speicherplatz …“): zeigt die Zwischenspeicher auf der Festplatte
  mit Füllstand – Vorschauen und Miniaturen (4 GB), Tiefenkarten (2 GB), Raw-Details (10 GB), KI-Entrauschen (20 GB) –
  und leert sie einzeln. Alles darin wird bei Bedarf neu berechnet.

Sidecars bleiben die maßgebliche Quelle; der Katalog ist ein Index und kann jederzeit neu aufgebaut werden.

### Stapel

Zusammengehörige Bilder (Serie, Belichtungsreihe, Varianten eines Motivs) zeigt
der Browser als ein Vorschaubild, das oberste Bild des Stapels. Die Zahl oben links nennt die
Bilder im Stapel; ein Klick darauf (oder **Umschalt+G**) klappt ihn auf und wieder zu.

- **Strg+G** bildet aus der Auswahl einen Stapel, das angezeigte Bild wird das oberste.
  **Strg+Umschalt+G** löst die Stapel der Auswahl auf. Im Menü **Foto → Stapel** (auch per
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
  nächsten Laden. Gilt die eigene Änderung als neuer, bleibt sie. **Foto → XMP → „Metadaten aus
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

Die Tabelle zeigt die Voreinstellung. Im Fenster **Hilfe → Tastenkürzel** (F1) lässt sich jedem Befehl
ein anderes oder ein weiteres Kürzel geben: Klick auf ein Kürzel und die neue Taste drücken, „+“
fügt eines hinzu, Rechtsklick entfernt es. Ist die Taste schon vergeben, wechselt sie zum neuen
Befehl (das Fenster sagt, von welchem). Menüs, Befehlssuche und Tooltips zeigen die geänderten
Kürzel; gespeichert werden sie in `settings.json`. Fest bleiben Strg+K, Strg+1 … Strg+0, Esc,
die Leertaste und die Tasten der Schnellbearbeitung.

| Taste | Funktion |
|---|---|
| Strg+K | Befehlspalette: jede Funktion, jedes Werkzeug und jeden Tool-Abschnitt per Suche finden (auch über das Suchfeld in der Titelleiste) |
| Strg+1 … Strg+0 | Werkzeug-Tabs in der Reihenfolge der Tab-Leiste (außer „Verlauf“; „Ausgabe“ hat keins mehr) |
| Strg+Mausrad | Vorschaugröße im Raster (auch Regler in der Statusleiste) |
| ← / → | vorheriges / nächstes Bild |
| 0–5 | Bewertung |
| 6 / 7 / 8 / 9 | Farb-Label Rot / Gelb / Grün / Blau |
| G | Raster ↔ Einzelbild |
| Strg+Z / Strg+Y | Rückgängig / Wiederholen |
| Strg+Umschalt+C / V | Anpassungen kopieren / einfügen |
| Strg+D | Auswahl exportieren |
| Strg+N / Strg+Umschalt+O | neue Session / Session öffnen |
| Strg+J / Strg+Entf | nach „Selects“ / in den Papierkorb verschieben (Session; im Raster auch Entf) |
| Strg+A | alle auswählen · Strg/Umschalt+Klick, Umschalt+Pfeiltaste: Mehrfachauswahl |
| Strg+[ / Strg+] | drehen |
| Y | Vorher/Nachher |
| E | Belichtungswarnung |
| I | Bildinfo über dem Bild: kurz / ausführlich / aus |
| Umschalt+P | Softproof |
| W | Weißabgleich-Pipette |
| Z / Doppelklick | Einpassen ↔ 100 % · Mausrad: zoomen · Ziehen: verschieben (höchstens bis zum Bildrand, das Bild bleibt im Blick) |
| C / Enter / Esc | Zuschnitt-Werkzeug / übernehmen / abbrechen · beim Ziehen Alt: von der Mitte aus, Umschalt: Seitenverhältnis beibehalten (wenn frei) |
| Esc | beendet der Reihe nach Werkzeug, Ansicht (Vorher/Nachher, Vergleich, Ausgabevorschau, 100 %) und Vollbild |
| B / N / L / R | Pinsel / Radierer / linearer / radialer Verlauf |
| H / S | Reparieren / Klonen · Entf: Punkt löschen · Umschalt/Alt beim Malen: gewählten Bereich erweitern/verkleinern |
| X | Entfernen (KI-Funktion) |
| M | Maske anzeigen · [ / ]: Pinselgröße · rechte Maustaste ziehen: Größe (links/rechts) und Härte (hoch/runter) · mittlere Maustaste: verschieben · Alt: radieren |
| J | Zauberpinsel |
| U / Umschalt+U | Vorher/Nachher geteilt / nebeneinander |
| O | Hilfslinien wechseln |
| Umschalt+A | automatisch geraderichten |
| Strg+Umschalt+R | Auswahl umbenennen |
| Umschalt+F11 | Bild in eigenem Fenster (Zweitmonitor) |
| Maustaste halten | Lupe (100 %) im eingepassten Bild |
| Umschalt+Enter | Referenzbild festlegen / entfernen |
| K | Schärfemaske |
| F2 / F3 | neue Variante / Variante klonen |
| Strg+G / Strg+Umschalt+G | Stapel bilden / auflösen |
| Umschalt+G | Stapel auf-/zuklappen |
| Alt+↑ / Alt+↓ | Ebene darüber / darunter bearbeiten (zuletzt der Hintergrund) |
| Q / A / D / F / V / T + Mausrad | Schnellbearbeitung: Belichtung / Kontrast / Sättigung / Lichter / Tiefen / Temperatur (auf der aktiven Ebene, sonst global) |

Doppelklick auf einen Reglernamen setzt den Regler zurück. Nach einem Klick auf einen Regler ändern
ihn die Pfeiltasten und das Mausrad (Umschalt: grob, Alt: fein); ohne diesen Klick scrollt das
Mausrad nur das Panel. Ein Zug am Regler ist ein Rückgängig-Schritt, ebenso mehrere Pfeiltasten-
oder Mausrad-Schritte kurz hintereinander (auch bei der Schnellbearbeitung).

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

## Lizenz

Silberkorn steht unter der [MIT-Lizenz](LICENSE). Eingebaut ist rawler (dnglab) zum Lesen der RAW-Dateien,
unter LGPL-2.1: Wer die `.exe` weitergibt, gibt mit diesem Repository auch den Weg, rawler auszutauschen und
neu zu bauen. Die KI-Modelle (darunter RawNIND unter GPL-3.0) und die lensfun-Daten (CC BY-SA 3.0) werden nicht
mitgeliefert, sondern beim Start heruntergeladen; ihre Lizenzen stehen unter Hilfe → „Lizenzen …“.

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
- **Phase 6 (fertig):** Mehrfachansicht mit Referenzbild und synchronem Zoom, Look angleichen (Referenzbild oder Bilddatei), Hauttöne vereinheitlichen
- **Phase 7 (fertig):** Varianten (virtuelle Kopien), Schärfemaske
- **Phase 8 (fertig):** Vorher/Nachher nebeneinander, Wasserzeichen (Text/Logo), EXIF und Copyright im Export, Hilfslinien
- **Phase 9 (fertig):** automatisches Geraderichten (Drehung, vertikaler/horizontaler Keystone)
- **Phase 10 (fertig):** Lupe, Verlauf und Schnappschüsse, Umbenennen mit Platzhaltern (auch im Ausgaberezept)
- **Phase 11 (fertig):** XMP-Austausch, Stapel, Viewer auf dem Zweitmonitor, Softproof, HDR und Panorama, KI-Entrauschen, Entfernen, Gesichter und Sichtung
- **Phase 12 (fertig):** Anpassungen werkzeugweise kopieren und übertragen, Stile mit Werkzeugauswahl (übereinander anwendbar), eigene Tastenkürzel, eigene Farbbereiche im Farbeditor, Luma-Kurve
- **Phase 13 (fertig):** Neu ausleuchten und Atmosphäre über die Tiefenkarte, Leuchten (Orton), Stile als Vorschau-Raster, Focus Stacking
- **Phase 14 (fertig):** Motiv auswählen mit BiRefNet (genaue Kanten)
