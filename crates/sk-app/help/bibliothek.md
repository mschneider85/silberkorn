# Bibliothek

Der Tab **Bibliothek** ordnet die Bilder: woher sie kommen, welche gezeigt werden und wie sie zusammengehören. Bewertung, Farb-Label und Keywords stehen zusätzlich im Tab **Metadaten**.

## Ordner, Katalog und Sessions

Silberkorn kennt drei Arten, mit Bildern zu arbeiten. Sie schließen sich nicht aus: Ordner lassen sich immer durchsuchen, der Katalog ist immer da, eine Session ersetzt ihn nur, solange sie geöffnet ist.

### Ordner

**Ordner öffnen …** ({taste:open-folder}) oder **Durchsuchen …** im Abschnitt **Ordner** zeigt die Bilder eines Ordners. Darunter stehen seine Unterordner; **Übergeordnet** geht eine Ebene höher, **mit Unterordnern** zeigt auch alle Bilder darunter.

Ein nur durchsuchter Ordner wird nicht in den Katalog aufgenommen. Erst wenn man ein Bild bearbeitet, kommt es an seinem Ort in den Katalog. **Importieren …** im Abschnitt **Katalog** nimmt den ganzen Ordner samt Unterordnern auf.

### Katalog

Der Katalog sammelt Bilder aus beliebigen Ordnern und Laufwerken. Er bietet **Alle Bilder**, **Letzter Import**, die **Katalogordner** sowie Alben, Filter und Suche über alles. Die Einstellungen der Bilder liegen im Katalog selbst, sodass auch Bilder auf schreibgeschützten Laufwerken bearbeitet werden können. Silberkorn sichert den Katalog jede Woche.

**Bereinigen** entfernt Einträge, deren Dateien es nicht mehr gibt.

### Sessions

Eine Session ist ein abgeschlossener Projektordner, etwa für ein Shooting oder einen Kunden. Er enthält feste Unterordner:

- **Aufnahmen**: neue Bilder (Ordner `Capture`)
- **Selects**: die ausgewählten Bilder (`Selects`)
- **Ausgabe**: die exportierten Dateien (`Output`)
- **Papierkorb**: bis er geleert wird (`Trash`)

**Neue Session …** ({taste:new-session}) im Menü **Datei** fragt nach Name und Speicherort und legt den Ordner an. **Session öffnen …** ({taste:open-session}) öffnet eine `.sksession`-Datei, **Zuletzt verwendet** die letzten Sessions; die zuletzt offene öffnet sich beim Start wieder. **Session schließen** ({taste:close-session}) kehrt zum Katalog zurück.

Im Tab **Bibliothek** stehen dann oben die Ordner der Session und **Session-Favoriten** für häufig genutzte Ordner (**Aktuellen Ordner hinzufügen**). Alben, Filter und Suche wirken nur auf die Bilder der Session. Die Einstellungen liegen als Datei neben jedem Bild; die ganze Session lässt sich verschieben, umbenennen oder auf einen anderen Rechner kopieren.

> Faustregel: der Katalog für das Archiv, eine Session für ein einzelnes Projekt, das man als Ganzes abgeben oder archivieren will.

## Import

**Bilder in den Katalog importieren …** (in einer Session **Bilder in die Session importieren …**) im Menü **Datei** oder in der Werkzeugleiste fragt nach einem Quellordner und öffnet den Importdialog.

1. Der Dialog zeigt alle RAW-Dateien der Quelle als Vorschauen. **Unterordner einbeziehen** nimmt auch Unterordner dazu, **Andere Quelle …** wechselt den Ordner.
2. Ein Klick wählt ein Bild ab oder wieder an, Umschalt+Klick einen Bereich. **Alle**, **Keine** und **Nur neue** helfen bei vielen Bildern; schon vorhandene Bilder sind markiert.
3. Unter **Dateien** (nur Katalog) legt man fest, was mit den Dateien geschieht: **Nicht kopieren** (die Bilder bleiben, wo sie sind), **In den Bilderordner kopieren** (Standard: `Bilder/Silberkorn` im Benutzerordner) oder **In einen Ordner kopieren**. Beim Kopieren legt **Unterordner nach Aufnahmedatum** Ordner wie `2026/10-18` an.
4. **Beim Kopieren umbenennen** gibt den Kopien Namen nach einer Vorlage, mit denselben Platzhaltern wie beim [Umbenennen](bibliothek#Umbenennen). Die Originale bleiben, wie sie sind.
5. Der Knopf unten startet den Import.

In einer Session werden die Bilder immer in den Ordner **Aufnahmen** kopiert.

### Speicherkarte

Steckt man eine Speicherkarte ein, öffnet sich der Importdialog von selbst mit ihrem Bilderordner. Von einer Karte wird immer kopiert, auch wenn **Nicht kopieren** gewählt war: Die Dateien verschwinden ja mit der Karte. Wird die Karte herausgezogen, schließt sich der Dialog.

Das automatische Öffnen schaltet **Importdialog öffnen, wenn eine Speicherkarte erkannt wird** unten im Dialog ab.

## Bewerten und markieren

Alle drei wirken auf die ganze Auswahl und lassen sich mit {taste:undo} zurücknehmen. Sie stehen im Tab **Metadaten**, im Menü **Foto** und im Rechtsklick-Menü eines Vorschaubilds.

### Bewertung

{taste:rating-1} bis {taste:rating-5} vergeben ein bis fünf Sterne, {taste:rating-0} entfernt sie.

### Farb-Label

{taste:label-red} Rot, {taste:label-yellow} Gelb, {taste:label-green} Grün, {taste:label-blue} Blau. **Farb-Label: Keine** entfernt es.

### Keywords

Im Tab **Metadaten** unter **Keywords** ein Wort eingeben und Enter drücken; Vorschläge kommen aus den vorhandenen Keywords. ✕ an einem Keyword entfernt es.

### Aufnahme

Im Tab **Metadaten** zeigt **Aufnahme** Datei, Ordner, Kamera, Objektiv, Brennweite, Blende, Belichtungszeit, ISO und Datum des gewählten Bildes. Lange Einträge stehen beim Überfahren ganz da.

## Filter & Suche

Der Abschnitt **Filter & Suche** grenzt die Ansicht ein:

- Suchfeld: Dateiname, Kamera, Objektiv oder Keyword.
- **Mindestens**: Mindestbewertung in Sternen.
- **Farb-Label** und **Keyword**.
- Sortierung nach **Dateiname**, **Aufnahmedatum** oder **Bewertung**, daneben die Sortierrichtung.

**✕ Filter** hebt alle Filter auf. {taste:find} springt von überall in die Bibliothek.

## Alben

Ein Album sammelt Bilder unabhängig von ihrem Ordner. Namen eingeben, dann:

- **+ Album** legt ein Album mit der aktuellen Auswahl an.
- **+ Filteralbum** speichert den aktuellen Filter: Das Album zeigt immer alle Bilder, die gerade dazu passen.

Rechtsklick auf ein Album bietet **Auswahl hinzufügen**, **Umbenennen** und **Löschen …**; die Bilder selbst bleiben dabei erhalten. **Auswahl aus Album entfernen** steht unter der Liste, solange ein Album gezeigt wird.

## Stapel

Ein Stapel fasst zusammengehörige Bilder zusammen, etwa eine Serie oder Belichtungsreihe. Das Raster zeigt nur das oberste Bild; die Zahl oben links nennt die Bilder im Stapel, ein Klick darauf klappt ihn auf und zu.

- **Stapel bilden** ({taste:stack-create}): aus der Auswahl; das angezeigte Bild kommt nach oben.
- **Stapel auflösen** ({taste:stack-dissolve}), **Stapel auf-/zuklappen** ({taste:stack-toggle}).
- Im Menü **Foto → Stapel** außerdem **Aus Stapel entfernen**, **Als oberstes Bild**, **Schärfstes Bild nach oben**, **Alle Stapel aufklappen** und **Alle Stapel zuklappen**.
- **Automatisch nach Aufnahmezeit …** stapelt die Auswahl (oder alle Bilder der Ansicht): Eine Pause von mehr als der eingestellten Zeit beginnt einen neuen Stapel.

Ein zugeklappter Stapel bringt beim [Zusammenfügen](zusammenfuegen) alle seine Bilder mit.

## Varianten

Eine Variante ist eine weitere Bearbeitung derselben RAW-Datei, etwa eine Farb- und eine Schwarzweiß-Fassung. **Neue Variante** ({taste:new-variant}) beginnt mit Standardeinstellungen, **Variante klonen** ({taste:clone-variant}) übernimmt die aktuelle Bearbeitung.

Varianten erscheinen als eigene Bilder mit eigener Bewertung, eigenem Label und eigenen Keywords und lassen sich wie normale Bilder vergleichen, in Alben legen und exportieren. **Variante löschen …** im Menü **Foto** entfernt eine weitere Variante; die erste gehört zur Datei selbst.

## Gesichter & Sichtung

Der Abschnitt **Gesichter & Sichtung** hilft, aus vielen ähnlichen Bildern die guten zu finden. **Ansicht analysieren** untersucht alle noch nicht analysierten Bilder der Ansicht im Hintergrund, **Erneut** analysiert alle noch einmal. Beim ersten Mal werden etwa 40 MB geladen.

Danach filtern die Knöpfe **Augen geschlossen**, **Unscharf**, **Mit Gesichtern** und **Nicht analysiert**. Im Raster markiert ein Augen-Symbol geschlossene Augen, ein Tropfen-Symbol ein unscharfes Bild. Auch die Filter lassen sich als Filteralbum speichern.

Unter **Personen** stehen die erkannten Personen als Kacheln:

- Klick zeigt ihre Bilder, ein zweiter wieder alle. Doppelklick gibt ihr einen Namen; er wird zum Keyword ihrer Bilder.
- Wurde dieselbe Person doppelt erkannt, eine Kachel auf die andere ziehen.
- Ist eine Person gewählt, stehen darunter ihre Gesichter einzeln. Falsch zugeordnete auf die richtige Person ziehen oder auf **Neue Person**, die beim Ziehen erscheint.

Die [Schärfemaske](ansicht#Schärfemaske) zeigt zusätzlich, welche Teile eines Bildes scharf sind.

## Umbenennen

**Umbenennen …** ({taste:rename}) benennt die ausgewählten Dateien nach einer Vorlage um, mit Vorschau. Platzhalter fügt ein Klick ein: `{Name}`, `{Zähler}` (`{Zähler:3}` dreistellig, Startwert unter **Zähler ab**), `{Datum}`, `{Zeit}`, `{Jahr}`, `{Monat}`, `{Tag}`, `{Kamera}`, `{Objektiv}`, `{Session}` und `{Variante}`. Einstellungen und Varianten wandern mit; ist ein Name schon vergeben, wird „-2“ usw. angehängt.

## XMP

Bewertung, Farb-Label und Keywords lassen sich mit Lightroom, Bridge und Capture One über XMP-Dateien austauschen (Menü **Foto → XMP**):

- **Metadaten in XMP schreiben** und **Metadaten aus XMP lesen** für die Auswahl.
- **Änderungen automatisch in XMP schreiben** hält die XMP-Dateien laufend aktuell (standardmäßig aus).

Ändert ein anderes Programm eine XMP-Datei, übernimmt Silberkorn die Werte beim nächsten Laden. Entwicklungseinstellungen anderer Programme bleiben in der Datei unberührt.

## Verschieben und Papierkorb

In einer Session verschiebt **Nach „Selects“ verschieben** ({taste:to-selects}) die Auswahl in den Ordner **Selects**, **In den Papierkorb** ({taste:trash}) in den **Papierkorb** der Session; im Raster genügt auch {taste:delete-spot}. **Verschieben nach** im Menü **Foto** bietet die anderen Session-Ordner, die Favoriten und **Ordner wählen …**. Einstellungen wandern immer mit.

**Papierkorb leeren …** (Menü **Datei** oder Rechtsklick auf den Papierkorb) löscht die Bilder darin nach einer Rückfrage endgültig.

> Den Papierkorb gibt es nur in Sessions. Im Katalog lassen sich auf diesem Weg nur weitere Varianten löschen; die Dateien selbst bleiben.

## Ordner überwachen

**Ordner überwachen** (im Abschnitt **Ordner**, in einer Session bei den Session-Ordnern, auch im Menü **Datei**) lässt neue Bilder im angezeigten Ordner von selbst erscheinen, etwa wenn die Kamerasoftware beim Fotografieren mit Kabel in den Ordner **Aufnahmen** schreibt. Mit **Neue Aufnahme gleich öffnen** wird jeweils die neueste groß gezeigt.

Die Überwachung wirkt, sobald ein Ordner ohne Unterordner angezeigt wird; die Statusleiste zeigt dann „Überwacht: …“ mit dem Ordnernamen.
