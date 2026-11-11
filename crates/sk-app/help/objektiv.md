# Objektiv & Geometrie

Der Tab **Objektiv & Geometrie** enthält alles, was den Bildausschnitt und die Form des Bildes verändert: zuschneiden, drehen, Objektivfehler korrigieren und stürzende Linien aufrichten. Diese Einstellungen gehören zum Inhalt eines Bildes: Beim Kopieren von Anpassungen sind sie nur dabei, wenn man sie ausdrücklich wählt.

Auf einer Ebene ist der Tab deaktiviert; er wirkt immer auf das ganze Bild.

## Zuschnitt

Legt den Bildausschnitt fest.

1. **Zuschneiden** wählen ({taste:crop}). Über dem Bild erscheint ein Rahmen, unten im Bild eine Leiste mit den Einstellungen.
2. Rahmen an Ecken oder Kanten ziehen; Ziehen im Rahmen verschiebt ihn, Ziehen außerhalb zieht einen neuen auf.
3. **Übernehmen** ({taste:finish-crop}) oder **Abbrechen** (Esc).

Beim Ziehen:

- Alt: von der Mitte aus.
- Umschalt: Seitenverhältnis beibehalten, wenn es frei ist.

### Seitenverhältnis

- **Original**: das Seitenverhältnis der Aufnahme, in ihrer Ausrichtung. Standard.
- **Frei**: beliebig.
- **1:1**, **3:2**, **4:3**, **5:4**, **7:5**, **16:9** und **Panorama 65:24**.

### Ausrichtung

Die zwei Knöpfe wählen **Querformat** oder **Hochformat**; {taste:crop-portrait} wechselt zwischen beiden. Bei **Frei**, **Original** und **1:1** gibt es keine Ausrichtung, die Knöpfe sind dann ausgegraut.

Seitenverhältnis und Ausrichtung stehen sowohl im Abschnitt als auch in der Leiste unten im Bild, dazu die Größe des Ausschnitts in Prozent des Bildes. Am Anfasser links lässt sich die Leiste verschieben: oben oder unten, jeweils links, mittig oder rechts. Dort bleibt sie, auch für die anderen Werkzeuge.

> ↺ im Kopf setzt den Zuschnitt samt Seitenverhältnis zurück.

## Drehen & Spiegeln

- **Geraderichten**: dreht das Bild um bis zu 45°, etwa für einen schiefen Horizont. Solange am Regler gezogen wird, liegt ein Raster über dem Bild.
- **Linie** (erster Knopf der Leiste darunter): im Bild an einem Horizont oder einer Kante entlangziehen, die waagerecht oder senkrecht sein soll; das Bild wird danach ausgerichtet.
- Die zwei Pfeile drehen um 90° nach links ({taste:rotate-left}) bzw. rechts ({taste:rotate-right}).
- Die letzten zwei Knöpfe spiegeln das Bild horizontal bzw. vertikal; ein gespiegeltes Bild zeigt den Knopf hervorgehoben.

Das **A** im Kopf richtet Horizont und Kanten automatisch waagerecht aus, auch für alle ausgewählten Bilder; die Perspektive bleibt dabei unverändert. Beim Geraderichten wird das Bild gerade so weit vergrößert, dass keine leeren Ecken entstehen.

## Objektivkorrektur

Korrigiert Fehler des Objektivs: Verzeichnung, Randabschattung und Farbsäume. Oben steht das Objektiv aus den Aufnahmedaten, mit Brennweite und der Quelle der Korrektur.

### Korrektur

Woher die Korrektur kommt:

- **Kamera**: die Korrektur, die die Kamera für das angesetzte Objektiv in der RAW-Datei gespeichert hat. Neue Bilder bekommen sie automatisch, wenn die Datei sie enthält.
- **Lokales Profil**: ein Profil aus der Objektivdatenbank, passend zu Objektiv und Brennweite. Neue Bilder bekommen es automatisch, wenn die Datei keine eigene Korrektur mitbringt.
- **Keine**: ohne Korrektur.

Was es für das Bild nicht gibt, ist ausgegraut. Das **A** im Kopf wendet das passende Profil an, auch auf alle ausgewählten Bilder. ↺ stellt den Zustand eines neuen Bildes wieder her.

> Die Objektivdatenbank lädt Silberkorn beim Programmstart im Hintergrund aus dem Internet und hält sie aktuell. Solange sie fehlt, steht „Lade Objektivprofile …“ im Abschnitt.

### Regler

- **Verzeichnung**: gleicht tonnen- oder kissenförmig gebogene Linien aus, zusätzlich zur Korrektur oben.
- **Randabschattung**: hellt dunkle Bildecken auf oder dunkelt sie ab.

Seltener Gebrauchtes steht in zwei eingeklappten Bereichen darunter, **Farbfehler** und **Fisheye**. Ein Klick öffnet sie; ein blauer Punkt hinter dem Namen zeigt, dass darin etwas eingestellt ist.

### Fisheye

**Bildwinkel** rechnet ein Fisheye-Bild in ein normales Weitwinkelbild um, sodass gekrümmte Linien wieder gerade werden. Der Wert ist der Bildwinkel des Fisheyes über die Diagonale, bei den meisten Vollformat-Fisheyes 180°. Das Bild wird so weit vergrößert, dass keine leeren Ränder entstehen.

### Chromatische Aberration

Im Bereich **Farbfehler**. Farbige Ränder an Kanten zum Bildrand hin. **Rot/Cyan** und **Blau/Gelb** gleichen Rot- und Blaukanal an. Ein Profil oder die Kamerakorrektur erledigen das meist schon.

### Farbsäume

Im Bereich **Farbfehler**. Lila und grüne Säume an kontrastreichen Kanten, auch in der Bildmitte, typisch bei offener Blende und Gegenlicht. **Lila** und **Grün** entfärben sie; Flächen gleicher Farbe bleiben unberührt. Grün trifft auch Blattränder, deshalb sparsam einsetzen.

## Trapezkorrektur

Richtet stürzende Linien auf, etwa bei Gebäuden, die nach oben zusammenlaufen.

- **Vertikal**: stürzende Linien senkrecht nach oben.
- **Horizontal**: für schräg fotografierte Fassaden.
- **Stärke**: schwächt die Korrektur ab, etwa damit ein Gebäude oben noch leicht zuläuft und natürlich wirkt.

Solange an den Reglern gezogen wird, liegt ein Raster über dem Bild. Das Bild wird so weit vergrößert, dass keine leeren Ränder entstehen.

### Automatisch

- Das **A** im Kopf ({taste:auto-upright}) korrigiert Drehung und stürzende Linien.
- **Beide Richtungen** korrigiert zusätzlich die horizontale Perspektive.

Beides wirkt auf alle ausgewählten Bilder. Silberkorn sucht dafür gerade Kanten im Bild; deutlich schräge Kanten wie Dächer bleiben unberücksichtigt. Das Ergebnis lässt sich mit den Reglern nachjustieren.

### Linien ziehen

Wenn die Automatik die falschen Kanten erwischt:

1. **Linien ziehen** wählen. Unter **Linien für** festlegen, ob **Vertikale Kanten**, **Horizontale Kanten** oder beide (**Vertikal und horizontal**) ausgerichtet werden; die Auswahl dort startet auch gleich.
2. Die Linien im Bild an ihren Enden (oder als Ganzes) auf Kanten legen, die senkrecht bzw. waagerecht sein sollen.
3. **Übernehmen** ({taste:finish-crop}) berechnet Drehung und Trapezkorrektur; Esc bricht ab.

Eine Linie je Richtung dreht nur; zwei korrigieren auch die Perspektive. Wer ein anderes Werkzeug wählt, übernimmt verschobene Linien ebenfalls.
