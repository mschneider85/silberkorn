# Erste Schritte

Silberkorn entwickelt RAW-Dateien und verwaltet Fotos. Bearbeitet wird nie die Originaldatei: Alle Einstellungen werden getrennt gespeichert und lassen sich jederzeit ändern oder zurücksetzen.

## Die Oberfläche

### Kopfleiste

Ganz oben stehen die Menüs **Datei**, **Bearbeiten**, **Foto**, **Entwickeln**, **Ansicht** und **Hilfe**, in der Mitte das Suchfeld der [Befehlssuche](erste-schritte#Befehlssuche). Darunter liegt die Werkzeugleiste mit den häufigsten Aktionen: **Ordner öffnen**, Import, **Raster** und **Einzelbild**, **Rückgängig** und **Wiederholen**, Drehen, **Zuschneiden**, **Geraderichten**, **Weißabgleich**, Anpassungen kopieren und einfügen, Vergleichsansichten, Zoom und rechts der blaue Knopf **Exportieren**.

### Tab-Leiste

Die schmale Leiste links wechselt zwischen den Werkzeug-Tabs: **Bibliothek** und **Metadaten** zum Ordnen, **Objektiv**, **Belichtung**, **Farbe**, **Details**, **Effekte**, **Ebenen** und **Retusche** zum Entwickeln, dann **Stile**, **Verlauf** und **Ausgabe**. Strg+1 bis Strg+0 wählen die Tabs in dieser Reihenfolge (außer **Verlauf**).

### Werkzeugspalte

Neben der Tab-Leiste stehen die Werkzeuge des gewählten Tabs in aufklappbaren Abschnitten. Im Kopf eines Werkzeugs sitzen kleine Knöpfe: ↺ setzt es zurück, **A** stellt es automatisch ein, und sind mehrere Bilder ausgewählt, überträgt das Kopieren-Symbol das Werkzeug auf die anderen (siehe [Verlauf und Anpassungen](verlauf)).

Regler bedient man durch Ziehen. Doppelklick auf den Namen eines Reglers setzt ihn zurück. Nach einem Klick auf einen Regler ändern ihn auch die Pfeiltasten und das Mausrad (Umschalt: grob, Alt: fein).

### Bild und Filmstreifen

In der Mitte steht das Bild, entweder als Raster aller Bilder oder groß als Einzelbild ({taste:grid} schaltet um). Unter dem Einzelbild zeigt der Filmstreifen die übrigen Bilder der Ansicht. Rechts neben dem Bild erscheint, wenn Platz ist, die Seitenleiste mit Histogramm, Bildinfo, den Bearbeitungen und den letzten Schritten. Mehr dazu im Kapitel [Ansicht](ansicht).

### Statusleiste

Unten meldet die Statusleiste, was gerade passiert. Alles, was länger dauert (Export, Zusammenfügen, KI-Berechnungen, Vorschauen), zeigt hier seinen Fortschritt; Export und Zusammenfügen lassen sich mit ✕ abbrechen. Rechts stehen die Zahl der Bilder in der Ansicht und ein Regler für die Größe der Vorschaubilder.

## Befehlssuche

Strg+K (oder ein Klick auf das Suchfeld in der Kopfleiste) öffnet die **Befehlssuche**. Ein paar Buchstaben genügen („weiß“, „rausch“, „hdr“), und sie findet jeden Befehl, jeden Tab und jeden Werkzeug-Abschnitt. Enter führt den Befehl aus bzw. springt zum Werkzeug. Rechts steht jeweils das Tastenkürzel.

> Wer nicht weiß, wo eine Funktion steckt, findet sie mit Strg+K meist am schnellsten.

## Bilder öffnen

Silberkorn arbeitet auf drei Arten:

- **Ordner öffnen …** ({taste:open-folder}) zeigt die Bilder eines Ordners, ohne sie aufzunehmen.
- Der **Katalog** sammelt Bilder aus beliebigen Ordnern an einem Ort, mit Alben, Filtern und Suche.
- Eine **Session** ist ein abgeschlossener Projektordner, etwa für ein Shooting.

Wie sich die drei unterscheiden und wann man welche nimmt, steht im Kapitel [Bibliothek](bibliothek#Ordner, Katalog und Sessions). Neue Bilder kommen über den [Import](bibliothek#Import) hinzu; steckt man eine Speicherkarte ein, öffnet sich der Importdialog von selbst.

## Ein Bild bewerten

Ein Klick wählt ein Bild, Strg+Klick und Umschalt+Klick wählen mehrere. Dann:

- {taste:rating-1} bis {taste:rating-5} vergeben Sterne, {taste:rating-0} entfernt die Bewertung.
- {taste:label-red}, {taste:label-yellow}, {taste:label-green} und {taste:label-blue} setzen ein Farb-Label.
- {taste:previous} und {taste:next} blättern zum vorherigen bzw. nächsten Bild.

Bewertung und Farb-Label gelten für die ganze Auswahl. Mit **Filter & Suche** im Tab **Bibliothek** zeigt man danach etwa nur Bilder ab drei Sternen.

## Ein Bild entwickeln

Ein Doppelklick im Raster öffnet ein Bild groß. Ein neues Bild startet so, wie die Kamera es selbst entwickelt hätte; die Regler setzen darauf auf.

1. **Automatisch anpassen** ({taste:auto-adjust}, oben im Tab **Belichtung**) stellt Weißabgleich, Belichtung, Kontrast, Lichter, Tiefen und Dynamik in einem Schritt ein. Mit mehreren ausgewählten Bildern werden alle angepasst. Der Regler **Stärke** darunter schwächt das Ergebnis ab.
2. Im Tab **Belichtung** regeln **Belichtung**, **Kontrast**, **Lichter**, **Tiefen**, **Weiß** und **Schwarz** die Helligkeit.
3. Im Tab **Farbe** stellt **Weißabgleich** mit **Temperatur** und **Tönung** die Farbe des Lichts ein. Schneller geht es mit der **Weißabgleich-Pipette** ({taste:wb-picker}): auf eine Stelle klicken, die neutral grau oder weiß sein soll.
4. {taste:before} zeigt zum Vergleich das Bild ohne Bearbeitung.

Jeder Schritt lässt sich mit {taste:undo} zurücknehmen. Alles Weitere zu den Werkzeugen steht in den Kapiteln [Entwickeln](entwickeln), [Objektiv](objektiv), [Ebenen und Masken](ebenen-masken), [Retusche](retusche) und [KI-Werkzeuge](ki-werkzeuge).

## Exportieren

Bilder auswählen und **Exportieren** ({taste:export}) drücken. Ohne weitere Einstellungen entsteht ein JPEG in voller Größe im Unterordner `Output` neben dem Bild. Format, Größe, Ordner und Wasserzeichen legen die Ausgaberezepte im Tab **Ausgabe** fest, siehe [Ausgabe](ausgabe).

## Hilfe und Tutorials

- **Hilfe** ({taste:help}) öffnet dieses Handbuch.
- **Tutorials …** im Menü **Hilfe** führt Schritt für Schritt durch typische Arbeiten.
- **Einstellungen …** ({taste:settings}, im Menü **Bearbeiten**) fasst zusammen, was das Programm einrichtet: **Allgemein** (Katalog und seine Sicherungen, Updates, wo Protokolle und Einstellungen liegen), **Bibliothek** (Ordner überwachen, Speicherkarten, XMP-Dateien), **Grafikkarte**, **Speicherplatz** und **Tastenkürzel**. Unter **Tastenkürzel** stehen alle Kürzel und lassen sich ändern; die Übersicht steht auch unter [Tastenkürzel](tastenkuerzel).

## Wie geht es weiter

- [Bibliothek](bibliothek): Ordner, Katalog, Sessions, Import, Bewerten, Alben, Stapel
- [Ansicht](ansicht): Raster, Zoom, Vergleichsansichten, Histogramm, Hilfslinien
- [Verlauf und Anpassungen](verlauf): Rückgängig, Schnappschüsse, Kopieren, Zurücksetzen
- [Vergleichen und Looks](vergleichen): Referenzbild, Look angleichen, Stile
- [Zusammenfügen](zusammenfuegen): HDR, Panorama, Focus Stacking, Langzeitbelichtung
- [Ausgabe](ausgabe): Exportieren, Wasserzeichen, Softproof, Druckvorschau
