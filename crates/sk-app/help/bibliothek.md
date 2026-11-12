# Bibliothek

Der Tab **Bibliothek** wählt, welche Bilder gezeigt werden: aus dem Katalog, einem Ordner, einem Album oder von einer Person. Die [Filterleiste](#Filterleiste) über den Bildern grenzt sie weiter ein. Bewertung, Farb-Label und Keywords stehen im Tab **Metadaten**.

## Ordner, Katalog und Sessions

Silberkorn kennt drei Arten, mit Bildern zu arbeiten. Sie schließen sich nicht aus: Ordner lassen sich immer durchsuchen, der Katalog ist immer da, eine Session ersetzt ihn nur, solange sie geöffnet ist.

### Ordner

Unter **Ordner** stehen die Ordner des Katalogs und der gerade gezeigte als Baum; daneben die Zahl ihrer Bilder samt Unterordnern. Ein Klick zeigt die Bilder eines Ordners, der Pfeil davor klappt ihn auf und zu. **+** öffnet einen anderen Ordner: Pfad eintippen oder **Durchsuchen …**, auch **Ordner öffnen …** ({taste:open-folder}) im Menü **Datei**. Ein Ordner zeigt auch die Bilder seiner Unterordner; wer nur seine eigenen sehen will, schaltet **Unterordner einbeziehen** in den Einstellungen (Seite **Bibliothek**) aus.

Ein nur durchsuchter Ordner wird nicht in den Katalog aufgenommen; ein Hinweis unter der Filterleiste sagt das. Erst wenn man ein Bild bearbeitet, kommt es an seinem Ort in den Katalog. **Importieren …** im Hinweis oder oben im Tab nimmt den ganzen Ordner samt Unterordnern auf.

### Katalog

Der Katalog sammelt Bilder aus beliebigen Ordnern und Laufwerken. Er bietet **Alle Bilder**, **Letzter Import**, seine Ordner sowie Alben, Filter und Suche über alles. Die Einstellungen der Bilder liegen im Katalog selbst, sodass auch Bilder auf schreibgeschützten Laufwerken bearbeitet werden können. Silberkorn sichert den Katalog jede Woche.

**Bereinigen** im Menü mit den drei Punkten neben **Katalog** entfernt Einträge, deren Dateien es nicht mehr gibt.

### Sessions

Eine Session ist ein abgeschlossener Projektordner, etwa für ein Shooting oder einen Kunden. Er enthält feste Unterordner:

- **Aufnahmen**: neue Bilder (Ordner `Capture`)
- **Selects**: die ausgewählten Bilder (`Selects`)
- **Ausgabe**: die exportierten Dateien (`Output`)
- **Papierkorb**: bis er geleert wird (`Trash`)

**Neue Session …** ({taste:new-session}) im Menü **Datei** fragt nach Name und Speicherort und legt den Ordner an. **Session öffnen …** ({taste:open-session}) öffnet eine `.sksession`-Datei, **Zuletzt verwendet** die letzten Sessions; die zuletzt offene öffnet sich beim Start wieder. **Session schließen** ({taste:close-session}) kehrt zum Katalog zurück.

Im Tab **Bibliothek** stehen dann oben die Ordner der Session, im Menü mit den drei Punkten daneben **Importieren …** und **Session schließen**. Rechtsklick auf **Papierkorb** leert ihn. Unter **Favoriten** liegen häufig genutzte Ordner; **+** fügt den aktuellen hinzu. Alben, Filter und Suche wirken nur auf die Bilder der Session. Die Einstellungen liegen als Datei neben jedem Bild; die ganze Session lässt sich verschieben, umbenennen oder auf einen anderen Rechner kopieren.

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

Das automatische Öffnen schaltet **Importdialog öffnen, wenn eine Speicherkarte erkannt wird** unten im Dialog ab, ebenso in den **Einstellungen …** auf der Seite **Bibliothek**.

## Bewerten und markieren

Alle drei wirken auf die ganze Auswahl und lassen sich mit {taste:undo} zurücknehmen. Sie stehen im Tab **Metadaten**, im Menü **Foto** und im Rechtsklick-Menü eines Vorschaubilds.

Sind mehrere Bilder ausgewählt, zeigt der Tab Metadaten gestrichelt, was nur einige davon haben: die Sterne bis zur höchsten Bewertung (daneben etwa **2–5 ★**), einen Ring um jedes vorkommende Farb-Label und Keywords mit **1/3**. Ein Klick auf einen Stern oder ein Label setzt es für alle, ein Klick auf **1/3** gibt das Keyword allen.

### Bewertung

{taste:rating-1} bis {taste:rating-5} vergeben ein bis fünf Sterne, {taste:rating-0} entfernt sie.

### Farb-Label

{taste:label-red} Rot, {taste:label-yellow} Gelb, {taste:label-green} Grün, {taste:label-blue} Blau. **Farb-Label: Keine** entfernt es.

### Keywords

Im Tab **Metadaten** unter **Keywords** ein Wort in das Feld mit dem **+** eingeben und Enter drücken; passende vorhandene Keywords erscheinen darunter gestrichelt und lassen sich anklicken. ✕ an einem Keyword entfernt es.

Darunter stehen unter **Vorschläge** bis zu acht Keywords, die zu dem passen, was auf dem Bild zu sehen ist (etwa *Strand*, *Hund*, *Sonnenuntergang*); ein Klick fügt eines hinzu. Sie kommen von derselben KI wie die Suche nach dem Bildinhalt und erscheinen, sobald das Bild einmal erfasst ist (beim ersten Öffnen etwa eine Sekunde).

### Aufnahme

Oben im Tab **Metadaten** stehen Dateiname, Format und Aufnahmedatum. **Aufnahme** zeigt Blende, Belichtungszeit, ISO und Brennweite groß, darunter Kamera und Objektiv. **Datei** nennt Auflösung, Megapixel und Seitenverhältnis, die Dateigröße und den Ordner; **Im Explorer** öffnet ihn. Den ganzen Pfad zeigt der Zeiger auf dem Ordnernamen. Bei mehreren ausgewählten Bildern gelten **Aufnahme** und **Datei** für das Hauptbild.

## Filterleiste

Die Leiste über den Bildern bleibt in jedem Tab sichtbar. Grenzt ein Filter die Bilder ein, steht in ihr etwa **7 von 12**, die Statusleiste zeigt **gefiltert** und das Symbol des Tabs **Bibliothek** einen blauen Punkt.

- Suchfeld: Dateiname, Kamera, Objektiv oder Keyword. {taste:find} setzt den Cursor hinein.
- **Bildinhalt**: was auf den Bildern zu sehen ist, in eigenen Worten und jeder Sprache, etwa *Kinder im Schnee* (siehe unten).
- **Bewertung** (≥ 1 ★ bis = 5 ★), **Farb-Label** und [Sichtung](#Sichtung): je ein Knopf mit Menü. Ein aktiver Filter wird zum blauen Chip; × hebt ihn auf.
- **Als Filteralbum** speichert den Filter als [Album](#Alben), **Zurücksetzen** hebt alle Filter auf.
- Ganz rechts das Symbol der Sortierung: Sein Menü bietet **Dateiname**, **Aufnahmedatum** und **Bewertung**, darunter die Richtung (aufsteigend oder absteigend).

### Suche nach dem Bildinhalt

Das Feld **Bildinhalt** findet Bilder nach dem, was sie zeigen, auch ohne Keywords. Die Treffer stehen nach Relevanz geordnet, die besten zuerst; die übrigen Filter gelten weiter. **Ähnliche finden** (Rechtsklick auf ein Vorschaubild oder Menü **Foto**) zeigt die Bilder, die dem gewählten ähneln, etwa die übrigen Aufnahmen einer Szene; ✕ neben „Ähnlich wie …“ beendet das.

Dafür erfasst eine KI jedes Bild einmal (auf dem Vorschaubild der Kamera, je nach Rechner 0,2 bis 1,5 Sekunden). Das geschieht bei der ersten Suche für alle Bilder der Ansicht im Hintergrund; die Statusleiste zeigt den Fortschritt, und die Treffer erscheinen nach und nach. Danach ist jede Suche sofort da. Das Ergebnis liegt im Katalog bzw. in der Session.

Bilder, deren Inhalt nicht erfasst ist, stehen nicht unter den Treffern; ihre Zahl steht unter der Filterleiste. Wurde das Erfassen abgebrochen, setzt **Erfassen** daneben es fort.

Ein Filteralbum speichert die Suche nach dem Bildinhalt nicht mit.

## Alben

Ein Album sammelt Bilder unabhängig von ihrem Ordner. **+** neben **Alben** legt eines mit der aktuellen Auswahl an. **Als Filteralbum** in der Filterleiste speichert den aktuellen Filter: Das Album zeigt immer alle Bilder, die gerade dazu passen.

Doppelklick auf ein Album benennt es um. Rechtsklick bietet **Auswahl hinzufügen**, **Auswahl aus Album entfernen**, **Umbenennen** und **Löschen …**; die Bilder selbst bleiben dabei erhalten.

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

## Sichtung

Die Sichtung hilft, aus vielen ähnlichen Bildern die guten zu finden. Das Menü **Sichtung** in der Filterleiste zeigt **Brauchbar** (scharf, Augen offen) oder **Aussortieren** (Augen geschlossen oder unscharf). Darunter steht, wie viele Bilder der Ansicht analysiert sind; **Ansicht analysieren** untersucht die übrigen im Hintergrund, sind alle fertig, analysiert **Erneut analysieren** sie noch einmal. Beim ersten Mal werden etwa 40 MB geladen.

Nicht analysierte Bilder zeigt keiner der beiden Filter. Im Raster markiert ein Augen-Symbol geschlossene Augen, ein Tropfen-Symbol ein unscharfes Bild. Auch die Sichtung lässt sich als Filteralbum speichern.

Die [Schärfemaske](ansicht#Schärfemaske) zeigt zusätzlich, welche Teile eines Bildes scharf sind.

## Personen

Die Analyse der Sichtung erkennt auch Gesichter. Unter **Personen** stehen sie als Kacheln:

- Klick zeigt ihre Bilder, ein zweiter wieder alle. Doppelklick gibt ihr einen Namen; er wird zum Keyword ihrer Bilder.
- Wurde dieselbe Person doppelt erkannt, eine Kachel auf die andere ziehen.
- Ist eine Person gewählt, stehen darunter die Ausschnitte, auf denen sie erkannt wurde, die unsichersten zuerst; **Alle … zeigen** zeigt den Rest, × blendet sie aus. Falsch zugeordnete auf die richtige Person ziehen oder auf **Neue Person**, die beim Ziehen erscheint.

## Umbenennen

**Umbenennen …** ({taste:rename}) benennt die ausgewählten Dateien nach einer Vorlage um, mit Vorschau. Platzhalter fügt ein Klick ein: `{Name}`, `{Zähler}` (`{Zähler:3}` dreistellig, Startwert unter **Zähler ab**), `{Datum}`, `{Zeit}`, `{Jahr}`, `{Monat}`, `{Tag}`, `{Kamera}`, `{Objektiv}`, `{Session}` und `{Variante}`. Einstellungen und Varianten wandern mit; ist ein Name schon vergeben, wird „-2“ usw. angehängt.

## XMP

Bewertung, Farb-Label und Keywords lassen sich mit Lightroom, Bridge und Capture One über XMP-Dateien austauschen (Menü **Foto → XMP**):

- **Metadaten in XMP schreiben** und **Metadaten aus XMP lesen** für die Auswahl.
- **Änderungen automatisch in XMP schreiben** (in den **Einstellungen …**, Seite **Bibliothek**) hält die XMP-Dateien laufend aktuell (standardmäßig aus).

Ändert ein anderes Programm eine XMP-Datei, übernimmt Silberkorn die Werte beim nächsten Laden. Entwicklungseinstellungen anderer Programme bleiben in der Datei unberührt.

## Verschieben und Papierkorb

In einer Session verschiebt **Nach „Selects“ verschieben** ({taste:to-selects}) die Auswahl in den Ordner **Selects**, **In den Papierkorb** ({taste:trash}) in den **Papierkorb** der Session; im Raster genügt auch {taste:delete-spot}. **Verschieben nach** im Menü **Foto** bietet die anderen Session-Ordner, die Favoriten und **Ordner wählen …**. Einstellungen wandern immer mit.

**Papierkorb leeren …** (Menü **Datei** oder Rechtsklick auf den Papierkorb) löscht die Bilder darin nach einer Rückfrage endgültig.

> Den Papierkorb gibt es nur in Sessions. Im Katalog lassen sich auf diesem Weg nur weitere Varianten löschen; die Dateien selbst bleiben.

## Ordner überwachen

**Ordner überwachen** (im Menü **Datei** und in den **Einstellungen …** auf der Seite **Bibliothek**) lässt neue Bilder im angezeigten Ordner von selbst erscheinen, etwa wenn die Kamerasoftware beim Fotografieren mit Kabel in den Ordner **Aufnahmen** schreibt. Mit **Neue Aufnahme gleich öffnen** wird jeweils die neueste groß gezeigt.

Die Überwachung wirkt, sobald ein Ordner angezeigt wird (neue Bilder in ihm selbst, nicht in seinen Unterordnern); die Statusleiste zeigt dann „Überwacht: …“ mit dem Ordnernamen.
