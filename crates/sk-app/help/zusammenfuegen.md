# Zusammenfügen

Aus mehreren Aufnahmen entsteht ein neues Bild: mehr Dynamikumfang (HDR), ein weiterer Bildwinkel (Panorama), durchgehende Schärfe (Focus Stacking) oder eine lange Belichtung aus einer Serie.

## So geht es

1. Mindestens zwei Bilder auswählen, oder einen zugeklappten Stapel: Er bringt alle seine Aufnahmen mit.
2. Im Menü **Entwickeln → Zusammenfügen** das Verfahren wählen: **HDR …**, **Panorama …**, **Focus Stacking …** oder **Langzeitbelichtung …**. Die Befehle stehen auch in der Befehlssuche (Strg+K).
3. Im Dialog die Optionen einstellen und **Zusammenfügen** drücken.

Das Zusammenfügen läuft im Hintergrund, der Fortschritt steht in der Statusleiste, ✕ bricht ab. Man kann währenddessen weiterarbeiten. Das Ergebnis wird danach ausgewählt.

## Das Ergebnis

Es entsteht ein neues DNG neben den Originalen, benannt nach dem ersten Bild mit einer Endung wie `-HDR.dng`. Die Originale bleiben unverändert.

Das DNG enthält die Daten noch unentwickelt: Weißabgleich, Farbe und Tonwerte bleiben voll bearbeitbar wie bei einer RAW-Datei. Auch Lightroom und Capture One öffnen es.

> Am besten fügt man unbearbeitete oder nur leicht bearbeitete Aufnahmen zusammen und entwickelt danach das fertige DNG.

## HDR

Fügt eine Belichtungsreihe zu einem Bild mit dem Dynamikumfang aller Aufnahmen zusammen: helle Lichter aus der knappen, dunkle Schatten aus der reichlichen Belichtung. Ergebnis: `…-HDR.dng`.

- **Automatisch ausrichten (aus der Hand fotografiert)**: gleicht kleine Verschiebungen zwischen den Aufnahmen aus. Vom Stativ nicht nötig.
- **Geister entfernen** (**Aus**, **Niedrig**, **Mittel**, **Hoch**): Bewegte Motive wie Menschen, Äste oder Wolken werden aus der mittleren Belichtung übernommen, damit sie nicht mehrfach durchscheinend erscheinen.

Das Ergebnis wirkt zunächst oft flau oder zu hell: Die Lichter sind vollständig erhalten und lassen sich mit **Lichter** und **Weiß** im Tab **Belichtung** zurückholen. **Automatisch anpassen** ist ein guter Anfang.

## Panorama

Fügt überlappende Aufnahmen zu einem Panorama zusammen; Reihenfolge und Ausrichtung ergeben sich aus dem Bildinhalt. Ergebnis: `…-Pano.dng`. Die Aufnahmen sollten sich um etwa ein Drittel überlappen.

- **Projektion**: **Sphärisch** für weite und mehrreihige Panoramen, **Zylindrisch** für eine Reihe, **Perspektivisch** für Architektur (gerade Linien, bis etwa 120°).
- **Rand automatisch beschneiden**: schneidet auf das größte Rechteck ohne leere Ränder zu.
- **Objektivkorrektur der Einzelbilder anwenden**: korrigiert Verzeichnung und Vignettierung jeder Aufnahme vor dem Zusammenfügen, wie sie in ihrem Tab **Objektiv** eingestellt ist.

## Focus Stacking

Fügt eine Reihe mit wandernder Schärfeebene zu einem Bild zusammen, das überall scharf ist, etwa für Makro- oder Produktfotos. Von jeder Stelle wird die schärfste Aufnahme genommen. Ergebnis: `…-Fokus.dng`.

Verwacklung aus der Hand und die Größenänderung beim Fokussieren werden ausgeglichen; Optionen gibt es keine. Die Zahl der Aufnahmen ist nicht begrenzt.

## Langzeitbelichtung

Fügt eine Serie derselben Szene zu einem Bild zusammen, als wäre lange belichtet worden, ohne ND-Filter. Unter **Verfahren**:

- **Mittelwert**: wie eine lange Belichtung. Wasser und Wolken werden weich, das Rauschen sinkt. Ergebnis: `…-Langzeit.dng`.
- **Median**: Was nur kurz durchs Bild läuft (Passanten, Autos), verschwindet; übrig bleibt, was in den meisten Aufnahmen gleich aussieht. Ergebnis: `…-Median.dng`.
- **Hellste**: Lichtspuren, Sternspuren, Feuerwerk. Ergebnis: `…-Lichtspuren.dng`.

**Automatisch ausrichten (aus der Hand fotografiert)** gleicht Verschiebungen aus. Bei **Hellste** vom Stativ ausschalten: sonst werden womöglich die Sterne übereinandergelegt, und die Spuren fehlen. Hat die Kamera die Aufnahmen unterschiedlich belichtet, werden sie vorher angeglichen.

> Für den **Median** braucht Silberkorn alle Aufnahmen zugleich im Arbeitsspeicher. Reicht er nicht, nennt der Dialog, wie viele Aufnahmen gleichmäßig über die Serie verteilt verwendet werden.

## Super Resolution

Ähnlich arbeitet **Super Resolution …** im Menü **Entwickeln**: Es verdoppelt Breite und Höhe eines einzelnen Bildes und legt ebenfalls ein neues DNG (`…-SR.dng`) daneben. Beschrieben ist es im Kapitel [KI-Werkzeuge](ki-werkzeuge).
